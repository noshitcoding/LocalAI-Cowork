//! Immutable original transport/effect records under the device's private state.
use super::*;
pub(super) fn write(root: &Path, name: &str, value: &Value) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 200
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
        "device_receipt_name_invalid"
    );
    private_directory(root)?;
    let raw = serde_json::to_vec(value)?;
    ensure!(
        raw.len() <= 4 * 1024 * 1024,
        "device_original_receipt_too_large"
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let temporary = root.join(format!(".{}-{}.tmp", name, Uuid::new_v4()));
    let mut file = options.open(&temporary)?;
    file.write_all(&raw)?;
    file.sync_all()?;
    drop(file);
    let published = fs::hard_link(&temporary, root.join(name));
    let _ = fs::remove_file(&temporary);
    published?;
    #[cfg(unix)]
    {
        fs::File::open(root)?.sync_all()?;
    }
    Ok(())
}
pub(super) fn helper(root: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    private_directory(root)?;
    let path = root.join(format!("{}-{name}", digest(bytes)));
    match OpenOptions::new().create_new(true).write(true).open(&path) {
        Ok(mut file) => {
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            crate::config::ordinary(&path, false)?;
            ensure!(fs::read(&path)? == bytes, "device_embedded_adapter_changed");
        }
        Err(e) => return Err(e.into()),
    }
    Ok(path)
}
pub(super) async fn supervise(
    engine: &ExecutionEngine,
    run: Uuid,
    program: PathBuf,
    arguments: Vec<String>,
    directory: PathBuf,
    timeout: u64,
    stop: &CancellationToken,
) -> Result<Value> {
    let launch = cowork_executor::TrustedLaunch {
        program,
        arguments,
        directory,
        environment: engine.config.environment.clone(),
        timeout: Duration::from_secs(timeout),
        capture_bytes_per_stream: 4 * 1024 * 1024,
    };
    let id = format!("oneshot-{}", Uuid::new_v4());
    engine
        .processes
        .launch(&id, run, launch.directory.clone(), launch)?;
    // The run registry owns the actual supervisor thread even if this future
    // is canceled. RunGuard and agent shutdown cancel and join that thread.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout + 30);
    loop {
        let ready = engine.processes.finished(&id, run)?;
        if ready {
            break;
        }
        ensure!(
            !stop.is_cancelled() && tokio::time::Instant::now() < deadline,
            "device_interop_original_wait_interrupted_no_replay"
        );
        tokio::select! {_=stop.cancelled()=>bail!("device_run_canceled"),_=tokio::time::sleep(Duration::from_millis(25))=>{}}
    }
    let result = engine.processes.take_capture(&id, run)?;
    ensure!(
        !stop.is_cancelled()
            && result.termination == cowork_executor::Termination::Exited
            && result.exit_code == Some(0)
            && result.output_complete
            && !result.stdout.truncated,
        "device_interop_effect_unknown_no_replay"
    );
    let value: Value = serde_json::from_slice(&result.stdout.bytes)
        .context("device_interop_original_response_invalid")?;
    ensure!(
        value["ok"] == true,
        "device_interop_original_operation_failed"
    );
    engine.admit(run,&format!("interop-received/{}",Uuid::new_v4()),json!({"format":"device-interop-original-response/1","response":value,"sha256":digest(&result.stdout.bytes),"process":result.process_id,"termination":result.termination}))?;
    Ok(value)
}
