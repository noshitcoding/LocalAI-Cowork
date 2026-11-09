//! Actual executor admission and encrypted original offline recovery through the user daemon.
use super::*;
use crate::enterprise_policy::{digest, Pin};
use std::io::IsTerminal;
#[derive(Debug)]
struct RemoteRejection(u16);
impl std::fmt::Display for RemoteRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "actual grant service rejected request ({})", self.0)
    }
}
impl std::error::Error for RemoteRejection {}
async fn request(
    client: &ControlPlaneClient,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> Result<Value> {
    let mut q = client
        .http
        .request(method, format!("{}{}", client.server_url, path))
        .bearer_auth(&client.token)
        .timeout(Duration::from_secs(8));
    if let Some(body) = body {
        q = q.json(body)
    }
    let mut r = q.send().await?;
    let status = r.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = r.chunk().await? {
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            bail!("actual enterprise response exceeds bounded body")
        };
        bytes.extend_from_slice(&chunk)
    }
    if !status.is_success() {
        return Err(RemoteRejection(status.as_u16()).into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn path(client: &ControlPlaneClient, suffix: &str) -> String {
    format!(
        "/api/v1/agent/executors/{}/enterprise/{suffix}",
        client.executor_id
    )
}
pub(super) async fn desktop_watch(
    client: &ControlPlaneClient,
    config: &Config,
    lease: &RunLease,
) -> anyhow::Error {
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let Some(daemon) = &config.local_daemon else {
            return anyhow::anyhow!("original native desktop custody missing");
        };
        if let Err(e) = admit(client, config, daemon, lease, None, false).await {
            return e;
        };
        let record = daemon
            .call("runs.get", json!({"run_id":lease.run.spec.id}))
            .await;
        match record {
            Ok(v) => {
                if let Ok(r) = serde_json::from_value::<RunRecord>(v) {
                    if r.state.is_terminal() || r.state == RunState::Interrupted {
                        return anyhow::anyhow!("original desktop run no longer executing");
                    }
                } else {
                    return anyhow::anyhow!("original desktop run missing");
                }
            }
            Err(e) => return e,
        }
    }
}
pub(super) async fn refresh(
    client: &ControlPlaneClient,
    daemon: &LocalDaemonClient,
    pin: &Pin,
) -> Result<()> {
    let status = request(
        client,
        Method::GET,
        &path(client, &format!("device-grants/{}", pin.grant_id)),
        None,
    )
    .await;
    match status {
        Ok(status) => {
            if status["current_authorized"] != true
                || status["state"] != "active"
                || status["revision"] != json!(pin.revision)
                || status["policy_sha256"] != json!(pin.policy_sha256)
            {
                let _ = daemon
                    .call("enterprise.grants.stop", json!({"grant_id":pin.grant_id}))
                    .await;
                bail!("current actual source grant revoked or changed")
            };
            daemon
                .call(
                    "enterprise.grants.observe",
                    json!({"grant_id":pin.grant_id,"status":status}),
                )
                .await?;
            Ok(())
        }
        Err(e) => {
            if e.downcast_ref::<RemoteRejection>().is_some() {
                let _ = daemon
                    .call("enterprise.grants.stop", json!({"grant_id":pin.grant_id}))
                    .await;
            }
            Err(e)
        }
    }
}
pub(super) async fn admit(
    client: &ControlPlaneClient,
    config: &Config,
    daemon: &LocalDaemonClient,
    lease: &RunLease,
    workspace: Option<&Path>,
    materialize: bool,
) -> Result<()> {
    let pin =
        Pin::from_input(&lease.run.spec.input)?.context("original enterprise grant missing")?;
    pin.validate()?;
    if config.kind != ExecutorKind::PersonalDevice {
        bail!("native enterprise policy requires its actual personal-device runtime")
    };
    daemon.verify_device(config.executor_id).await?;
    refresh(client, daemon, &pin).await?;
    let proof = daemon
        .call(
            "enterprise.runs.admit",
            json!({"run_spec":lease.run.spec,"workspace_path":workspace,"materialize":materialize}),
        )
        .await?;
    if proof["admitted"] != true {
        bail!("actual original local enterprise approval unavailable")
    };
    Ok(())
}
pub(super) async fn permit(
    client: &ControlPlaneClient,
    daemon: &LocalDaemonClient,
    lease: &RunLease,
) -> Result<()> {
    let pin = Pin::from_input(&lease.run.spec.input)?.context("original pin missing")?;
    if pin.offline_seconds == 0 {
        return Ok(());
    }
    let prepared=daemon.call("enterprise.offline.prepare",json!({"lease":lease,"server_url":client.server_url,"credential_sha256":digest(&client.token)?})).await?;
    if !prepared["permit"].is_null() {
        return Ok(());
    }
    let body = json!({"request_id":prepared["request_id"],"lease_token":lease.lease_token});
    let actual = request(
        client,
        Method::POST,
        &format!(
            "/api/v1/agent/executors/{}/runs/{}/enterprise/offline-permit",
            client.executor_id, lease.run.spec.id
        ),
        Some(&body),
    )
    .await?;
    daemon
        .call(
            "enterprise.offline.accept",
            json!({"run_id":lease.run.spec.id,"permit":actual}),
        )
        .await?;
    Ok(())
}
async fn sync_batches(
    client: &ControlPlaneClient,
    daemon: &LocalDaemonClient,
    lease: &RunLease,
) -> Result<()> {
    for _ in 0..10 {
        let batch = daemon
            .call(
                "enterprise.offline.batch",
                json!({"run_id":lease.run.spec.id}),
            )
            .await?;
        if batch.is_null() {
            return Ok(());
        }
        let permit: Uuid = serde_json::from_value(batch["permit_id"].clone())?;
        let receipt = request(
            client,
            Method::POST,
            &path(client, &format!("offline-permits/{permit}/reconcile")),
            Some(&batch["body"]),
        )
        .await?;
        daemon
            .call(
                "enterprise.offline.ack",
                json!({"run_id":lease.run.spec.id,"receipt":receipt}),
            )
            .await?;
    }
    Ok(())
}
pub(super) async fn relay(
    client: &ControlPlaneClient,
    daemon: &LocalDaemonClient,
    lease: &RunLease,
    workspace: Option<&Path>,
) -> Result<Value> {
    let pin = Pin::from_input(&lease.run.spec.input)?.context("original pin missing")?;
    let mut cursor = 0;
    let mut refresh_at = tokio::time::Instant::now();
    loop {
        if refresh_at.elapsed() >= Duration::from_secs(3) {
            let _ = refresh(client, daemon, &pin).await;
            refresh_at = tokio::time::Instant::now();
        }
        let record: RunRecord = serde_json::from_value(
            daemon
                .call("runs.get", json!({"run_id":lease.run.spec.id}))
                .await?,
        )?;
        let events: Vec<RunEvent> = serde_json::from_value(
            daemon
                .call(
                    "runs.events",
                    json!({"run_id":lease.run.spec.id,"after":cursor}),
                )
                .await?,
        )?;
        for event in events {
            let delivered = match event.kind {
                RunEventKind::ApprovalRequested if !record.state.is_terminal() => {
                    relay_local_approval(client, daemon, lease, &event).await
                }
                RunEventKind::InputRequested if !record.state.is_terminal() => {
                    relay_local_input(client, daemon, lease, &event).await
                }
                RunEventKind::CheckpointCreated => {
                    relay_local_checkpoint(client, daemon, lease, &event).await
                }
                RunEventKind::ArtifactCreated => {
                    relay_local_artifact(client, lease, workspace, &event).await
                }
                RunEventKind::Created
                | RunEventKind::StateChanged
                | RunEventKind::ApprovalResolved
                | RunEventKind::InputReceived
                | RunEventKind::Completed
                | RunEventKind::Failed => Ok(()),
                _ => client.append_event(lease, &event).await,
            };
            if delivered.is_err() && pin.offline_seconds == 0 {
                return delivered.map(|_| Value::Null);
            }
            cursor = event.sequence;
        }
        if pin.offline_seconds > 0 {
            let _ = sync_batches(client, daemon, lease).await;
        }
        match record.state {
            RunState::Completed => {
                refresh(client, daemon, &pin).await?;
                return Ok(record.result.unwrap_or(Value::Null));
            }
            RunState::Failed | RunState::Interrupted | RunState::Canceled | RunState::Expired => {
                bail!(
                "original native enterprise run ended {:?}; no replay; original record retained",
                record.state
            )
            }
            _ => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
}
// This loop reads original encrypted custody only. It never imports, starts or retries a run.
pub(super) async fn recover_loop(client: ControlPlaneClient, daemon: LocalDaemonClient) {
    let _ = daemon
        .call("enterprise.containers.recover", json!({}))
        .await;
    loop {
        if let Err(e) = recover_once(&client, &daemon).await {
            tracing::warn!(error=%e,"original enterprise offline reconciliation pending")
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
async fn recover_once(client: &ControlPlaneClient, daemon: &LocalDaemonClient) -> Result<()> {
    let pending = daemon
        .call(
            "enterprise.offline.pending",
            json!({"server_url":client.server_url,"credential_sha256":digest(&client.token)?}),
        )
        .await?;
    for entry in pending
        .as_array()
        .context("actual pending original list missing")?
    {
        if let Err(error) = recover_entry(client, daemon, entry).await {
            tracing::warn!(run_id=?entry["lease"]["run"]["spec"]["id"],error=%error,"retained original reconciliation remains pending");
        }
    }
    Ok(())
}
async fn recover_entry(
    client: &ControlPlaneClient,
    daemon: &LocalDaemonClient,
    entry: &Value,
) -> Result<()> {
    let lease: RunLease = serde_json::from_value(entry["lease"].clone())?;
    if entry["permit"].is_null() {
        return Ok(());
    }
    let pin = Pin::from_input(&lease.run.spec.input)?.context("original pin missing")?;
    let current = refresh(client, daemon, &pin).await.is_ok();
    let id: Uuid = serde_json::from_value(entry["permit"]["permit_id"].clone())?;
    let observed = request(
        client,
        Method::GET,
        &path(client, &format!("offline-permits/{id}")),
        None,
    )
    .await?;
    if observed["permit"] != entry["permit"] {
        bail!("actual server original permit changed")
    };
    if let Some(receipts) = observed["reconciliations"].as_array() {
        for r in receipts {
            let _ = daemon
                .call(
                    "enterprise.offline.ack",
                    json!({"run_id":lease.run.spec.id,"receipt":r}),
                )
                .await;
        }
    }
    sync_batches(client, daemon, &lease).await?;
    let record: RunRecord = serde_json::from_value(
        daemon
            .call("runs.get", json!({"run_id":lease.run.spec.id}))
            .await?,
    )?;
    if current && record.state == RunState::Completed && !entry["completion"].is_null() {
        let original: CompleteRunRequest = serde_json::from_value(entry["completion"].clone())?;
        if original.lease_token != lease.lease_token
            || original.result != record.result.unwrap_or(Value::Null)
        {
            bail!("retained original bridge completion differs from the local run")
        }
        let body = serde_json::to_value(original)?;
        let actual = request(
            client,
            Method::POST,
            &format!(
                "/api/v1/agent/executors/{}/runs/{}/complete",
                client.executor_id, lease.run.spec.id
            ),
            Some(&body),
        )
        .await?;
        daemon
            .call(
                "enterprise.offline.settled",
                json!({"run_id":lease.run.spec.id,"receipt":actual}),
            )
            .await?;
    }
    Ok(())
}
pub(super) async fn status_loop(client: ControlPlaneClient, daemon: LocalDaemonClient) {
    loop {
        if let Ok(value) = daemon.call("runs.list_active", json!({})).await {
            if let Some(runs) = value["items"].as_array() {
                for run in runs {
                    if let Ok(record) = serde_json::from_value::<RunRecord>(run.clone()) {
                        if let Ok(Some(pin)) = Pin::from_input(&record.spec.input) {
                            let _ = refresh(&client, &daemon, &pin).await;
                        }
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
pub(super) async fn transfer_loop(client: ControlPlaneClient, daemon: LocalDaemonClient) {
    loop {
        if let Err(error) = crate::enterprise_transfers::once(&client, &daemon).await {
            tracing::warn!(error=%error,"native original transfer discovery pending")
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
pub(super) async fn cli(
    client: &ControlPlaneClient,
    config: &Config,
    args: &[String],
) -> Result<()> {
    let command = args
        .first()
        .map(String::as_str)
        .context("native enterprise command missing")?;
    let daemon = config
        .local_daemon
        .as_ref()
        .context("native consent requires the actual personal user daemon")?;
    daemon.verify_device(config.executor_id).await?;
    match command {
        "enterprise-grants" => {
            if args.len() != 1 {
                bail!("enterprise-grants takes no arguments")
            };
            let grants = request(client, Method::GET, &path(client, "device-grants"), None).await?;
            println!("{}", serde_json::to_string_pretty(&grants)?);
        }
        "enterprise-grant-stop" => {
            let id: Uuid = args.get(1).context("grant ID required")?.parse()?;
            if args.len() != 2 {
                bail!("enterprise-grant-stop requires exactly one original grant ID")
            };
            println!(
                "{}",
                daemon
                    .call("enterprise.grants.stop", json!({"grant_id":id}))
                    .await?
            );
        }
        "enterprise-grant-resume" => {
            let id: Uuid = args.get(1).context("grant ID required")?.parse()?;
            if args.len() != 2 {
                bail!("enterprise-grant-resume requires exactly one original grant ID")
            };
            let original = daemon
                .call("enterprise.grants.confirmation", json!({"grant_id":id}))
                .await?;
            confirm(client, daemon, id, &original).await?;
        }
        "enterprise-grant-approve" => {
            if !(args.len() == 2 || args.len() == 3) {
                bail!("enterprise-grant-approve GRANT_ID [LOCAL_ROOTS_JSON_FILE]")
            };
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                bail!("explicit local native consent requires an interactive terminal")
            };
            let id: Uuid = args[1].parse()?;
            let status = request(
                client,
                Method::GET,
                &path(client, &format!("device-grants/{id}")),
                None,
            )
            .await?;
            if status["current_authorized"] != true || status["state"] != "pending" {
                bail!("current original pending grant required; use resume for an already approved original")
            };
            let roots: BTreeMap<String, PathBuf> = if let Some(file) = args.get(2) {
                let path = Path::new(file);
                if fs::metadata(path)?.len() > 65536 {
                    bail!("local root decision file exceeds limit")
                };
                serde_json::from_slice(&fs::read(path)?)?
            } else {
                BTreeMap::new()
            };
            println!("Original server/device/chat/mode/capabilities/expiry:\n{}\nActual locally selected paths:\n{}",serde_json::to_string_pretty(&status)?,serde_json::to_string_pretty(&roots)?);
            println!("Full grants permit host processes, tools and credentials available to your local user. Selected grants confine file tools to the chosen directories. Sandbox uses the locally qualified isolated OCI image. Type the entire original policy SHA256 to authorize this original grant:");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            let sha = status["policy_sha256"]
                .as_str()
                .context("original policy SHA missing")?;
            if input.trim() != sha {
                bail!("local original policy was not explicitly confirmed")
            };
            let sandbox = if status["policy"]["access"] == "sandbox" {
                Some(daemon.call("enterprise.sandbox.qualify",json!({"docker":env::var("COWORK_ENTERPRISE_DOCKER").context("locally selected absolute Docker executable missing")?,"image":env::var("COWORK_ENTERPRISE_SANDBOX_IMAGE").context("locally selected immutable OCI image ID missing")?,"context":env::var("COWORK_ENTERPRISE_DOCKER_CONTEXT").context("locally selected explicit Docker context missing")?})).await?)
            } else {
                None
            };
            let original=daemon.call("enterprise.grants.approve",json!({"server_url":client.server_url,"grant":status,"roots":roots,"local_confirmation":sha,"sandbox":sandbox})).await?;
            confirm(client, daemon, id, &original).await?;
        }
        _ => bail!("unknown native enterprise command"),
    };
    Ok(())
}
async fn confirm(
    client: &ControlPlaneClient,
    daemon: &LocalDaemonClient,
    id: Uuid,
    original: &Value,
) -> Result<()> {
    let receipt = request(
        client,
        Method::POST,
        &path(client, &format!("device-grants/{id}/confirm")),
        Some(&original["confirmation"]),
    )
    .await?;
    let status = request(
        client,
        Method::GET,
        &path(client, &format!("device-grants/{id}")),
        None,
    )
    .await?;
    if status["confirmation"] != receipt {
        bail!("current server status differs from actual original confirmation")
    };
    daemon
        .call(
            "enterprise.grants.observe",
            json!({"grant_id":id,"status":status}),
        )
        .await?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}
