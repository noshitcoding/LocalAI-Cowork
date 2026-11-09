use serde::Serialize;
use std::{path::PathBuf, process::Command, time::Duration};
use tauri::Manager;
mod actor_model_registry;
mod audit;
mod backup_save;
mod browser_host;
mod connectors;
mod daemon_process;
#[cfg(windows)]
mod data_directory;
mod desktop_launch;
mod desktop_preferences;
mod folders;
mod historical_imports;
mod imports;
mod key_recovery;
mod mcp;
mod model_registry;
mod native_client;
mod offline;
mod offline_mcp;
mod oidc_custody;
mod passkey;
mod passkey_transport;
mod personal_providers;
#[cfg(windows)]
#[path = "../../../../services/daemon/src/private_secret_file.rs"]
mod private_secret_file;
mod save;
mod secrets;
mod tool_setup;
#[cfg(windows)]
mod update_installer;
mod update_journal;
mod update_witness;
mod updates;
#[cfg(windows)]
mod windows_startup;

fn application_window(window: &tauri::WebviewWindow) -> bool {
    let Ok(url) = window.url() else {
        return false;
    };
    window.label() == "main"
        && ((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
            || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"))
            || (cfg!(debug_assertions)
                && url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
                && url.port() == Some(5175)))
}
#[derive(Serialize)]
struct Connection {
    endpoint: String,
    token: String,
}
fn private_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let launch = app
        .try_state::<desktop_launch::DataDirectory>()
        .ok_or("Desktop launch profile is unavailable")?;
    let path = match launch.path() {
        Some(path) => Ok(path.to_path_buf()),
        None => app
            .path()
            .app_local_data_dir()
            .map_err(|_| String::from("Cannot resolve V2 data directory")),
    }?;
    #[cfg(windows)]
    app.state::<data_directory::Root>()
        .prepare(&path)
        .map_err(|_| "Cannot prepare private V2 data directory")?;
    Ok(path)
}
fn daemon_path() -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        "cowork-daemon.exe"
    } else {
        "cowork-daemon"
    };
    if cfg!(debug_assertions) {
        return Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!(
                "cowork-daemon-{}{}",
                env!("COWORK_TARGET_TRIPLE"),
                if cfg!(windows) { ".exe" } else { "" }
            )));
    }
    Ok(std::env::current_exe()
        .map_err(|_| "Cannot resolve application path")?
        .parent()
        .ok_or("Application directory missing")?
        .join(name))
}
fn compatible_health(raw: &[u8]) -> bool {
    let Ok(health) = serde_json::from_slice::<cowork_contracts::Health>(raw) else {
        return false;
    };
    health.product == "LocalAI Cowork V2"
        && health.version == env!("CARGO_PKG_VERSION")
        && health.schema_version == cowork_contracts::SCHEMA_VERSION
        && health.deployment == "single_owner_preview"
        && health
            .capabilities
            .iter()
            .any(|value| value == "personal_money_status")
}
#[tauri::command]
async fn connection_info(
    app: tauri::AppHandle,
    launch: tauri::State<'_, tokio::sync::Mutex<()>>,
) -> Result<Connection, String> {
    let _launch = launch.lock().await;
    let dir = private_data_dir(&app)?;
    let update_folder = update_journal::folder(&dir)?;
    let _update_guard = update_journal::lock(&update_folder)?;
    let update_gate = app.state::<updates::UpdateGate>();
    update_gate.refresh(&update_folder)?;
    if update_gate.blocked() {
        return Err("Daemon start is paused until the recorded update attempt is resolved.".into());
    }
    std::fs::create_dir_all(&dir).map_err(|_| "Cannot create V2 data directory")?;
    let token_file = dir.join("daemon.token");
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(750))
        .build()
        .map_err(|_| "HTTP initialization failed")?;
    async fn connected(client: &reqwest::Client, path: &std::path::Path) -> Option<String> {
        let token = std::fs::read_to_string(path).ok()?.trim().to_owned();
        let mut response = client
            .get("http://127.0.0.1:43821/api/v2/health")
            .bearer_auth(&token)
            .send()
            .await
            .ok()?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|length| length > 65536)
        {
            return None;
        }
        let mut raw = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if raw.len().checked_add(chunk.len())? > 65536 {
                return None;
            }
            raw.extend_from_slice(&chunk);
        }
        compatible_health(&raw).then_some(token)
    }
    if let Some(token) = connected(&client, &token_file).await {
        return Ok(Connection {
            endpoint: "http://127.0.0.1:43821".into(),
            token,
        });
    }
    // The child is deliberately independent of the WebView/window lifetime.
    let mut command = Command::new(daemon_path()?);
    command.env("COWORK_DATA_DIR", &dir);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
        .spawn()
        .map_err(|_| "Cannot start the V2 daemon. Build or install its matching sidecar first.")?;
    for _ in 0..40 {
        if let Some(token) = connected(&client, &token_file).await {
            return Ok(Connection {
                endpoint: "http://127.0.0.1:43821".into(),
                token,
            });
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err("V2 daemon did not become ready. Check the data directory and whether another V2 instance owns port 43821.".into())
}

fn main() {
    #[cfg(windows)]
    if let Some(result) = update_installer::early_mode() {
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    if let Some(result) = update_witness::early_mode() {
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    // Observer modes above accept only their original isolated argument contract.
    let launch = match desktop_launch::DataDirectory::from_process() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2)
        }
    };
    let autostart = tauri_plugin_autostart::Builder::new().args(launch.autostart_args());
    #[cfg(target_os = "macos")]
    let autostart = autostart.macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent);
    let builder = tauri::Builder::default();
    #[cfg(windows)]
    let builder = builder.manage(data_directory::Root::default());
    builder
        .manage(launch)
        .plugin(tauri_plugin_dialog::init())
        .plugin(autostart.build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates::UpdateGate::default())
        .manage(tokio::sync::Mutex::new(()))
        .manage(save::SaveGate::default())
        .manage(folders::FolderGate::default())
        .manage(secrets::SecretsGate::default())
        .manage(key_recovery::RecoveryGate::default())
        .manage(model_registry::RegistryGate::default())
        .manage(imports::ImportGate::default())
        .manage(historical_imports::HistoricalImportGate::default())
        .manage(tool_setup::ToolSetupGate::default())
        .manage(passkey::PasskeyGate::default())
        .setup(|app| {
            updates::initialize(app.handle());
            browser_host::initialize();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connection_info,
            desktop_preferences::desktop_preferences_status,
            desktop_preferences::desktop_set_autostart,
            desktop_preferences::desktop_request_notifications,
            desktop_preferences::desktop_notify,
            desktop_preferences::desktop_sound,
            passkey::native_passkey_login,
            passkey::native_passkey_register,
            passkey::native_passkey_cancel,
            passkey::native_passkey_pending,
            passkey::native_passkey_resume,
            passkey::native_passkey_resolve,
            oidc_custody::native_oidc_store,
            oidc_custody::native_oidc_pending,
            oidc_custody::native_oidc_read,
            oidc_custody::native_oidc_forget,
            updates::desktop_update_check,
            updates::desktop_update_install,
            updates::desktop_update_status,
            updates::desktop_update_reconcile,
            tool_setup::toolbox_setup_status,
            browser_host::browser_host_registration,
            tool_setup::toolbox_setup_start,
            tool_setup::toolbox_setup_select,
            tool_setup::toolbox_setup_cancel,
            tool_setup::toolbox_setup_reload,
            tool_setup::toolbox_setup_ensure_documents,
            personal_providers::configure_personal_provider,
            model_registry::native_model_registry_import,
            model_registry::native_model_registry_revoke,
            actor_model_registry::native_actor_model_registry_import,
            actor_model_registry::native_actor_model_registry_revoke,
            backup_save::save_database_backup,
            connectors::connector_save,
            connectors::connector_action,
            mcp::configure_mcp_stdio,
            mcp::save_mcp_credential,
            mcp::mcp_credential_status,
            mcp::mcp_stdio_factories,
            mcp::disconnect_mcp_stdio,
            mcp::mcp_oauth_native,
            save::save_file,
            imports::preview_v1_import,
            imports::commit_v1_import,
            imports::preview_v1_team_import,
            imports::commit_v1_team_import,
            historical_imports::historical_import_request,
            historical_imports::historical_import_upload_chunk,
            historical_imports::historical_import_read_page,
            historical_imports::historical_import_read_history_bytes,
            historical_imports::historical_import_select,
            historical_imports::historical_import_stage_file,
            folders::choose_native_folder,
            folders::list_native_folders,
            folders::revoke_native_folder,
            secrets::native_secrets_status,
            secrets::native_secrets_request,
            secrets::native_secrets_setup,
            secrets::native_secrets_reconcile,
            secrets::native_secrets_abandon,
            key_recovery::native_keys_backup,
            key_recovery::native_keys_restore,
            key_recovery::native_keys_reencrypt,
            key_recovery::native_keys_cleanup,
            audit::native_audit_verify_v1,
            key_recovery::native_keys_retire,
            key_recovery::native_keys_retire_orphan,
            offline::offline_device,
            offline::offline_grants,
            offline::offline_runs,
            offline::offline_issue_grant,
            offline::offline_import_grant,
            offline::offline_configure_key,
            offline::offline_configure_tools,
            offline::offline_tool_configuration,
            offline::offline_cancel,
            offline::offline_workspace_file,
            offline::offline_workspace_upload,
            offline::offline_start,
            offline::offline_approve,
            offline::offline_export_run,
            offline::offline_run,
            offline::offline_ticket,
            offline::offline_check_grant,
            offline::offline_reconcile,
            offline_mcp::offline_mcp_tasks,
            offline_mcp::offline_mcp_task_detail,
            offline_mcp::offline_mcp_interactions,
            offline_mcp::offline_mcp_answer,
            offline_mcp::offline_mcp_cancel,
            offline_mcp::offline_mcp_result,
            offline_mcp::offline_mcp_original_receipt
        ])
        .build(tauri::generate_context!())
        .expect("LocalAI Cowork V2 failed to start")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
                app.state::<tool_setup::ToolSetupGate>().shutdown();
            }
        });
}

#[cfg(test)]
mod compatibility_tests {
    use super::compatible_health;
    use serde_json::json;

    fn current() -> serde_json::Value {
        json!({"product":"LocalAI Cowork V2", "version":env!("CARGO_PKG_VERSION"),
            "schema_version":cowork_contracts::SCHEMA_VERSION, "deployment":"single_owner_preview",
            "capabilities":["orders", "personal_money_status"]})
    }

    #[test]
    fn current_daemon_contract_is_accepted() {
        assert!(compatible_health(&serde_json::to_vec(&current()).unwrap()));
    }

    #[test]
    fn older_or_foreign_ready_process_cannot_replace_the_matching_sidecar() {
        for (field, value) in [
            ("product", json!("other service")),
            ("version", json!("1.0.0")),
            (
                "schema_version",
                json!(cowork_contracts::SCHEMA_VERSION - 1),
            ),
            ("deployment", json!("tenant")),
            ("capabilities", json!(["orders"])),
        ] {
            let mut health = current();
            health[field] = value;
            assert!(!compatible_health(&serde_json::to_vec(&health).unwrap()));
        }
        assert!(!compatible_health(b"not JSON"));
    }
}
