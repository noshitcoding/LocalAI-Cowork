//! Native-only encrypted original consent, actual paths, stop fences and original offline custody.
use super::*;
use crate::enterprise_policy::{digest, known_root, Custody, Pin};
use cowork_contracts::enterprise::LaptopAccess;
use std::collections::BTreeMap;
const SCHEMA:&str="CREATE TABLE IF NOT EXISTS daemon_enterprise_grants(grant_id TEXT PRIMARY KEY,device_id TEXT NOT NULL,server_url TEXT NOT NULL,custody_sha256 TEXT NOT NULL,encrypted_original TEXT NOT NULL,confirmation TEXT,last_seen_at TEXT,stopped INTEGER NOT NULL DEFAULT 0);CREATE TABLE IF NOT EXISTS daemon_enterprise_clock(id INTEGER PRIMARY KEY CHECK(id=1),highwater TEXT NOT NULL);";
pub(super) fn clock(db: &Connection) -> Result<DateTime<Utc>> {
    let local = Utc::now();
    let old: Option<String> = db
        .query_row(
            "SELECT highwater FROM daemon_enterprise_clock WHERE id=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let old = old
        .map(|t| DateTime::parse_from_rfc3339(&t).map(|d| d.with_timezone(&Utc)))
        .transpose()?;
    static ANCHOR: std::sync::OnceLock<(std::time::Instant, DateTime<Utc>)> =
        std::sync::OnceLock::new();
    let floor = old.map_or(local, |o| o.max(local));
    let anchor = ANCHOR.get_or_init(|| (std::time::Instant::now(), floor));
    let monotonic = anchor.1 + chrono::Duration::from_std(anchor.0.elapsed())?;
    let at = floor.max(monotonic);
    db.execute("INSERT INTO daemon_enterprise_clock(id,highwater) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET highwater=excluded.highwater",[at.to_rfc3339()])?;
    Ok(at)
}
pub(super) async fn hold(daemon: &Daemon, id: Uuid, message: &str) -> Result<()> {
    let db = daemon.database.lock().await;
    let raw: String = db.query_row(
        "SELECT record_json FROM daemon_runs WHERE id=?1",
        [id.to_string()],
        |r| r.get(0),
    )?;
    let mut run: RunRecord = serde_json::from_str(&raw)?;
    if run.state.is_terminal() {
        return Ok(());
    }
    run.state = RunState::Interrupted;
    run.revision += 1;
    run.updated_at = Utc::now();
    run.error = Some(RunError {
        code: "native_enterprise_held".into(),
        message: message.into(),
        retryable: false,
        details: json!({"original_effects_unknown":true,"replay_allowed":false,"physical_stop_confirmed":false}),
    });
    refresh_etag(&mut run);
    save_record(&db, &run)?;
    append_event(
        &db,
        id,
        RunEventKind::Warning,
        json!({"code":"native_enterprise_held","message":message,"original_effects_unknown":true,"replay_allowed":false}),
    )
}
fn decode_custody(daemon: &Daemon, encrypted: &str) -> Result<Custody> {
    let plain = decrypt_secret_payload(encrypted, &daemon.config.model_secret_key)?;
    Ok(serde_json::from_slice(&plain)?)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approve {
    server_url: String,
    grant: Value,
    roots: BTreeMap<String, PathBuf>,
    local_confirmation: String,
    sandbox: Option<crate::enterprise_sandbox::Qualification>,
}
pub(super) async fn approve(daemon: &Daemon, params: Value) -> Result<Value> {
    let q: Approve = serde_json::from_value(params)?;
    let expected = q.grant["policy_sha256"]
        .as_str()
        .context("original policy missing")?;
    if q.local_confirmation != expected {
        bail!("explicit original local consent required")
    };
    let mut roots = BTreeMap::new();
    let private = daemon
        .config
        .database_path
        .parent()
        .context("native private data parent missing")?
        .canonicalize()?;
    for (alias, path) in q.roots {
        let path = known_root(&path)?;
        if path.starts_with(&private) || private.starts_with(&path) {
            bail!("selected native roots cannot expose the private credential/consent store")
        };
        roots.insert(alias, path);
    }
    if let Some(sandbox) = &q.sandbox {
        sandbox.current().await?;
    }
    let sandbox = q.sandbox.map(|s| serde_json::to_value(s)).transpose()?;
    // Recover the immutable local decision before creating another timestamp.
    {
        let db = daemon.database.lock().await;
        db.execute_batch(SCHEMA)?;
        let id: Uuid = serde_json::from_value(q.grant["id"].clone())?;
        if let Some((sealed, stopped)) = db
            .query_row(
                "SELECT encrypted_original,stopped FROM daemon_enterprise_grants WHERE grant_id=?1",
                [id.to_string()],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
            )
            .optional()?
        {
            let old = decode_custody(daemon, &sealed)?;
            if stopped
                || old.server_url != q.server_url.trim_end_matches('/')
                || old.original_grant != q.grant
                || old.roots != roots
                || old.sandbox != sandbox
                || old.expires_at <= clock(&db)?
            {
                bail!("different or stopped immutable original native decision");
            }
            return Ok(
                json!({"grant_id":id,"local_custody_sha256":digest(&old)?,"confirmation":old.confirmation()?}),
            );
        }
    }
    let c = Custody::from_original(
        &q.server_url,
        daemon.config.device_id,
        q.grant,
        roots,
        sandbox,
    )?;
    let sha = digest(&c)?;
    let sealed = encrypt_secret_payload(&serde_json::to_vec(&c)?, &daemon.config.model_secret_key)?;
    let db = daemon.database.lock().await;
    db.execute_batch(SCHEMA)?;
    let at = clock(&db)?;
    if c.expires_at <= at {
        bail!("original native consent expired")
    };
    if let Some(old) = db
        .query_row(
            "SELECT custody_sha256 FROM daemon_enterprise_grants WHERE grant_id=?1",
            [c.grant_id.to_string()],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        if old != sha {
            bail!("a different immutable original native decision already exists")
        }
    } else {
        db.execute("INSERT INTO daemon_enterprise_grants(grant_id,device_id,server_url,custody_sha256,encrypted_original) VALUES(?1,?2,?3,?4,?5)",params![c.grant_id.to_string(),c.device_id.to_string(),c.server_url,sha,sealed])?;
    }
    Ok(json!({"grant_id":c.grant_id,"local_custody_sha256":sha,"confirmation":c.confirmation()?}))
}
pub(super) async fn confirmation(daemon: &Daemon, params: Value) -> Result<Value> {
    let id: Uuid = serde_json::from_value(params["grant_id"].clone())?;
    let db = daemon.database.lock().await;
    db.execute_batch(SCHEMA)?;
    let encrypted: String = db.query_row(
        "SELECT encrypted_original FROM daemon_enterprise_grants WHERE grant_id=?1 AND stopped=0",
        [id.to_string()],
        |r| r.get(0),
    )?;
    let c = decode_custody(daemon, &encrypted)?;
    if c.expires_at <= clock(&db)? {
        bail!("native original consent expired")
    };
    Ok(json!({"grant_id":id,"local_custody_sha256":digest(&c)?,"confirmation":c.confirmation()?}))
}
pub(super) async fn observe(daemon: &Daemon, params: Value) -> Result<Value> {
    let id: Uuid = serde_json::from_value(params["grant_id"].clone())?;
    let status = &params["status"];
    let db = daemon.database.lock().await;
    db.execute_batch(SCHEMA)?;
    let encrypted: String = db.query_row(
        "SELECT encrypted_original FROM daemon_enterprise_grants WHERE grant_id=?1 AND stopped=0",
        [id.to_string()],
        |r| r.get(0),
    )?;
    let c = decode_custody(daemon, &encrypted)?;
    let sha = digest(&c)?;
    let expected = json!({"format":1,"id":id,"revision":c.pending_revision+1,"policy_sha256":c.policy_sha256,"local_custody_sha256":sha,"device_id":c.device_id,"state":"active"});
    if status["current_authorized"] != true
        || status["confirmation"] != expected
        || status["id"] != json!(id)
        || status["device_id"] != json!(c.device_id)
        || status["revision"] != json!(c.pending_revision + 1)
        || status["state"] != "active"
        || status["policy_sha256"] != json!(c.policy_sha256)
        || status["policy"] != serde_json::to_value(&c.policy)?
        || status["expires_at"] != c.original_grant["expires_at"]
        || status["chat_id"] != c.original_grant["chat_id"]
        || status["owner_user_id"] != c.original_grant["owner_user_id"]
        || c.expires_at <= clock(&db)?
    {
        bail!("current actual server confirmation differs from original native approval")
    };
    if let Some(old) = db.query_row(
        "SELECT confirmation FROM daemon_enterprise_grants WHERE grant_id=?1",
        [id.to_string()],
        |r| r.get::<_, Option<String>>(0),
    )? {
        if serde_json::from_str::<Value>(&old)? != expected {
            bail!("different original server confirmation")
        }
    }
    db.execute("UPDATE daemon_enterprise_grants SET confirmation=?1,last_seen_at=?2 WHERE grant_id=?3 AND stopped=0",params![expected.to_string(),clock(&db)?.to_rfc3339(),id.to_string()])?;
    Ok(expected)
}
pub(super) async fn stop(daemon: &Daemon, params: Value) -> Result<Value> {
    let id: Uuid = serde_json::from_value(params["grant_id"].clone())?;
    {
        let db = daemon.database.lock().await;
        db.execute_batch(SCHEMA)?;
        db.execute(
            "UPDATE daemon_enterprise_grants SET stopped=1 WHERE grant_id=?1",
            [id.to_string()],
        )?;
    }
    let runs = list_active_runs(daemon).await?;
    if let Some(runs) = runs["items"].as_array() {
        for run in runs {
            if run["spec"]["input"]["_enterprise_grant"]["grant_id"] == json!(id) {
                if let Ok(run) = serde_json::from_value::<RunRecord>(run.clone()) {
                    let _ = cancel_run(daemon, run.spec.id).await;
                }
            }
        }
    }
    Ok(
        json!({"grant_id":id,"local_stop_requested":true,"external_effects_undone":false,"replay_allowed":false}),
    )
}
pub(super) async fn custody(daemon: &Daemon, pin: &Pin, spec: &RunSpec) -> Result<Custody> {
    let db = daemon.database.lock().await;
    db.execute_batch(SCHEMA)?;
    let (encrypted,server,confirmed,seen):(String,String,Option<String>,Option<String>)=db.query_row("SELECT encrypted_original,server_url,confirmation,last_seen_at FROM daemon_enterprise_grants WHERE grant_id=?1 AND stopped=0",[pin.grant_id.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let c = decode_custody(daemon, &encrypted)?;
    c.validate(&server, daemon.config.device_id, pin)?;
    let now = clock(&db)?;
    if confirmed.is_none() || c.expires_at <= now {
        bail!("original local approval has no actual server confirmation")
    };
    let fresh = seen
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .is_some_and(|seen| {
            now.signed_duration_since(seen.with_timezone(&Utc))
                .num_seconds()
                <= 15
        });
    if !fresh && !crate::enterprise_offline::authorized(daemon, &db, spec, pin, now)? {
        bail!("current server grant unavailable and no original bounded offline permit");
    }
    Ok(c)
}
pub(super) async fn admission(
    daemon: &Daemon,
    spec: &RunSpec,
    workspace: Option<&Path>,
) -> Result<Option<Custody>> {
    let Some(pin) = Pin::from_input(&spec.input)? else {
        return Ok(None);
    };
    let c = custody(daemon, &pin, spec).await?;
    if spec
        .required_capabilities
        .iter()
        .any(|cap| !c.policy.capabilities.contains(&cap.0))
    {
        bail!("run requests capabilities outside original native approval")
    }
    if c.original_grant["chat_id"] != json!(spec.thread_id)
        || c.original_grant["owner_user_id"] != json!(spec.creator_user_id)
        || spec.executor_target
            != (ExecutorTarget::PersonalDevice {
                device_id: daemon.config.device_id,
            })
    {
        bail!("native original policy belongs to another chat, actor or executor")
    }
    if matches!(c.policy.access, LaptopAccess::Selected) {
        let path = workspace
            .context("selected mode requires a locally known actual workspace")?
            .canonicalize()?;
        if !c.roots.values().any(|root| path.starts_with(root)) {
            bail!("actual local workspace is outside the original native selected roots")
        }
    }
    if matches!(c.policy.access, LaptopAccess::Sandbox) {
        let path = known_root(workspace.context("isolated native sandbox workspace missing")?)?;
        let expected = daemon
            .config
            .database_path
            .parent()
            .context("native data parent missing")?
            .join("enterprise-sandboxes")
            .join(spec.id.to_string())
            .canonicalize()?;
        if path != expected {
            bail!("sandbox must use its original isolated native workspace");
        }
        let q: crate::enterprise_sandbox::Qualification =
            serde_json::from_value(c.sandbox.clone().context("sandbox qualification missing")?)?;
        q.current().await?;
    }
    Ok(Some(c))
}
pub(super) async fn sandbox_workspace(daemon: &Daemon, params: Value) -> Result<Value> {
    let spec: RunSpec = serde_json::from_value(params["run_spec"].clone())?;
    let pin = Pin::from_input(&spec.input)?.context("original native pin missing")?;
    let c = custody(daemon, &pin, &spec).await?;
    if !matches!(c.policy.access, LaptopAccess::Sandbox) {
        bail!("original mode is not sandbox")
    };
    let path = daemon
        .config
        .database_path
        .parent()
        .context("native data parent missing")?
        .join("enterprise-sandboxes")
        .join(spec.id.to_string());
    create_private_dir(&path)?;
    let path = known_root(&path)?;
    Ok(json!({"workspace_path":path}))
}
pub(super) async fn run_admit(daemon: &Daemon, params: Value) -> Result<Value> {
    let spec: RunSpec = serde_json::from_value(params["run_spec"].clone())?;
    let workspace = if let Some(path) = params["workspace_path"].as_str() {
        Some(known_root(Path::new(path))?)
    } else {
        project_workspace(daemon, spec.project_id).await?
    };
    let c = admission(daemon, &spec, workspace.as_deref()).await?;
    if params["materialize"] == true {
        if let Some(c) = &c {
            if matches!(c.policy.access, LaptopAccess::Selected)
                && !workspace.as_ref().is_some_and(|p| {
                    c.policy
                        .write_roots
                        .iter()
                        .filter_map(|a| c.roots.get(a))
                        .any(|r| p.starts_with(r))
                })
            {
                bail!("snapshot materialization exceeds original native write custody");
            }
        }
    }
    Ok(
        json!({"admitted":c.is_some(),"mode":c.as_ref().map(|c|c.policy.access),"local_custody_sha256":c.as_ref().map(digest).transpose()?}),
    )
}
pub(super) async fn watch(daemon: &Daemon, spec: &RunSpec) -> anyhow::Error {
    let Some(pin) = Pin::from_input(&spec.input).ok().flatten() else {
        return anyhow::anyhow!("enterprise original pin missing");
    };
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Err(error) = custody(daemon, &pin, spec).await {
            return error;
        }
        if local_run_is_canceled(daemon, spec.id).await.unwrap_or(true) {
            return anyhow::anyhow!("native original enterprise run canceled");
        }
    }
}
pub(super) fn allowed(c: &Custody, name: &str, capability: Option<&Capability>) -> bool {
    if capability.is_some_and(|cap| !c.policy.capabilities.contains(&cap.0)) {
        return false;
    }
    match c.policy.access {
        LaptopAccess::None => false,
        LaptopAccess::Full => true,
        LaptopAccess::Selected => matches!(
            name,
            "Think"
                | "AskUser"
                | "Read"
                | "Glob"
                | "Grep"
                | "ListDirectory"
                | "FileStat"
                | "Write"
                | "Append"
                | "Edit"
                | "MultiEdit"
                | "CreateDirectory"
                | "MovePath"
                | "CopyPath"
                | "DeleteFile"
                | "OfficeInspect"
                | "OfficeReplaceText"
        ),
        LaptopAccess::Sandbox => matches!(
            name,
            "Think"
                | "AskUser"
                | "Read"
                | "Glob"
                | "Grep"
                | "ListDirectory"
                | "FileStat"
                | "Write"
                | "Append"
                | "Edit"
                | "MultiEdit"
                | "CreateDirectory"
                | "MovePath"
                | "CopyPath"
                | "DeleteFile"
                | "Bash"
        ),
    }
}
pub(super) fn paths(c: &Custody, root: &Path, invocation: &ToolInvocation) -> Result<()> {
    if !matches!(c.policy.access, LaptopAccess::Selected) {
        return Ok(());
    };
    let writes = matches!(
        invocation.name.as_str(),
        "Write"
            | "Append"
            | "Edit"
            | "MultiEdit"
            | "CreateDirectory"
            | "MovePath"
            | "DeleteFile"
            | "OfficeReplaceText"
    );
    let arguments = invocation
        .arguments
        .as_object()
        .context("tool arguments must be object")?;
    for (key, value) in arguments {
        if matches!(
            key.as_str(),
            "path" | "source" | "destination" | "output_path"
        ) {
            let value = value
                .as_str()
                .context("native path argument must be string")?;
            let candidate = safe_workspace_path_no_symlinks(root, value, false)?;
            let write = writes || key == "destination" || key == "output_path";
            let aliases = if write {
                &c.policy.write_roots
            } else {
                &c.policy.read_roots
            };
            if !aliases
                .iter()
                .filter_map(|alias| c.roots.get(alias))
                .any(|path| candidate.starts_with(path))
            {
                bail!("tool path exceeds original native read/write roots")
            }
        }
    }
    if matches!(invocation.name.as_str(), "Glob" | "Grep" | "ListDirectory")
        && !c
            .policy
            .read_roots
            .iter()
            .filter_map(|a| c.roots.get(a))
            .any(|path| root.starts_with(path))
    {
        bail!("native recursive read root is outside original read custody")
    };
    Ok(())
}
