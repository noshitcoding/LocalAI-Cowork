use super::native_client::{NativeClient, failure};
use cowork_contracts::{ApiError, UiLanguage};
use serde_json::Value;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

#[derive(Default)]
pub struct ImportGate {
    source: tokio::sync::Mutex<Option<Value>>,
    team: tokio::sync::Mutex<Option<Value>>,
}

#[tauri::command]
pub async fn preview_v1_import(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, ImportGate>,
    language: UiLanguage,
    source_family: Option<String>,
    capture_attachments: Option<bool>,
) -> Result<Option<Value>, ApiError> {
    let client = NativeClient::new(&window, "import_connection_failed", "import_uncertain")?;
    let mut held = gate.source.try_lock().map_err(|_| failure("import_busy"))?;
    let source_family = source_family.unwrap_or_else(|| "desktop".into());
    if !matches!(source_family.as_str(), "desktop" | "daemon" | "enterprise") {
        return Err(failure("invalid_request"));
    }
    let app = window.app_handle().clone();
    let german = matches!(language, UiLanguage::De);
    let selected = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title(if german {
                "V1-Datei zur Übernahme auswählen"
            } else {
                "Select V1 source file"
            })
            .add_filter("V1 SQLite / JSON", &["db", "sqlite", "sqlite3", "json"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| failure("import_connection_failed"))?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|_| failure("invalid_request"))?;
    let kind = if path
        .extension()
        .and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case("json"))
    {
        "json"
    } else {
        "sqlite"
    };
    let app = window.app_handle().clone();
    let approved=tauri::async_runtime::spawn_blocking(move||app.dialog().message(if german{"Die ausgewählte Datei wird unverändert in eine private Kopie eingelesen. Unter Linux muss eine SQLite-Quelldatei aus einer geschlossenen V1-Installation stammen. Bestätige, dass du diese Quelle ausdrücklich übernehmen möchtest."}else{"The selected source is read into a private copy. On Linux, SQLite must be an offline export from a closed V1 installation. Confirm this explicit source selection."}).title("LocalAI Cowork V2").kind(MessageDialogKind::Info).buttons(MessageDialogButtons::OkCancel).blocking_show()).await.map_err(|_|failure("import_connection_failed"))?;
    if !approved {
        return Ok(None);
    }
    let source_root = if capture_attachments.unwrap_or(false) {
        let app = window.app_handle().clone();
        let selected = tauri::async_runtime::spawn_blocking(move || {
            app.dialog()
                .file()
                .set_title(if german {
                    "Originalordner der V1-Artefakte ausdrücklich freigeben"
                } else {
                    "Select and authorize the original V1 artifact directory"
                })
                .blocking_pick_folder()
        })
        .await
        .map_err(|_| failure("import_connection_failed"))?;
        let Some(selected) = selected else {
            return Ok(None);
        };
        Some(
            selected
                .into_path()
                .map_err(|_| failure("invalid_request"))?,
        )
    } else {
        None
    };
    let result=post_import(&client,"/native/import-v1/preview",serde_json::json!({"path":path,"kind":kind,"cold_source_confirmed":true,"source_family":source_family,"binary_files_confirmed":source_root.is_some(),"source_root":source_root}),None).await?;
    *gate.team.try_lock().map_err(|_| failure("import_busy"))? = None;
    *held = Some(result.clone());
    Ok(Some(result))
}
#[tauri::command]
pub async fn commit_v1_import(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, ImportGate>,
    preview_id: String,
    plan_sha256: String,
) -> Result<Value, ApiError> {
    let client = NativeClient::new(&window, "import_connection_failed", "import_uncertain")?;
    let held = gate.source.try_lock().map_err(|_| failure("import_busy"))?;
    let preview = held
        .as_ref()
        .ok_or_else(|| failure("import_preview_required"))?;
    if preview["id"].as_str() != Some(&preview_id)
        || preview["plan_sha256"].as_str() != Some(&plan_sha256)
    {
        return Err(failure("import_preview_conflict"));
    }
    post_import(
        &client,
        "/native/import-v1/commit",
        serde_json::json!({"preview_id":preview_id,"plan_sha256":plan_sha256}),
        None,
    )
    .await
}

async fn post_import(
    client: &NativeClient,
    path: &str,
    body: Value,
    session: Option<&str>,
) -> Result<Value, ApiError> {
    let mut request = client
        .request(reqwest::Method::POST, path)
        .timeout(std::time::Duration::from_secs(300))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&body).map_err(|_| failure("invalid_request"))?);
    if let Some(session) = session {
        if session.is_empty() || session.len() > 4096 || session.chars().any(char::is_control) {
            return Err(failure("invalid_request"));
        }
        request = request.bearer_auth(session);
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| failure("import_uncertain"))?;
    let success = response.status().is_success();
    const LIMIT: usize = 64 * 1024 * 1024;
    if response.content_length().is_some_and(|v| v > LIMIT as u64) {
        return Err(failure("import_uncertain"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("import_uncertain"))?
    {
        if chunk.len() > LIMIT - bytes.len() {
            return Err(failure("import_uncertain"));
        }
        bytes.extend_from_slice(&chunk);
    }
    if !success {
        return Err(serde_json::from_slice(&bytes).unwrap_or_else(|_| failure("import_uncertain")));
    }
    serde_json::from_slice(&bytes).map_err(|_| failure("import_uncertain"))
}
fn team_path(team: &str, project: &str, operation: &str) -> Result<String, ApiError> {
    for value in [team, project] {
        let id = uuid::Uuid::parse_str(value).map_err(|_| failure("invalid_request"))?;
        if id.is_nil() || id.to_string() != value {
            return Err(failure("invalid_request"));
        }
    }
    Ok(format!(
        "/teams/{team}/projects/{project}/import-v1/{operation}"
    ))
}
#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "Preserve established named Tauri IPC arguments"
)]
pub async fn preview_v1_team_import(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, ImportGate>,
    team_id: String,
    project_id: String,
    session_token: String,
    source_preview_id: String,
    source_plan_sha256: String,
    mapping: Option<Value>,
) -> Result<Value, ApiError> {
    let client = NativeClient::new(&window, "import_connection_failed", "import_uncertain")?;
    let path = team_path(&team_id, &project_id, "preview")?;
    let source = gate.source.try_lock().map_err(|_| failure("import_busy"))?;
    let source = source
        .as_ref()
        .ok_or_else(|| failure("import_preview_required"))?;
    if source["id"].as_str() != Some(&source_preview_id)
        || source["plan_sha256"].as_str() != Some(&source_plan_sha256)
    {
        return Err(failure("import_preview_conflict"));
    }
    let mut held = gate.team.try_lock().map_err(|_| failure("import_busy"))?;
    let result=post_import(&client,&path,serde_json::json!({"source_preview_id":source_preview_id,"source_plan_sha256":source_plan_sha256,"mapping":mapping.unwrap_or_else(||serde_json::json!({}))}),Some(&session_token)).await?;
    *held = Some(result.clone());
    Ok(result)
}
#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "Preserve established named Tauri IPC arguments"
)]
pub async fn commit_v1_team_import(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, ImportGate>,
    team_id: String,
    project_id: String,
    session_token: String,
    preview_id: String,
    plan_sha256: String,
    secrets: Option<Value>,
) -> Result<Value, ApiError> {
    let client = NativeClient::new(&window, "import_connection_failed", "import_uncertain")?;
    let path = team_path(&team_id, &project_id, "commit")?;
    let held = gate.team.try_lock().map_err(|_| failure("import_busy"))?;
    let preview = held
        .as_ref()
        .ok_or_else(|| failure("import_preview_required"))?;
    if preview["id"].as_str() != Some(&preview_id)
        || preview["plan_sha256"].as_str() != Some(&plan_sha256)
        || preview["team_id"].as_str() != Some(&team_id)
        || preview["project_id"].as_str() != Some(&project_id)
    {
        return Err(failure("import_preview_conflict"));
    }
    post_import(
        &client,
        &path,
        serde_json::json!({"preview_id":preview_id,"plan_sha256":plan_sha256,"secrets":secrets.unwrap_or_else(||serde_json::json!({}))}),
        Some(&session_token),
    )
    .await
}
