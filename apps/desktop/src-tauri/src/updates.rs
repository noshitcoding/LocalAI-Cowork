use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;
use tauri_plugin_updater::{Update, UpdaterExt};

pub struct UpdateGate {
    pending: tokio::sync::Mutex<Option<(String, Update)>>,
    blocked: AtomicBool,
}
impl Default for UpdateGate {
    fn default() -> Self {
        Self {
            pending: tokio::sync::Mutex::new(None),
            blocked: AtomicBool::new(true),
        }
    }
}
impl UpdateGate {
    pub fn blocked(&self) -> bool {
        self.blocked.load(Ordering::SeqCst)
    }
    pub fn refresh(
        &self,
        folder: &std::path::Path,
    ) -> Result<Vec<super::update_journal::PendingAttempt>, String> {
        match super::update_journal::pending(folder) {
            Ok(pending) => {
                self.blocked.store(!pending.is_empty(), Ordering::SeqCst);
                Ok(pending)
            }
            Err(error) => {
                self.blocked.store(true, Ordering::SeqCst);
                Err(error)
            }
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Lifecycle {
    format: u32,
    product: String,
    pub(crate) version: String,
    schema_version: u32,
    pid: u32,
    instance_id: String,
    closing: bool,
    #[serde(default)]
    active_owner_terminals: usize,
}
impl Lifecycle {
    pub(crate) fn valid(&self) -> bool {
        self.format == 1
            && self.product == "LocalAI Cowork V2"
            && !self.version.is_empty()
            && self.version.len() <= 128
            && self.schema_version > 0
            && self.pid > 1
            && uuid::Uuid::parse_str(&self.instance_id).is_ok_and(|id| !id.is_nil())
    }
    fn checked(&self) -> bool {
        self.valid()
            && self.version == env!("CARGO_PKG_VERSION")
            && self.schema_version == cowork_contracts::SCHEMA_VERSION
            && self.pid > 1
            && self.pid != std::process::id()
            && uuid::Uuid::parse_str(&self.instance_id).is_ok_and(|id| !id.is_nil())
    }
}
pub fn initialize(app: &tauri::AppHandle) {
    let gate = app.state::<UpdateGate>();
    let _ = (|| -> Result<(), String> {
        let folder = super::update_journal::folder(&super::private_data_dir(app)?)?;
        let _guard = super::update_journal::lock(&folder)?;
        gate.refresh(&folder).map(|_| ())
    })();
}
#[derive(Serialize)]
pub struct UpdateStatus {
    blocked: bool,
    current_version: String,
    attempts: Vec<super::update_journal::PendingAttempt>,
    journal_error: Option<String>,
}
#[tauri::command]
pub async fn desktop_update_status(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, UpdateGate>,
) -> Result<UpdateStatus, String> {
    if !super::application_window(&window) {
        return Err("native_update_window_required".into());
    }
    let launch = app.state::<tokio::sync::Mutex<()>>();
    let _launch = launch.lock().await;
    let folder = super::update_journal::folder(&super::private_data_dir(&app)?)?;
    let _guard = super::update_journal::lock(&folder)?;
    let (attempts, journal_error) = match gate.refresh(&folder) {
        Ok(v) => (v, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    Ok(UpdateStatus {
        blocked: gate.blocked(),
        current_version: env!("CARGO_PKG_VERSION").into(),
        attempts,
        journal_error,
    })
}
#[tauri::command]
pub async fn desktop_update_reconcile(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, UpdateGate>,
    attempt: String,
) -> Result<UpdateStatus, String> {
    use super::update_journal as journal;
    if !super::application_window(&window) {
        return Err("native_update_window_required".into());
    }
    let launch = app.state::<tokio::sync::Mutex<()>>();
    let _launch = launch.lock().await;
    let folder = journal::folder(&super::private_data_dir(&app)?)?;
    let _guard = journal::lock(&folder)?;
    let (_, digest) = journal::load(&folder, &attempt)?;
    let current = journal::pending(&folder)?;
    let value = current
        .iter()
        .find(|v| v.attempt == attempt)
        .ok_or("update_attempt_not_pending")?;
    if !value.can_reconcile {
        return Err("installer_outcome_unconfirmed".into());
    }
    journal::phase(&folder, &attempt, &digest, "reconciled-without-replay")?;
    let attempts = gate.refresh(&folder)?;
    Ok(UpdateStatus {
        blocked: gate.blocked(),
        current_version: env!("CARGO_PKG_VERSION").into(),
        attempts,
        journal_error: None,
    })
}

#[derive(Serialize)]
pub struct UpdateInfo {
    configured: bool,
    current_version: String,
    update_id: Option<String>,
    version: Option<String>,
    notes: Option<String>,
}

#[tauri::command]
pub async fn desktop_update_check(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, UpdateGate>,
) -> Result<UpdateInfo, String> {
    if !super::application_window(&window) {
        return Err("native_update_window_required".into());
    }
    if gate.blocked() {
        return Err("recorded_update_requires_process_reconciliation".into());
    }
    let configured = app.config().plugins.0.get("updater").is_some_and(|value| {
        value
            .get("pubkey")
            .and_then(|v| v.as_str())
            .is_some_and(|v| !v.is_empty())
    });
    let mut pending = gate.pending.lock().await;
    *pending = None;
    if !configured {
        return Ok(UpdateInfo {
            configured: false,
            current_version: env!("CARGO_PKG_VERSION").into(),
            update_id: None,
            version: None,
            notes: None,
        });
    }
    let update = app
        .updater()
        .map_err(|_| "update_configuration_invalid")?
        .check()
        .await
        .map_err(|_| "update_check_failed")?;
    let Some(update) = update else {
        return Ok(UpdateInfo {
            configured: true,
            current_version: env!("CARGO_PKG_VERSION").into(),
            update_id: None,
            version: None,
            notes: None,
        });
    };
    if update.download_url.scheme() != "https"
        || !update.download_url.username().is_empty()
        || update.download_url.password().is_some()
        || update.signature.is_empty()
    {
        return Err("signed_https_update_required".into());
    }
    let id = format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{}\0{}\0{}",
                update.version, update.download_url, update.signature
            )
            .as_bytes()
        )
    );
    let result = UpdateInfo {
        configured: true,
        current_version: update.current_version.clone(),
        update_id: Some(id.clone()),
        version: Some(update.version.clone()),
        notes: update.body.clone(),
    };
    *pending = Some((id, update));
    Ok(result)
}

#[tauri::command]
pub async fn desktop_update_install(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, UpdateGate>,
    update_id: String,
) -> Result<(), String> {
    if !super::application_window(&window) {
        return Err("native_update_window_required".into());
    }
    if gate.blocked() {
        return Err("recorded_update_requires_process_reconciliation".into());
    }
    let mut pending = gate.pending.lock().await;
    let (id, update) = pending.as_ref().ok_or("checked_update_missing")?;
    if id != &update_id {
        return Err("checked_update_changed".into());
    }
    // Verify the complete payload before requesting any daemon shutdown.
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|_| "signed_update_download_failed")?;
    let launch = app.state::<tokio::sync::Mutex<()>>();
    let _launch = launch.lock().await;
    let folder = super::update_journal::folder(&super::private_data_dir(&app)?)?;
    let _journal_guard = super::update_journal::lock(&folder)?;
    gate.refresh(&folder)?;
    if gate.blocked() {
        return Err("recorded_update_requires_process_reconciliation".into());
    }
    let native = super::native_client::NativeClient::new(
        &window,
        "native_update_connection_unavailable",
        "native_update_shutdown_unconfirmed",
    )
    .map_err(|e| e.code)?;
    let current: Lifecycle = native
        .result(
            native.request(reqwest::Method::GET, "/native/lifecycle"),
            false,
        )
        .await
        .map_err(|e| e.code)?;
    if !current.checked() || current.closing {
        return Err("matching_idle_daemon_required".into());
    }
    if current.active_owner_terminals > 0 {
        return Err("stop_active_terminals_before_update".into());
    }
    let path = super::daemon_path()?;
    let pid = current.pid;
    let process = tauri::async_runtime::spawn_blocking(move || {
        super::daemon_process::DaemonProcess::open(pid, &path)
    })
    .await
    .map_err(|_| "daemon_process_observation_failed")??;
    let attempt = format!("{}-{}", update_id, uuid::Uuid::new_v4());
    let payload = format!("{:x}", Sha256::digest(&bytes));
    use super::update_journal as journal;
    let digest = journal::record(
        &folder.join(format!("{attempt}.json")),
        &journal::Attempt {
            format: journal::FORMAT,
            attempt: attempt.clone(),
            update_id,
            from: update.current_version.clone(),
            to: update.version.clone(),
            payload_sha256: payload,
            signature_verified: true,
            daemon: current.clone(),
            state: "shutdown_prepared".into(),
        },
    )?;
    gate.blocked.store(true, Ordering::SeqCst);
    #[cfg(windows)]
    if let Err(error) = super::update_installer::prepare(&folder, &attempt, &digest, &bytes) {
        journal::phase(&folder, &attempt, &digest, "shutdown-not-dispatched")?;
        gate.refresh(&folder)?;
        return Err(error);
    }
    let mut observer = match super::update_witness::spawn(&process, &folder, &attempt, &digest) {
        Ok(child) => child,
        Err(error) => {
            journal::phase(&folder, &attempt, &digest, "shutdown-not-dispatched")?;
            gate.refresh(&folder)?;
            return Err(error);
        }
    };
    if let Err(error) =
        super::update_witness::ready(&mut observer, &folder, &attempt, &digest).await
    {
        journal::phase(&folder, &attempt, &digest, "shutdown-not-dispatched")?;
        tauri::async_runtime::spawn_blocking(move || observer.wait());
        gate.refresh(&folder)?;
        return Err(error);
    }
    let result:Result<Lifecycle,_>=native.post("/native/lifecycle/shutdown",&serde_json::json!({"version":current.version,
        "schema_version":current.schema_version,"pid":current.pid,"instance_id":current.instance_id})).await;
    match result {
        Ok(stopped)
            if stopped.checked()
                && stopped.pid == current.pid
                && stopped.instance_id == current.instance_id
                && stopped.closing =>
        {
            journal::phase(&folder, &attempt, &digest, "shutdown-acknowledged")?;
        }
        Ok(_) => {
            tauri::async_runtime::spawn_blocking(move || observer.wait());
            return Err("daemon_shutdown_reply_changed_update_not_installed".into());
        }
        Err(error) if error.code != "native_update_shutdown_unconfirmed" => {
            journal::phase(&folder, &attempt, &digest, "shutdown-rejected")?;
            tauri::async_runtime::spawn_blocking(move || observer.wait());
            gate.refresh(&folder)?;
            return Err(error.code);
        }
        Err(_) => {}
    };
    // Retaining a native handle closes the PID-reuse gap. Lost HTTP replies never
    // suffice: only positive exit observation permits installation.
    super::update_witness::stopped(observer, &folder, &attempt, &digest).await?;
    #[cfg(windows)]
    {
        let mut installer = super::update_installer::spawn(&folder, &attempt, &digest)?;
        if let Err(error) =
            super::update_installer::ready(&mut installer, &folder, &attempt, &digest).await
        {
            journal::phase(&folder, &attempt, &digest, "installer-handoff-aborted")?;
            tauri::async_runtime::spawn_blocking(move || installer.wait());
            return Err(error);
        }
        journal::phase(&folder, &attempt, &digest, "install-requested")?;
        // The detached observer owns the exact original GUI process handle and
        // launches the bound installer only after this application physically exits.
        tauri::async_runtime::spawn_blocking(move || installer.wait());
        *pending = None;
        app.exit(0);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        journal::phase(&folder, &attempt, &digest, "install-requested")?;
        if update.install(bytes).is_err() {
            journal::phase(&folder, &attempt, &digest, "installer-failed")?;
            return Err("update_install_failed_receipt_retained".into());
        }
        // Linux's updater returns after atomic replacement.
        #[cfg(target_os = "linux")]
        journal::phase(&folder, &attempt, &digest, "install-applied")?;
        *pending = None;
        #[cfg(target_os = "linux")]
        app.restart();
        Ok(())
    }
}
