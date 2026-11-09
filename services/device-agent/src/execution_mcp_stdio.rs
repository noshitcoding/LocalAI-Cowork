//! One newly supervised local process tree and its original pipes, no process adoption.
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub(super) struct Owned {
    pub endpoint: Option<Endpoint>,
    id: String,
    directory: PathBuf,
    nonce: Uuid,
    relay_stop: CancellationToken,
    relays: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Owned {
    fn drop(&mut self) {
        self.relay_stop.cancel();
        for r in &self.relays {
            r.abort();
        }
    }
}
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(!path.exists(), "device_mcp_original_pipe_packet_exists");
    let temporary = path.with_file_name(format!(".{}.tmp", Uuid::new_v4()));
    let mut f = crate::create_private(&temporary)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    ensure!(!path.exists(), "device_mcp_original_pipe_packet_exists");
    fs::rename(&temporary, path)?;
    #[cfg(unix)]
    fs::File::open(path.parent().context("device_mcp_pipe_parent")?)?.sync_all()?;
    Ok(())
}
impl Owned {
    pub(super) async fn open(
        engine: &ExecutionEngine,
        run: Uuid,
        source: Uuid,
        binding: &McpBinding,
        stop: &CancellationToken,
    ) -> Result<Self> {
        let python = engine
            .config
            .programs
            .get("python")
            .context("device_mcp_stdio_requires_configured_python")?
            .clone();
        let parent = engine.config.data_directory.join("mcp-bridge");
        crate::new_directory(&parent)?;
        let directory = parent.join(format!("{run}-{source}"));
        crate::new_directory(&directory)?;
        let helper = directory.join("original-bridge.py");
        write(&helper, include_bytes!("execution_mcp_bridge.py"))?;
        let nonce = Uuid::new_v4();
        let config = directory.join("original-config.json");
        write(
            &config,
            &serde_json::to_vec(
                &json!({"command":binding.command,"arguments":binding.arguments,"environment":binding.environment,"protocol_version":cowork_mcp::PROTOCOL_VERSION,"operation":{"mode":"serve","directory":directory,"nonce":nonce}}),
            )?,
        )?;
        let workspace = engine
            .toolbox
            .workspace(&run.to_string())
            .map_err(|_| anyhow::anyhow!("device_workspace_unavailable"))?;
        let id = format!("mcp-{source}");
        engine.processes.launch(
            &id,
            run,
            directory.clone(),
            cowork_executor::TrustedLaunch {
                program: python,
                arguments: vec![
                    "-I".into(),
                    helper.to_string_lossy().into_owned(),
                    config.to_string_lossy().into_owned(),
                ],
                directory: workspace,
                environment: engine.config.environment.clone(),
                timeout: Duration::from_secs(86400),
                capture_bytes_per_stream: 4096,
            },
        )?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            ensure!(
                !stop.is_cancelled() && tokio::time::Instant::now() < deadline,
                "device_mcp_original_process_start_unconfirmed"
            );
            if directory.join("ready.json").exists() {
                let ready =
                    super::super::process::wait_json(&directory.join("ready.json"), stop, 1)
                        .await?;
                ensure!(
                    ready == json!({"format":"device-mcp-pipe-ready/1","nonce":nonce}),
                    "device_mcp_original_pipe_changed"
                );
                break;
            }
            ensure!(
                !engine.processes.finished(&id, run)?,
                "device_mcp_original_process_exited"
            );
            tokio::select! {_=stop.cancelled()=>bail!("device_run_canceled"),_=tokio::time::sleep(Duration::from_millis(20))=>{}}
        }
        // The selected original child has read its private environment; do not
        // retain a second plaintext copy of operator credentials in the spool.
        fs::remove_file(config)?;
        let (sdk_read, mut relay_write) = tokio::io::duplex(65536);
        let (mut relay_read, sdk_write) = tokio::io::duplex(65536);
        let relay_stop = stop.child_token();
        let sender_stop = relay_stop.clone();
        let sender_dir = directory.clone();
        let sender = tokio::spawn(async move {
            let mut index = 0u32;
            loop {
                let mut bytes = Vec::new();
                loop {
                    let byte = tokio::select! {_=sender_stop.cancelled()=>return,r=relay_read.read_u8()=>match r{Ok(v)=>v,Err(_)=>return}};
                    if byte == b'\n' {
                        break;
                    }
                    if bytes.len() >= 4 * 1024 * 1024 {
                        return;
                    }
                    bytes.push(byte);
                }
                if bytes.is_empty() || index >= 8192 {
                    return;
                }
                let path = sender_dir.join(format!("{index:08}.in.json"));
                if write(&path, &bytes).is_err() {
                    return;
                }
                index += 1;
            }
        });
        let receiver_stop = relay_stop.clone();
        let receiver_dir = directory.clone();
        let receiver = tokio::spawn(async move {
            for index in 0u32..8192 {
                let path = receiver_dir.join(format!("{index:08}.out.json"));
                loop {
                    if receiver_stop.is_cancelled() {
                        return;
                    }
                    if path.exists() {
                        break;
                    }
                    if receiver_dir.join("closed.json").exists() {
                        return;
                    }
                    tokio::select! {_=receiver_stop.cancelled()=>return,_=tokio::time::sleep(Duration::from_millis(10))=>{}}
                }
                let bytes =
                    match crate::config::private_file(&path, 4 * 1024 * 1024).and_then(|mut f| {
                        let mut bytes = Vec::new();
                        std::io::Read::read_to_end(&mut f, &mut bytes)?;
                        Ok(bytes)
                    }) {
                        Ok(v) => v,
                        Err(_) => return,
                    };
                if relay_write.write_all(&bytes).await.is_err()
                    || relay_write.write_all(b"\n").await.is_err()
                    || relay_write.flush().await.is_err()
                {
                    return;
                }
                if fs::remove_file(&path).is_err() {
                    return;
                }
            }
        });
        let endpoint = Endpoint::isolated_stdio(nonce, sdk_read, sdk_write)
            .map_err(|_| anyhow::anyhow!("device_mcp_original_pipe_invalid"))?;
        Ok(Self {
            endpoint: Some(endpoint),
            id,
            directory,
            nonce,
            relay_stop,
            relays: vec![sender, receiver],
        })
    }
    pub(super) async fn close(&mut self, engine: &ExecutionEngine, run: Uuid) -> Result<Value> {
        let close = self.directory.join("close.json");
        if !close.exists() {
            write(
                &close,
                &serde_json::to_vec(
                    &json!({"format":"device-mcp-pipe-close/1","nonce":self.nonce}),
                )?,
            )?;
        }
        let cleanup = CancellationToken::new();
        engine
            .processes
            .wait_closed(&self.id, run, &cleanup)
            .await?;
        let observed =
            super::super::process::wait_json(&self.directory.join("closed.json"), &cleanup, 1)
                .await?;
        ensure!(
            observed["format"] == "device-mcp-pipe-closed/1"
                && observed["nonce"] == self.nonce.to_string()
                && observed["original_child_closed"] == true,
            "device_mcp_original_process_close_unconfirmed"
        );
        let outcome = engine.processes.take_capture(&self.id, run)?;
        ensure!(
            outcome.termination == cowork_executor::Termination::Exited
                && outcome.exit_code == Some(0)
                && outcome.output_complete
                && !outcome.stdout.truncated,
            "device_mcp_original_process_close_unconfirmed"
        );
        self.relay_stop.cancel();
        for r in self.relays.drain(..) {
            r.abort();
            let _ = r.await;
        }
        Ok(
            json!({"format":"device-mcp-source-closure/1","original_process":self.id,"nonce":self.nonce,"supervised_outcome":"Exited:0:complete","original_child":observed,"effects_unknown":true,"automatic_replay":false}),
        )
    }
}
