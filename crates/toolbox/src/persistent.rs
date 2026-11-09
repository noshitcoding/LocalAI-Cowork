use super::*;
use std::time::Instant;

/// Process lifetime remains under the existing job/process-group supervisor.
/// Call cancellation kills and joins the context, so uncertain calls cannot leak.
pub(crate) struct ManagedService {
    pub(crate) directory: PathBuf,
    cancel: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    gate: Mutex<()>,
}
impl ManagedService {
    pub(crate) fn start(
        directory: PathBuf,
        launch: cowork_executor::TrustedLaunch,
        cancelled: &AtomicBool,
    ) -> Result<Arc<Self>> {
        let cancel = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let (stopped, done) = (cancel.clone(), finished.clone());
        let worker = std::thread::Builder::new()
            .name("persistent-tool-supervisor".into())
            .spawn(move || {
                let _ = cowork_executor::supervise(&launch, &stopped);
                done.store(true, Ordering::Release);
            })?;
        let service = Arc::new(Self {
            directory,
            cancel,
            finished,
            worker: Mutex::new(Some(worker)),
            gate: Mutex::new(()),
        });
        let deadline = Instant::now() + Duration::from_secs(55);
        while !service.directory.join("ready.json").is_file() {
            if cancelled.load(Ordering::Acquire)
                || service.finished.load(Ordering::Acquire)
                || Instant::now() >= deadline
            {
                service.stop();
                return Err(invalid(
                    "Persistent tool could not start; install its Playwright/stdio dependencies",
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Ok(service)
    }
    pub(crate) fn stop(&self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = worker.join();
        }
    }
    pub(crate) fn call(
        &self,
        body: serde_json::Value,
        timeout: u64,
        cancelled: &AtomicBool,
    ) -> Result<serde_json::Value> {
        let _gate = self.gate.lock().map_err(|_| Error::Conflict)?;
        if cancelled.load(Ordering::Acquire) {
            self.stop();
            return Err(invalid("Persistent call cancelled before dispatch"));
        }
        if self.cancel.load(Ordering::Acquire) || self.finished.load(Ordering::Acquire) {
            return Err(invalid(
                "Persistent context ended; create a new explicit session",
            ));
        }
        let id = Uuid::new_v4();
        let request = self.directory.join(format!("{id}.request.json"));
        let response = self.directory.join(format!("{id}.response.json"));
        let raw = serde_json::to_vec(&serde_json::json!({"body":body,"timeout_seconds":timeout}))?;
        if raw.len() > 2 * 1024 * 1024 {
            return Err(invalid("Oversized persistent request"));
        }
        let temporary = self.directory.join(format!("{id}.input"));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&raw)?;
        file.sync_all()?;
        drop(file);
        fs::rename(temporary, request)?;
        let deadline = Instant::now() + Duration::from_secs(timeout);
        loop {
            if cancelled.load(Ordering::Acquire)
                || self.finished.load(Ordering::Acquire)
                || Instant::now() >= deadline
            {
                self.stop();
                return Err(invalid(
                    "Persistent tool outcome uncertain; the process tree was stopped and joined",
                ));
            }
            if let Ok(metadata) = fs::symlink_metadata(&response) {
                if metadata.is_symlink() || !metadata.is_file() || metadata.len() > 2 * 1024 * 1024
                {
                    self.stop();
                    return Err(invalid("Invalid persistent tool reply"));
                }
                let value: serde_json::Value = serde_json::from_slice(&fs::read(&response)?)?;
                fs::remove_file(response)?;
                if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
                    self.stop();
                    return Err(invalid(error));
                }
                return Ok(value["result"].clone());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}
impl Drop for ManagedService {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Explicit working directory and environment for a persistent stdio server.
pub struct StdioOptions<'a> {
    pub cwd: Option<&'a str>,
    pub environment: &'a BTreeMap<String, String>,
    pub native_cwd: bool,
}

impl Toolbox {
    fn managed_service(
        &self,
        id: &str,
        config: serde_json::Value,
        mut launch: cowork_executor::TrustedLaunch,
        cancelled: &AtomicBool,
    ) -> Result<Arc<ManagedService>> {
        let mut services = self.managed.lock().map_err(|_| Error::Conflict)?;
        services.retain(|_, service| !service.finished.load(Ordering::Acquire));
        if let Some(service) = services.get(id) {
            return Ok(service.clone());
        }
        if services.len() >= 64 {
            return Err(invalid("Close unused persistent sessions first"));
        }
        let directory = self
            .config
            .root
            .join("sessions")
            .join(id)
            .join(Uuid::new_v4().to_string());
        private_dir(&directory)?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.join("config.json"))?;
        file.write_all(&serde_json::to_vec(&config)?)?;
        file.sync_all()?;
        launch.arguments = if config["mode"] == "browser" && config["playwright_module"].is_string()
        {
            vec![
                self.adapters
                    .join("persistent_browser.cjs")
                    .to_string_lossy()
                    .into(),
                directory.to_string_lossy().into(),
            ]
        } else {
            vec![
                "-I".into(),
                self.adapters.join("persistent.py").to_string_lossy().into(),
                directory.to_string_lossy().into(),
            ]
        };
        launch.timeout = Duration::from_secs(24 * 60 * 60);
        launch.capture_bytes_per_stream = 16384;
        let service = ManagedService::start(directory, launch, cancelled)?;
        services.insert(id.into(), service.clone());
        Ok(service)
    }
    pub(crate) fn persistent_browser(
        &self,
        order: &str,
        request: &Request,
        launch: cowork_executor::TrustedLaunch,
        cancel: &AtomicBool,
    ) -> Result<cowork_executor::Outcome> {
        let Action::Browser { session_id, .. } = &request.action else {
            return Err(Error::Conflict);
        };
        let started = Instant::now();
        let programs = self.programs.lock().map_err(|_| Error::Conflict)?.clone();
        if !(programs.contains_key("python")
            || programs.contains_key("node") && programs.contains_key("playwright"))
        {
            return Err(invalid(
                "Configure Python Playwright or Node with its explicit Playwright module",
            ));
        }
        let config = serde_json::json!({"mode":"browser","epoch":Uuid::new_v4(),"session_id":session_id,
            "profile":self.config.root.join("sessions").join(session_id).join("profile"),"workspace":self.workspace(order)?,
            "executable":programs.get("chromium").map(|p|p.to_string_lossy().into_owned()),
            "playwright_module":if programs.contains_key("node"){programs.get("playwright").map(|p|p.to_string_lossy().into_owned())}else{None}});
        let service = self.managed_service(session_id, config, launch, cancel)?;
        let value = service.call(
            serde_json::to_value(&request.action)?,
            request.timeout_seconds,
            cancel,
        )?;
        if let Some(url) = value["url"].as_str()
            && let Some(session) = self
                .sessions
                .lock()
                .map_err(|_| Error::Conflict)?
                .get_mut(session_id)
        {
            session.url = Some(url.into());
        }
        let mut bytes = serde_json::to_vec(&value)?;
        let total = bytes.len() as u64;
        bytes.truncate(request.output_bytes);
        Ok(cowork_executor::Outcome {
            termination: cowork_executor::Termination::Exited,
            process_id: None,
            exit_code: Some(0),
            signal: None,
            stdout: cowork_executor::Capture {
                truncated: total > bytes.len() as u64,
                total_bytes: total,
                bytes,
                invalid_utf8: false,
            },
            stderr: Default::default(),
            duration_ms: started.elapsed().as_millis() as u64,
            output_complete: total <= request.output_bytes as u64,
        })
    }
    pub fn start_stdio(
        &self,
        id: &str,
        order: &str,
        program: &str,
        arguments: Vec<String>,
        cancelled: &AtomicBool,
    ) -> Result<()> {
        self.start_stdio_with_options(
            id,
            order,
            program,
            arguments,
            StdioOptions {
                cwd: None,
                environment: &BTreeMap::new(),
                native_cwd: false,
            },
            cancelled,
        )
    }
    /// Explicit environment values reach the adapter through opaque inherited slots,
    /// never its persisted configuration or command line.
    pub fn start_stdio_with_options(
        &self,
        id: &str,
        order: &str,
        program: &str,
        arguments: Vec<String>,
        options: StdioOptions<'_>,
        cancelled: &AtomicBool,
    ) -> Result<()> {
        let StdioOptions {
            cwd,
            environment,
            native_cwd,
        } = options;
        identifier(id)?;
        identifier(order)?;
        validate_stdio_environment(environment)?;
        if self.closing.load(Ordering::Acquire) {
            return Err(Error::Conflict);
        }
        let (config, launch) =
            self.prepare_stdio_launch(order, program, arguments, cwd, environment, native_cwd)?;
        self.managed_service(id, config, launch, cancelled)?;
        Ok(())
    }
    fn prepare_stdio_launch(
        &self,
        order: &str,
        program: &str,
        arguments: Vec<String>,
        cwd: Option<&str>,
        environment: &BTreeMap<String, String>,
        native_cwd: bool,
    ) -> Result<(serde_json::Value, cowork_executor::TrustedLaunch)> {
        let request = Request {
            idempotency_key: Uuid::new_v4().to_string(),
            action: Action::Terminal {
                program: program.into(),
                arguments: arguments.clone(),
                cwd: String::new(),
            },
            timeout_seconds: 120,
            output_bytes: 65536,
            approval_sha256: None,
        };
        let mut target = self.launch(order, &request)?;
        target.directory = self.stdio_directory(order, cwd, native_cwd)?;
        let python = self
            .programs
            .lock()
            .map_err(|_| Error::Conflict)?
            .get("python")
            .cloned()
            .ok_or_else(|| invalid("Stdio MCP requires the configured Python adapter"))?;
        let base_environment = target.environment.clone();
        let slots = environment
            .iter()
            .map(|(name, value)| {
                let slot = format!("COWORK_MCP_VALUE_{}", Uuid::new_v4().simple());
                target.environment.insert(slot.clone(), value.clone());
                (name.clone(), slot)
            })
            .collect::<BTreeMap<_, _>>();
        let config = serde_json::json!({"mode":"mcp","program":target.program,"arguments":arguments,"environment":base_environment,"environment_slots":slots,"workspace":target.directory});
        let mut launch = target;
        launch.program = python;
        Ok((config, launch))
    }
    pub fn stdio_directory(
        &self,
        order: &str,
        cwd: Option<&str>,
        native_cwd: bool,
    ) -> Result<PathBuf> {
        let workspace = self.workspace(order)?;
        let requested = cwd.unwrap_or(".");
        if requested.len() > 4096 || requested.chars().any(char::is_control) {
            return Err(invalid("Invalid MCP working directory"));
        }
        let path = Path::new(requested);
        let resolved = if native_cwd && path.is_absolute() {
            fs::canonicalize(path)?
        } else {
            fs::canonicalize(workspace.join(relative(requested)?))?
        };
        if !resolved.is_dir() || (!native_cwd && !resolved.starts_with(&workspace)) {
            return Err(invalid(
                "MCP working directory must stay inside its managed workspace",
            ));
        }
        Ok(resolved)
    }
    pub fn stdio_rpc(
        &self,
        id: &str,
        body: serde_json::Value,
        cancel: &AtomicBool,
    ) -> Result<serde_json::Value> {
        identifier(id)?;
        let service = self
            .managed
            .lock()
            .map_err(|_| Error::Conflict)?
            .get(id)
            .cloned()
            .ok_or(Error::NotFound)?;
        service.call(body, 120, cancel)
    }
    pub fn stop_persistent(&self, id: &str) -> Result<()> {
        if let Some(service) = self.managed.lock().map_err(|_| Error::Conflict)?.remove(id) {
            service.stop();
        }
        Ok(())
    }
    pub fn persistent_alive(&self, id: &str) -> bool {
        self.managed
            .lock()
            .map(|services| {
                services.get(id).is_some_and(|s| {
                    !s.cancel.load(Ordering::Acquire) && !s.finished.load(Ordering::Acquire)
                })
            })
            .unwrap_or(false)
    }
}
pub fn validate_stdio_environment(environment: &BTreeMap<String, String>) -> Result<()> {
    let mut names = std::collections::BTreeSet::new();
    if environment.len() > 32
        || environment
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            > 8192
    {
        return Err(invalid("MCP environment exceeds its limit"));
    }
    for (name, value) in environment {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .enumerate()
                .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
            || value.len() > 2000
            || value.contains('\0')
            || !names.insert(name.to_ascii_uppercase())
        {
            return Err(invalid("Invalid MCP environment variable"));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stdio_environment_rejects_ambiguous_names_and_oversized_values() {
        for environment in [
            BTreeMap::from([("A=B".into(), "value".into())]),
            BTreeMap::from([("PATH".into(), "one".into()), ("Path".into(), "two".into())]),
            BTreeMap::from([("KEY".into(), "x".repeat(2001))]),
            BTreeMap::from([("KEY".into(), "bad\0value".into())]),
        ] {
            assert!(validate_stdio_environment(&environment).is_err());
        }
        assert!(
            validate_stdio_environment(&BTreeMap::from([(
                "API_KEY".into(),
                "private value with spaces".into()
            )]))
            .is_ok()
        );
    }
    #[test]
    fn stdio_cwd_is_confined_on_server_and_values_never_enter_saved_adapter_config() {
        let root = std::env::temp_dir().join(format!("cowork-mcp-test-{}", Uuid::new_v4()));
        let executable = std::env::current_exe().unwrap();
        let toolbox = Toolbox::open(Config {
            root: root.clone(),
            programs: BTreeMap::from([
                ("python".into(), executable.clone()),
                ("node".into(), executable),
            ]),
            environment: BTreeMap::new(),
            github_token: None,
        })
        .unwrap();
        let order = Uuid::new_v4().to_string();
        let workspace = toolbox.workspace(&order).unwrap();
        fs::create_dir(workspace.join("child")).unwrap();
        assert_eq!(
            toolbox
                .stdio_directory(&order, Some("child"), false)
                .unwrap(),
            workspace.join("child")
        );
        assert!(toolbox.stdio_directory(&order, Some("../"), false).is_err());
        assert!(
            toolbox
                .stdio_directory(&order, Some(root.to_str().unwrap()), false)
                .is_err()
        );
        assert_eq!(
            toolbox
                .stdio_directory(&order, Some(root.to_str().unwrap()), true)
                .unwrap(),
            fs::canonicalize(&root).unwrap()
        );
        let (config, launch) = toolbox
            .prepare_stdio_launch(
                &order,
                "node",
                vec![],
                Some("child"),
                &BTreeMap::from([("API_KEY".into(), "private-marker-123".into())]),
                false,
            )
            .unwrap();
        assert!(!config.to_string().contains("private-marker-123"));
        let slot = config["environment_slots"]["API_KEY"].as_str().unwrap();
        assert_eq!(launch.environment.get(slot).unwrap(), "private-marker-123");
        assert!(!launch.environment.contains_key("API_KEY"));
        assert_eq!(launch.directory, workspace.join("child"));
        drop(toolbox);
        fs::remove_dir_all(root).unwrap();
    }
}
