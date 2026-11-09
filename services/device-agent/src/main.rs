//! Direct protocol-2 agent. This binary contains no V1 daemon or IPC client.
mod capability_profiles;
mod config;
mod desktop;
mod execution;
mod local_capabilities;
#[cfg(windows)]
mod private_secret_file;
mod transport;

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use config::Configuration;
use execution::{ExecutionEngine, ExecutionResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use transport::AgentClient;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Outbox {
    executor: Uuid,
    run: Uuid,
    lease: Uuid,
    success: bool,
    result: Value,
    snapshot: Option<Uuid>,
    diff: Value,
}
fn create_private(path: &Path) -> Result<File> {
    #[cfg(windows)]
    {
        Ok(private_secret_file::create_private_file(path)?)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        Ok(std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?)
    }
}
fn save(path: &Path, value: &Outbox) -> Result<()> {
    let mut file = create_private(path)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    #[cfg(unix)]
    File::open(path.parent().context("Missing journal parent")?)?.sync_all()?;
    Ok(())
}
fn new_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        #[cfg(windows)]
        private_secret_file::create_private_directory(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(path)?;
        }
    }
    config::private_directory(path)
}
fn status(
    root: &Path,
    instance: Uuid,
    executor: Uuid,
    state: &str,
    run: Option<Uuid>,
    connection: &ConnectionStatus,
) -> Result<()> {
    let temporary = root.join(format!(".agent-status-{}.tmp", Uuid::new_v4()));
    let mut file = create_private(&temporary)?;
    let now = Utc::now();
    let fresh = connection.last_heartbeat_at.is_some_and(|time| {
        let age = now.signed_duration_since(time).num_seconds();
        (0..=45).contains(&age)
    });
    let state = if matches!(state, "online" | "running") {
        if connection.error.is_some() {
            "network_error"
        } else if fresh {
            state
        } else {
            "connecting"
        }
    } else {
        state
    };
    file.write_all(&serde_json::to_vec(&json!({"format":1,"instance_id":instance,"executor_id":executor,"pid":std::process::id(),"state":state,"active_run_id":run,"updated_at":now.to_rfc3339(),"last_heartbeat_at":connection.last_heartbeat_at.map(|time|time.to_rfc3339()),"network_error":connection.error,"status_max_age_seconds":45}))?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temporary, root.join("agent-status.json"))?;
    Ok(())
}
#[derive(Default)]
struct ConnectionStatus {
    last_heartbeat_at: Option<DateTime<Utc>>,
    error: Option<&'static str>,
}
impl ConnectionStatus {
    fn accepted(&mut self) {
        self.last_heartbeat_at = Some(Utc::now());
        self.error = None;
    }
    fn failed(&mut self, error: &'static str) {
        self.error = Some(error);
    }
}
async fn flush(client: &AgentClient, directory: &Path) -> Result<()> {
    let mut count = 0;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("pending") {
            continue;
        }
        count += 1;
        ensure!(
            count <= 1000,
            "Too many pending completion receipts; review the local agent journal"
        );
        let value: Outbox = serde_json::from_reader(config::private_file(&path, 1024 * 1024)?)?;
        ensure!(
            value.executor == client.executor_id(),
            "Pending completion belongs to another paired executor"
        );
        let outcome = if value.success {
            client
                .complete(
                    value.run,
                    value.lease,
                    value.result,
                    value.snapshot,
                    value.diff,
                )
                .await
        } else {
            client.fail(value.run, value.lease, value.result).await
        };
        match outcome {
            Ok(_) => std::fs::rename(&path, path.with_extension("sent"))?,
            Err(e) if transport::authority_lost(&e) => {
                std::fs::rename(&path, path.with_extension("rejected"))?;
                eprintln!(
                    "Run {} completion rejected; receipt retained, execution will not be replayed",
                    value.run
                );
            }
            Err(_) => break,
        }
    }
    Ok(())
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                tokio::select! {_ = tokio::signal::ctrl_c()=>{},_ = signal.recv()=>{}}
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
fn identifier(v: &Value, key: &str) -> Result<Uuid> {
    v[key]
        .as_str()
        .context("Invalid server lease identifier")?
        .parse()
        .context("Invalid server lease UUID")
}
fn expiry(lease: &Value) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(
        lease["lease_expires_at"]
            .as_str()
            .context("Missing server lease expiry")?,
    )?
    .with_timezone(&Utc))
}
async fn run(configuration: Configuration) -> Result<()> {
    let root = configuration.workspace_root.clone();
    let lock_path = root.join("agent.lock");
    let lock = if lock_path.exists() {
        config::private_file(&lock_path, 4096)?
    } else {
        create_private(&lock_path)?
    };
    lock.try_lock()
        .context("An agent already owns this workspace; stop it before starting another")?;
    let outbox = root.join("transport-outbox");
    new_directory(&outbox)?;
    let engine = ExecutionEngine::open(configuration.execution()?)?;
    let availability_temporary = root.join(format!(".availability-{}.tmp", Uuid::new_v4()));
    let mut availability = create_private(&availability_temporary)?;
    availability.write_all(&serde_json::to_vec(&engine.availability())?)?;
    availability.sync_all()?;
    drop(availability);
    std::fs::rename(availability_temporary, root.join("agent-availability.json"))?;
    let client = AgentClient::new(
        &configuration.server_url,
        configuration.registration.executor_id,
        config::secret(&configuration.agent_token_file)?.to_string(),
    )?;
    let instance = Uuid::new_v4();
    let mut connection = ConnectionStatus::default();
    status(
        &root,
        instance,
        client.executor_id(),
        "connecting",
        None,
        &connection,
    )?;
    if let Err(error) = client.register(&configuration.registration).await {
        connection.failed(if transport::authority_lost(&error) {
            "registration_rejected"
        } else {
            "registration_unreachable"
        });
        status(
            &root,
            instance,
            client.executor_id(),
            "network_error",
            None,
            &connection,
        )?;
        return Err(error);
    }
    let stop = CancellationToken::new();
    let local_stop = stop.clone();
    let stop_file = root.join(format!(".stop-{instance}"));
    let stop_task = tokio::spawn(async move {
        loop {
            if config::private_file(&stop_file, 64).is_ok() {
                local_stop.cancel();
                break;
            }
            tokio::select! {_=local_stop.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(250))=>{}}
        }
    });
    let active = Arc::new(tokio::sync::Mutex::new(None));
    let desktop_stop = stop.child_token();
    let desktop_enabled = configuration
        .registration
        .capabilities
        .iter()
        .any(|c| c.name == "desktop.windows" || c.name == "desktop.linux");
    let desktop_task = if desktop_enabled {
        tokio::spawn(desktop::supervise(
            client.clone(),
            active.clone(),
            root.clone(),
            configuration.registration.kind == "personal_device",
            configuration
                .registration
                .personal_device_remote_control
                .clone()
                .unwrap_or_else(|| "off".into()),
            desktop_stop,
        ))
    } else {
        tokio::spawn(async { Ok(()) })
    };
    let signal_stop = stop.clone();
    let signal = tokio::spawn(async move {
        tokio::select! {_ = shutdown_signal()=>signal_stop.cancel(),_ = signal_stop.cancelled()=>{}}
    });
    let outcome=async {
        eprintln!("LocalAI Cowork V2 device agent ready ({}); Ctrl+C stops this agent",configuration.registration.executor_id);
        while !stop.is_cancelled() {
            status(&root, instance, client.executor_id(), "online", active.lock().await.as_ref().map(|(run,_,_)|*run), &connection)?;
            flush(&client,&outbox).await?;
            match client.heartbeat(&[]).await {
                Ok(_)=>{connection.accepted();status(&root, instance, client.executor_id(), "online", None, &connection)?;},
                Err(e) if transport::authority_lost(&e)=>{connection.failed("heartbeat_rejected");return Err(e);},
                Err(_)=>{connection.failed("heartbeat_unreachable");status(&root, instance, client.executor_id(), "network_error", None, &connection)?;tokio::select!{_ = stop.cancelled()=>break,_ = tokio::time::sleep(Duration::from_secs(5))=>{}};continue;}
            }
            let lease=match client.claim().await {
                Ok(v)=>v,
                Err(e) if transport::authority_lost(&e)=>{connection.failed("claim_rejected");return Err(e);},
                Err(_)=>{connection.failed("claim_unreachable");status(&root, instance, client.executor_id(), "network_error", None, &connection)?;tokio::select!{_ = stop.cancelled()=>break,_ = tokio::time::sleep(Duration::from_secs(3))=>{}};continue;}
            };
            if lease.is_null() {tokio::select!{_ = stop.cancelled()=>break,_ = tokio::time::sleep(Duration::from_secs(2))=>{}};continue;}
            let run=identifier(&lease["run"]["spec"],"id")?;
            let token=identifier(&lease,"lease_token")?;
            let deadline=Arc::new(tokio::sync::Mutex::new(expiry(&lease)?));
            let run_stop=stop.child_token();
            *active.lock().await=Some((run,token,run_stop.clone()));
            status(&root, instance, client.executor_id(), "running", Some(run), &connection)?;
            let task_engine=engine.clone();let task_client=client.clone();let task_stop=run_stop.clone();
            let mut task=tokio::spawn(async move{task_engine.execute(&task_client,lease,&task_stop).await});
            let watch_deadline=deadline.clone();let watch_stop=run_stop.clone();
            let watchdog=tokio::spawn(async move {
                loop {
                    if Utc::now()>=*watch_deadline.lock().await {watch_stop.cancel();break;}
                    tokio::select!{_ = watch_stop.cancelled()=>break,_ = tokio::time::sleep(Duration::from_millis(100))=>{}}
                }
            });
            let mut interval=tokio::time::interval(Duration::from_secs(5));
            let result=loop {
                tokio::select! {
                    result=&mut task=>break result.unwrap_or_else(|_|Err(anyhow::anyhow!("Device execution worker failed"))),
                    _=stop.cancelled()=>{run_stop.cancel();break task.await.unwrap_or_else(|_|Err(anyhow::anyhow!("Device execution worker failed during shutdown")));},
                    _=interval.tick()=>{
                        status(&root, instance, client.executor_id(), "running", Some(run), &connection)?;
                        if Utc::now()>=*deadline.lock().await {run_stop.cancel();}
                        else {
                            match client.lease_heartbeat(run,token).await {
                                Ok(v)=>match expiry(&v){Ok(v)=>*deadline.lock().await=v,Err(_)=>run_stop.cancel()},
                                Err(e) if transport::authority_lost(&e)=>run_stop.cancel(),
                                Err(_)=>{if Utc::now()>=*deadline.lock().await {run_stop.cancel();}},
                            }
                            match client.heartbeat(&[run]).await {Ok(_)=>connection.accepted(),Err(e)=>{connection.failed(if transport::authority_lost(&e){"heartbeat_rejected"}else{"heartbeat_unreachable"});if transport::authority_lost(&e){run_stop.cancel();}}}
                            status(&root, instance, client.executor_id(), "running", Some(run), &connection)?;
                        }
                        if run_stop.is_cancelled(){break task.await.unwrap_or_else(|_|Err(anyhow::anyhow!("Device execution worker failed during lease loss")));}
                    }
                }
            };
            run_stop.cancel();watchdog.await.context("Device lease watchdog failed")?;*active.lock().await=None;
            let receipt=match result {
                Ok(ExecutionResult{mut result,result_snapshot_manifest_id,result_diff_summary})=>{
                    if let Some(object)=result.as_object_mut(){
                        object.insert("result_snapshot_manifest_id".into(),json!(result_snapshot_manifest_id));
                        object.insert("result_diff_summary".into(),result_diff_summary.clone());
                    }
                    Outbox{executor:client.executor_id(),run,lease:token,success:true,result,snapshot:result_snapshot_manifest_id,diff:result_diff_summary}
                },
                Err(_)=>{eprintln!("Run {run} stopped or failed; inspect its private execution journal");Outbox{executor:client.executor_id(),run,lease:token,success:false,result:json!({"code":"device_operation_failed","message":"Device execution stopped or failed; local receipt retained","retryable":false}),snapshot:None,diff:json!({})}}
            };
            save(&outbox.join(format!("{run}.pending")),&receipt)?;
            flush(&client,&outbox).await?;
        }
        Ok(())
    }.await;
    stop.cancel();
    if let Some((_, _, run_stop)) = active.lock().await.take() {
        run_stop.cancel();
    }
    let desktop_result = desktop_task.await.context("Desktop supervisor failed");
    let signal_result = signal.await.context("Agent signal supervisor failed");
    engine.shutdown();
    stop_task.await.context("Local stop supervisor failed")?;
    status(
        &root,
        instance,
        client.executor_id(),
        "stopped",
        None,
        &connection,
    )?;
    drop(lock);
    desktop_result??;
    signal_result?;
    outcome
}
#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args == ["--version"] {
        println!(
            "cowork-device-agent-v2 {} protocol 2",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if args == ["--features"] {
        println!(
            "{}",
            serde_json::to_string(
                &json!({"format":1,"protocol_version":2,"local_gui_control":true,"desktop_platform":if cfg!(windows){"windows"}else if cfg!(target_os="linux"){"linux_x11"}else{"unavailable"},"desktop_backends":if cfg!(windows){json!(["native_windows"])}else if cfg!(target_os="linux"){json!(["x11","wayland_portal"])}else{json!([])},"max_concurrent_runs":1})
            )?
        );
        return Ok(());
    }
    if args == ["--local-capabilities"] {
        println!(
            "{}",
            serde_json::to_string(&local_capabilities::describe(None))?
        );
        return Ok(());
    }
    if matches!(
        args.first().map(String::as_str),
        Some("mcp-journal-inspect" | "mcp-journal-confirm-key")
    ) {
        let (path, source) = match args.as_slice() {
            [command, path, run, source] if command == "mcp-journal-inspect" => {
                let run: Uuid = run.parse()?;
                let source: Uuid = source.parse()?;
                ensure!(
                    !run.is_nil() && !source.is_nil(),
                    "Original run and source UUIDs are required"
                );
                (PathBuf::from(path), Some((run, source)))
            }
            [command, path] if command == "mcp-journal-confirm-key" => (PathBuf::from(path), None),
            _ => anyhow::bail!(
                "Usage: mcp-journal-inspect CONFIG RUN_UUID SOURCE_UUID | mcp-journal-confirm-key CONFIG"
            ),
        };
        let configuration = Configuration::load(&path)?;
        let execution = configuration.execution()?;
        config::private_directory(&execution.data_directory)?;
        let value = match source {
            Some((run, source)) => {
                ExecutionEngine::mcp_journal_inspect(&execution.data_directory, run, source)?
            }
            None => ExecutionEngine::mcp_journal_confirm_key(&execution.data_directory)?,
        };
        println!("{}", serde_json::to_string(&value)?);
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("consent") {
        ensure!(
            args.len() == 7
                && args[1] == "--data"
                && args[3] == "--request"
                && args[5] == "--decision",
            "Usage: consent --data ABS --request UUID --decision approve|reject"
        );
        let root = PathBuf::from(&args[2]);
        config::private_directory(&root)?;
        let value = execution::resolve_local_consent(&root, args[4].parse()?, &args[6])?;
        println!("{}", serde_json::to_string(&value)?);
        return Ok(());
    }
    let (path, check, capabilities) = match args.as_slice() {
        [flag, path] if flag == "--config" => (PathBuf::from(path), false, false),
        [flag, path, check] if flag == "--config" && check == "--check" => {
            (PathBuf::from(path), true, false)
        }
        [flag, path, check] if flag == "--config" && check == "--local-capabilities" => {
            (PathBuf::from(path), false, true)
        }
        _ => anyhow::bail!(
            "Usage: cowork-device-agent-v2 --config ABS/agent.json [--check|--local-capabilities] | --version | --features | --local-capabilities | consent --data ABS --request UUID --decision approve|reject | mcp-journal-inspect CONFIG RUN_UUID SOURCE_UUID | mcp-journal-confirm-key CONFIG"
        ),
    };
    let configuration = Configuration::load(&path)?;
    if capabilities {
        let execution = configuration.execution()?;
        println!(
            "{}",
            serde_json::to_string(&local_capabilities::describe(Some(&execution)))?
        );
        return Ok(());
    }
    if check {
        let _ = configuration.execution()?;
        println!("V2 agent configuration valid; no network connection or execution started");
        return Ok(());
    }
    run(configuration).await
}
