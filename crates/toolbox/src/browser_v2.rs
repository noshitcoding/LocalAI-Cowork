//! One browser context for the UI and admitted model actions. Profiles are local
//! to this toolbox/device; project identity is supplied by the trusted router.
use super::*;
use cowork_contracts::*;
use serde_json::{Value, json};

pub(crate) struct Session {
    pub view: BrowserSessionView,
    pub gate: Mutex<()>,
    _profile_lock: Option<fs::File>,
}
pub(crate) const OPERATIONS: &[&str] = &[
    "navigate",
    "dom",
    "screenshot",
    "click",
    "fill",
    "select",
    "scroll",
    "back",
    "forward",
    "reload",
    "tabs",
    "new_tab",
    "close_tab",
    "activate_tab",
    "frame",
    "pointer",
    "key",
    "resize",
    "annotate",
    "annotations",
    "clear_annotations",
    "console",
    "network",
    "network_details",
    "fetch_text",
    "search",
];
pub(crate) fn engine_name(engine: BrowserEngine) -> &'static str {
    match engine {
        BrowserEngine::Chrome => "chrome",
        BrowserEngine::Firefox => "firefox",
    }
}
pub fn validate_browser_command(command: &BrowserCommand) -> Result<()> {
    if !OPERATIONS.contains(&command.operation.as_str())
        || command
            .text
            .as_ref()
            .is_some_and(|v| v.len() > 32_000 || v.contains('\0'))
        || command
            .selector
            .as_ref()
            .is_some_and(|v| v.len() > 1000 || v.contains('\0'))
        || command
            .x
            .is_some_and(|v| !v.is_finite() || v.abs() > 100_000.)
        || command
            .y
            .is_some_and(|v| !v.is_finite() || v.abs() > 100_000.)
        || command.width.is_some_and(|v| !(320..=1920).contains(&v))
        || command.height.is_some_and(|v| !(240..=1080).contains(&v))
    {
        return Err(invalid("browser_invalid_command"));
    }
    if let Some(url) = &command.url {
        let parsed = url::Url::parse(url).map_err(|_| invalid("browser_http_url_required"))?;
        if url.len() > 8192
            || url.contains(['\r', '\n', '\0'])
            || !["http", "https"].contains(&parsed.scheme())
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(invalid("browser_http_url_required"));
        }
    }
    if ["click", "fill", "select", "annotate", "network_details"]
        .contains(&command.operation.as_str())
        && command.selector.as_ref().is_none_or(String::is_empty)
        || command.operation == "navigate" && command.url.is_none()
        || command.operation == "pointer" && (command.x.is_none() || command.y.is_none())
        || command.operation == "resize" && (command.width.is_none() || command.height.is_none())
        || command.operation == "key" && command.text.as_ref().is_none_or(String::is_empty)
        || command
            .tab_id
            .as_ref()
            .is_some_and(|v| v.len() > 100 || v.chars().any(char::is_control))
        || command
            .document_id
            .as_ref()
            .is_some_and(|v| v.len() > 100 || v.chars().any(char::is_control))
    {
        return Err(invalid("browser_missing_or_invalid_command_field"));
    }
    Ok(())
}
impl Toolbox {
    pub fn browser_connections(&self) -> Vec<BrowserExtensionConnection> {
        self.browser_bridge.connections()
    }
    pub fn start_browser_bridge(&self) -> Result<()> {
        self.browser_bridge.start()
    }
    pub fn create_browser_v2(
        &self,
        project: &str,
        order: &str,
        options: CreateBrowserSession,
    ) -> Result<BrowserSessionView> {
        if self.closing.load(Ordering::Acquire) {
            return Err(Error::Conflict);
        }
        identifier(project)?;
        identifier(order)?;
        let mut all = self.browser_v2.lock().map_err(|_| Error::Conflict)?;
        if all.len() >= 16 {
            return Err(invalid("browser_session_limit"));
        }
        let profile_lock = if options.mode == BrowserMode::Managed {
            if options.connection_id.is_some() {
                return Err(invalid("browser_invalid_connection"));
            }
            let programs = self.programs.lock().map_err(|_| Error::Conflict)?;
            if !programs.contains_key("python")
                || !programs.contains_key(engine_name(options.engine))
            {
                return Err(invalid("browser_install_runtime_first"));
            }
            let profile = self.browser_profile(project, options.engine)?;
            let lock_path = profile.join(".cowork-profile.lock");
            namespace(&lock_path)?;
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)?;
            lock.try_lock()
                .map_err(|_| invalid("browser_profile_busy"))?;
            Some(lock)
        } else {
            let id = options
                .connection_id
                .as_deref()
                .ok_or_else(|| invalid("browser_select_extension"))?;
            let connections = self.browser_connections();
            let connection = connections
                .iter()
                .find(|v| {
                    v.id == id && v.engine == options.engine && v.connected && !v.tabs.is_empty()
                })
                .ok_or(Error::NotFound)?;
            if all
                .values()
                .any(|s| s.view.connection_id.as_deref() == Some(&connection.id))
            {
                return Err(invalid("browser_connection_busy"));
            }
            None
        };
        let legacy = self.create_browser_session(order)?;
        let view = BrowserSessionView {
            id: legacy.id.clone(),
            project_id: project.into(),
            order_id: order.into(),
            device_id: hash(self.config.root.to_string_lossy().as_bytes()),
            engine: options.engine,
            mode: options.mode,
            epoch: Uuid::new_v4().to_string(),
            connection_id: options.connection_id,
            connected: true,
            manual: false,
            capabilities: BrowserCapabilities {
                live_view: options.mode == BrowserMode::Managed,
                pointer: options.mode == BrowserMode::Managed,
                keyboard: options.mode == BrowserMode::Managed,
                developer_tools: options.mode == BrowserMode::Managed,
                selected_tabs_only: options.mode == BrowserMode::Extension,
            },
        };
        all.insert(
            view.id.clone(),
            Arc::new(Session {
                view: view.clone(),
                gate: Mutex::new(()),
                _profile_lock: profile_lock,
            }),
        );
        self.browser_selected
            .lock()
            .map_err(|_| Error::Conflict)?
            .insert(order.into(), view.id.clone());
        Ok(view)
    }
    fn browser_profile(&self, project: &str, engine: BrowserEngine) -> Result<PathBuf> {
        identifier(project)?;
        let path = self
            .config
            .root
            .join("browser-profiles")
            .join(project)
            .join(engine_name(engine));
        private_dir(&path)?;
        Ok(path)
    }
    pub fn delete_browser_profile(&self, project: &str, engine: BrowserEngine) -> Result<()> {
        let sessions = self.browser_v2.lock().map_err(|_| Error::Conflict)?;
        if sessions.values().any(|s| {
            s.view.project_id == project
                && s.view.engine == engine
                && s.view.mode == BrowserMode::Managed
        }) {
            return Err(invalid("browser_profile_busy"));
        }
        let path = self.browser_profile(project, engine)?;
        namespace(&path.join(".cowork-profile.lock"))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.join(".cowork-profile.lock"))?;
        lock.try_lock()
            .map_err(|_| invalid("browser_profile_busy"))?;
        // Delete only the managed content, while retaining the locked parent.
        for name in ["profile", "runtime.json"] {
            let target = path.join(name);
            namespace(&target)?;
            if target.is_dir() {
                fs::remove_dir_all(target)?;
            } else if target.is_file() {
                fs::remove_file(target)?;
            }
        }
        Ok(())
    }
    pub fn browser_sessions_v2(&self, order: &str) -> Result<Vec<BrowserSessionView>> {
        let connections = self.browser_connections();
        Ok(self
            .browser_v2
            .lock()
            .map_err(|_| Error::Conflict)?
            .values()
            .filter(|s| s.view.order_id == order)
            .map(|s| {
                let mut v = s.view.clone();
                v.manual = self
                    .browser_manual
                    .lock()
                    .ok()
                    .and_then(|c| c.get(&v.id).copied())
                    .unwrap_or(false);
                if v.mode == BrowserMode::Extension {
                    v.connected = connections
                        .iter()
                        .any(|c| Some(&c.id) == v.connection_id.as_ref() && c.connected);
                }
                v
            })
            .collect())
    }
    pub fn browser_takeover(
        &self,
        order: &str,
        id: &str,
        manual: bool,
    ) -> Result<BrowserSessionView> {
        let session = self.browser_session_v2(order, id)?;
        let _gate = session.gate.lock().map_err(|_| Error::Conflict)?;
        let mut all = self.browser_v2.lock().map_err(|_| Error::Conflict)?;
        // Replacing metadata must preserve the original profile lock and gate.
        let old = all.get_mut(id).ok_or(Error::NotFound)?;
        let mut controls = self.browser_manual.lock().map_err(|_| Error::Conflict)?;
        controls.insert(id.into(), manual);
        self.browser_selected
            .lock()
            .map_err(|_| Error::Conflict)?
            .insert(order.into(), id.into());
        let mut view = old.view.clone();
        view.manual = manual;
        Ok(view)
    }
    fn browser_session_v2(&self, order: &str, id: &str) -> Result<Arc<Session>> {
        self.browser_v2
            .lock()
            .map_err(|_| Error::Conflict)?
            .get(id)
            .filter(|s| s.view.order_id == order)
            .cloned()
            .ok_or(Error::NotFound)
    }
    pub(crate) fn has_browser_v2(&self, id: &str) -> bool {
        self.browser_v2.lock().is_ok_and(|v| v.contains_key(id))
    }
    pub fn selected_browser_session(&self, order: &str) -> Option<String> {
        let selected = self.browser_selected.lock().ok()?.get(order).cloned()?;
        self.browser_session_v2(order, &selected)
            .ok()
            .map(|s| s.view.id.clone())
    }
    pub(crate) fn browser_approved_command(
        &self,
        order: &str,
        action: &Action,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let Action::Browser {
            session_id,
            operation,
            url,
            selector,
            text,
        } = action
        else {
            return Err(Error::Conflict);
        };
        let command = if operation == "control" {
            serde_json::from_str(text.as_deref().unwrap_or(""))?
        } else {
            BrowserCommand {
                operation: operation.clone(),
                url: url.clone(),
                selector: selector.clone(),
                text: text.clone(),
                ..Default::default()
            }
        };
        self.browser_command_v2(order, session_id, command, false, cancel)
    }
    pub fn browser_command_v2(
        &self,
        order: &str,
        id: &str,
        command: BrowserCommand,
        human: bool,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        if self.closing.load(Ordering::Acquire) {
            return Err(Error::Conflict);
        }
        validate_browser_command(&command)?;
        let session = self.browser_session_v2(order, id)?;
        let _gate = session.gate.lock().map_err(|_| Error::Conflict)?;
        if !self
            .browser_v2
            .lock()
            .map_err(|_| Error::Conflict)?
            .get(id)
            .is_some_and(|live| Arc::ptr_eq(live, &session))
        {
            return Err(Error::NotFound);
        }
        let manual = self
            .browser_manual
            .lock()
            .map_err(|_| Error::Conflict)?
            .get(id)
            .copied()
            .unwrap_or(false);
        if command.operation != "frame" && command.operation != "tabs" && manual != human {
            return Err(invalid(if manual {
                "browser_manual_takeover"
            } else {
                "browser_take_over_first"
            }));
        }
        if cancel.load(Ordering::Acquire) {
            return Err(invalid("browser_cancelled"));
        }
        if session.view.mode == BrowserMode::Extension {
            if [
                "frame",
                "pointer",
                "key",
                "resize",
                "console",
                "network",
                "network_details",
                "annotate",
                "annotations",
                "clear_annotations",
            ]
            .contains(&command.operation.as_str())
            {
                return Err(invalid("browser_capability_unavailable_use_native_browser"));
            }
            return self.browser_bridge.call(
                session
                    .view
                    .connection_id
                    .as_deref()
                    .ok_or(Error::NotFound)?,
                &session.view.epoch,
                command,
                cancel,
            );
        }
        let service = self.browser_managed_service(&session.view, cancel)?;
        let result = service.call(serde_json::to_value(&command)?, 45, cancel)?;
        if result["context_epoch"] != session.view.epoch {
            service.stop();
            return Err(invalid("browser_original_context_changed"));
        }
        if result["ok"] != true {
            return Err(invalid(
                result["error"]
                    .as_str()
                    .unwrap_or("browser_effect_uncertain"),
            ));
        }
        Ok(result)
    }
    pub fn close_browser_sessions_for_order(&self, order: &str) {
        let ids = self
            .sessions
            .lock()
            .map(|all| {
                all.values()
                    .filter(|s| s.order_id == order)
                    .map(|s| s.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for id in ids {
            let _ = self.close_browser_session(order, &id);
        }
    }
    pub fn browser_manual_input(
        &self,
        order: &str,
        id: &str,
        input: BrowserInput,
    ) -> Result<Value> {
        identifier(&input.idempotency_key)?;
        let session = self.browser_session_v2(order, id)?;
        if input.epoch != session.view.epoch {
            return Err(invalid("browser_original_context_changed"));
        }
        let directory = self
            .config
            .root
            .join("sessions")
            .join(id)
            .join("manual-inputs");
        private_dir(&directory)?;
        let key = hash(input.idempotency_key.as_bytes());
        let intent = directory.join(format!("{key}.intent.json"));
        let result = directory.join(format!("{key}.result.json"));
        // Retain the fingerprint, never typed text (which can be a password).
        let raw = hash(&serde_json::to_vec(&input)?).into_bytes();
        namespace(&intent)?;
        namespace(&result)?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&intent)
        {
            Ok(mut file) => {
                file.write_all(&raw)?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::read(intent)? != raw {
                    return Err(Error::Conflict);
                }
                return if result.is_file() {
                    Ok(serde_json::from_slice(&fs::read(result)?)?)
                } else {
                    Err(invalid("browser_manual_effect_uncertain_do_not_replay"))
                };
            }
            Err(error) => return Err(error.into()),
        }
        let value =
            self.browser_command_v2(order, id, input.command, true, &AtomicBool::new(false))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(result)?;
        file.write_all(&serde_json::to_vec(&value)?)?;
        file.sync_all()?;
        Ok(value)
    }
    fn browser_managed_service(
        &self,
        view: &BrowserSessionView,
        cancel: &AtomicBool,
    ) -> Result<Arc<persistent::ManagedService>> {
        let mut services = self.managed.lock().map_err(|_| Error::Conflict)?;
        if let Some(service) = services.get(&view.id) {
            return Ok(service.clone());
        }
        let programs = self.programs.lock().map_err(|_| Error::Conflict)?;
        let python = programs
            .get("python")
            .ok_or_else(|| invalid("browser_install_runtime_first"))?;
        let executable = programs
            .get(engine_name(view.engine))
            .ok_or_else(|| invalid("browser_install_runtime_first"))?;
        namespace(executable)?;
        let profile = self.browser_profile(&view.project_id, view.engine)?;
        let directory = self
            .config
            .root
            .join("sessions")
            .join(&view.id)
            .join(&view.epoch);
        private_dir(&directory)?;
        let manifest = profile.join("runtime.json");
        let executable_hash = hash(&fs::read(executable)?);
        let runtime = json!({"engine":view.engine,"sha256":executable_hash});
        if manifest.exists() {
            namespace(&manifest)?;
            if serde_json::from_slice::<Value>(&fs::read(&manifest)?)? != runtime {
                return Err(invalid(
                    "browser_profile_runtime_changed_delete_profile_or_restore_original_runtime",
                ));
            }
        } else {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&manifest)?;
            f.write_all(&serde_json::to_vec(&runtime)?)?;
            f.sync_all()?;
        }
        let config = json!({"session_id":view.id,"epoch":view.epoch,"engine":view.engine,"executable":executable,
            "executable_sha256":executable_hash,"profile":profile.join("profile"),"workspace":self.workspace(&view.order_id)?});
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("config.json"))?;
        file.write_all(&serde_json::to_vec(&config)?)?;
        file.sync_all()?;
        let launch = cowork_executor::TrustedLaunch {
            program: python.clone(),
            arguments: vec![
                "-I".into(),
                self.adapters
                    .join("managed_browser.py")
                    .to_string_lossy()
                    .into(),
                directory.to_string_lossy().into(),
            ],
            directory: self.workspace(&view.order_id)?,
            environment: self.config.environment.clone(),
            timeout: Duration::from_secs(24 * 3600),
            capture_bytes_per_stream: 4096,
        };
        let service = persistent::ManagedService::start(directory, launch, cancel)?;
        services.insert(view.id.clone(), service.clone());
        Ok(service)
    }
    pub(crate) fn browser_v2_outcome(
        &self,
        order: &str,
        request: &Request,
        cancel: &AtomicBool,
    ) -> Result<cowork_executor::Outcome> {
        let started = std::time::Instant::now();
        let value = self.browser_approved_command(order, &request.action, cancel)?;
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
}
