//! In-memory ownership of newly supervised helpers. Restart cannot adopt a helper.
use super::*;
pub(super) struct Process {
    pub run: Uuid,
    pub directory: PathBuf,
    cancel: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    pub outcome: Arc<std::sync::Mutex<Option<String>>>,
    capture: Arc<std::sync::Mutex<Option<cowork_executor::Outcome>>>,
}
#[derive(Default)]
pub(super) struct Registry {
    processes: std::sync::Mutex<BTreeMap<String, Process>>,
}
impl Registry {
    pub fn launch(
        &self,
        id: &str,
        run: Uuid,
        directory: PathBuf,
        launch: cowork_executor::TrustedLaunch,
    ) -> Result<()> {
        let mut map = self
            .processes
            .lock()
            .map_err(|_| anyhow::anyhow!("device_interop_process_lock"))?;
        ensure!(
            !map.contains_key(id),
            "device_interop_process_already_owned"
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let child_cancel = cancel.clone();
        let outcome = Arc::new(std::sync::Mutex::new(None));
        let observed = outcome.clone();
        let capture = Arc::new(std::sync::Mutex::new(None));
        let captured = capture.clone();
        let worker = std::thread::Builder::new()
            .name(format!("device-interop-{id}"))
            .spawn(move || {
                let result = cowork_executor::supervise(&launch, &child_cancel);
                let code = match result {
                    Ok(value) => {
                        let code = format!(
                            "{:?}:{}:{}",
                            value.termination,
                            value.exit_code.unwrap_or(-1),
                            value.output_complete
                        );
                        if let Ok(mut slot) = captured.lock() {
                            *slot = Some(value);
                        }
                        code
                    }
                    Err(_) => "owned_supervision_uncertain".into(),
                };
                if let Ok(mut state) = observed.lock() {
                    *state = Some(code);
                }
            })?;
        map.insert(
            id.into(),
            Process {
                run,
                directory,
                cancel,
                worker: Some(worker),
                outcome,
                capture,
            },
        );
        Ok(())
    }
    pub fn finished(&self, id: &str, run: Uuid) -> Result<bool> {
        let map = self
            .processes
            .lock()
            .map_err(|_| anyhow::anyhow!("device_interop_process_lock"))?;
        let p = map
            .get(id)
            .filter(|p| p.run == run)
            .context("device_interop_foreign_or_unowned_process")?;
        let value = p
            .outcome
            .lock()
            .map_err(|_| anyhow::anyhow!("device_interop_outcome_lock"))?
            .is_some();
        Ok(value)
    }
    pub fn take_capture(&self, id: &str, run: Uuid) -> Result<cowork_executor::Outcome> {
        let map = self
            .processes
            .lock()
            .map_err(|_| anyhow::anyhow!("device_interop_process_lock"))?;
        let p = map
            .get(id)
            .filter(|p| p.run == run)
            .context("device_interop_foreign_or_unowned_process")?;
        let value = p
            .capture
            .lock()
            .map_err(|_| anyhow::anyhow!("device_interop_capture_lock"))?
            .take()
            .context("device_interop_original_capture_unavailable")?;
        Ok(value)
    }
    pub fn directory(&self, id: &str, run: Uuid) -> Result<PathBuf> {
        let map = self
            .processes
            .lock()
            .map_err(|_| anyhow::anyhow!("device_interop_process_lock"))?;
        let p = map
            .get(id)
            .filter(|p| p.run == run)
            .context("device_interop_foreign_or_unowned_session")?;
        ensure!(
            p.outcome
                .lock()
                .map_err(|_| anyhow::anyhow!("device_interop_outcome_lock"))?
                .is_none(),
            "device_interop_original_process_closed"
        );
        Ok(p.directory.clone())
    }
    pub async fn wait_closed(&self, id: &str, run: Uuid, stop: &CancellationToken) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let observed = {
                let map = self
                    .processes
                    .lock()
                    .map_err(|_| anyhow::anyhow!("device_interop_process_lock"))?;
                let p = map
                    .get(id)
                    .filter(|p| p.run == run)
                    .context("device_interop_foreign_or_unowned_process")?;

                p.outcome
                    .lock()
                    .map_err(|_| anyhow::anyhow!("device_interop_outcome_lock"))?
                    .clone()
            };
            if let Some(code) = observed {
                ensure!(
                    code == "Exited:0:true",
                    "device_interop_original_process_quiescence_unknown"
                );
                return Ok(());
            }
            ensure!(
                !stop.is_cancelled() && tokio::time::Instant::now() < deadline,
                "device_interop_original_close_unknown"
            );
            tokio::select! {_=stop.cancelled()=>bail!("device_run_canceled"),_=tokio::time::sleep(Duration::from_millis(25))=>{}}
        }
    }
    pub fn stop_run(&self, run: Option<Uuid>) {
        let mut owned = vec![];
        if let Ok(mut map) = self.processes.lock() {
            let ids = map
                .iter()
                .filter(|(_, p)| run.is_none_or(|r| p.run == r))
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            for id in ids {
                if let Some(p) = map.remove(&id) {
                    p.cancel.store(true, Ordering::Release);
                    owned.push(p);
                }
            }
        }
        for mut p in owned {
            if let Some(worker) = p.worker.take() {
                let _ = worker.join();
            }
        }
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.stop_run(None);
    }
}
pub(super) struct RunGuard<'a> {
    pub registry: &'a Registry,
    pub run: Uuid,
}
impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.registry.stop_run(Some(self.run));
    }
}
pub(super) async fn wait_json(
    path: &Path,
    stop: &CancellationToken,
    seconds: u64,
) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        ensure!(
            !stop.is_cancelled() && tokio::time::Instant::now() < deadline,
            "device_interop_original_wait_interrupted_no_replay"
        );
        if path.exists() {
            crate::config::ordinary(path, false)?;
            ensure!(
                fs::metadata(path)?.len() <= 4 * 1024 * 1024,
                "device_interop_response_bound"
            );
            return Ok(serde_json::from_slice(&fs::read(path)?)?);
        }
        tokio::select! {_=stop.cancelled()=>bail!("device_run_canceled"),_=tokio::time::sleep(Duration::from_millis(30))=>{}}
    }
}
