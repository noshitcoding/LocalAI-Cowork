use super::native_client::NativeClient;
use cowork_contracts::*;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

#[derive(Default)]
pub struct FolderGate(tokio::sync::Mutex<()>);

fn failure(code: &str) -> ApiError {
    ApiError {
        code: code.into(),
        message: "Native folder operation was not confirmed".into(),
        retryable: false,
        correlation_id: uuid::Uuid::new_v4().to_string(),
    }
}

#[tauri::command]
pub async fn list_native_folders(
    window: tauri::WebviewWindow,
    project_id: String,
) -> Result<Vec<RegisteredFolder>, ApiError> {
    let client = NativeClient::new(
        &window,
        "folder_connection_failed",
        "folder_registration_uncertain",
    )?;
    client
        .result(
            client
                .request(reqwest::Method::GET, "/native/folders")
                .query(&[("project_id", project_id)]),
            false,
        )
        .await
}

#[tauri::command]
pub async fn choose_native_folder(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, FolderGate>,
    input: NativeChooseFolderRequest,
) -> Result<NativeFolderOutcome, ApiError> {
    let client = NativeClient::new(
        &window,
        "folder_connection_failed",
        "folder_registration_uncertain",
    )?;
    let _selection = gate
        .0
        .try_lock()
        .map_err(|_| failure("folder_picker_busy"))?;
    cowork_domain::valid_text(&input.idempotency_key, 128)
        .map_err(|_| failure("invalid_request"))?;
    cowork_domain::valid_text(&input.project_id, 128).map_err(|_| failure("invalid_request"))?;
    let project = client
        .result::<Vec<Project>>(client.request(reqwest::Method::GET, "/projects"), false)
        .await?
        .into_iter()
        .find(|p| p.id == input.project_id)
        .ok_or_else(|| failure("not_found"))?;
    let prior: FolderRequestLookup = client
        .result(
            client
                .request(reqwest::Method::GET, "/native/folder-requests")
                .query(&[
                    ("project_id", &input.project_id),
                    ("key", &input.idempotency_key),
                ]),
            false,
        )
        .await?;
    if let Some(folder) = prior.folder {
        if folder.grant.project_id != input.project_id || folder.grant.writable != input.writable {
            return Err(failure("conflict"));
        }
        return Ok(NativeFolderOutcome::Registered { folder });
    }
    let german = matches!(input.language, UiLanguage::De);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .app_handle()
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(if german {
            "Arbeitsordner auswählen"
        } else {
            "Choose a working folder"
        })
        .pick_folder(move |path| {
            let _ = sender.send(path);
        });
    let Some(selected) = receiver
        .await
        .map_err(|_| failure("folder_connection_failed"))?
    else {
        return Ok(NativeFolderOutcome::Canceled);
    };
    let selected = selected.into_path().map_err(|_| failure("forbidden"))?;
    let (canonical, identity) = tauri::async_runtime::spawn_blocking(move || {
        cowork_broker::inspect_native_folder(&selected)
    })
    .await
    .map_err(|_| failure("folder_connection_failed"))?
    .map_err(|_| failure("forbidden"))?;
    let rights = match (german, input.writable) {
        (true, true) => "Lesen und ändern",
        (true, false) => "Nur lesen",
        (false, true) => "Read and change",
        (false, false) => "Read only",
    };
    let project_name: String = project
        .name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}') {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect();
    let message = if german {
        format!(
            "Projekt: {}\nOrdner: {}\nZugriff: {}\n\nDer Ordner wird für dieses Projekt verfügbar. Du wählst ihn anschließend für einen Lauf aus. Gelesene Inhalte können an das von dir gewählte Modell übertragen werden. Bei der Freigabe werden keine Dateien gelesen oder hochgeladen.",
            project_name,
            canonical.display(),
            rights
        )
    } else {
        format!(
            "Project: {}\nFolder: {}\nAccess: {}\n\nThe folder will be available to this project. You select it for a run afterward. Contents read during a run may be sent to your chosen model. Granting access does not read or upload any files.",
            project_name,
            canonical.display(),
            rights
        )
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .app_handle()
        .dialog()
        .message(message)
        .title(if german {
            "Ordnerzugriff freigeben"
        } else {
            "Grant folder access"
        })
        .parent(&window)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            if german { "Freigeben" } else { "Grant access" }.into(),
            if german { "Abbrechen" } else { "Cancel" }.into(),
        ))
        .show(move |confirmed| {
            let _ = sender.send(confirmed);
        });
    if !receiver
        .await
        .map_err(|_| failure("folder_connection_failed"))?
    {
        return Ok(NativeFolderOutcome::Canceled);
    }
    let name = canonical
        .file_name()
        .unwrap_or(canonical.as_os_str())
        .to_string_lossy()
        .chars()
        .take(40)
        .collect::<String>();
    let selection = NativeFolderSelection {
        idempotency_key: input.idempotency_key,
        project_id: input.project_id,
        name,
        path: canonical,
        expected_resource_id: identity,
        writable: input.writable,
    };
    let folder: RegisteredFolder = client.post("/native/folders", &selection).await?;
    if folder.grant.project_id != selection.project_id
        || folder.grant.writable != selection.writable
        || folder.grant.resource_id != selection.expected_resource_id
    {
        return Err(failure("folder_registration_uncertain"));
    }
    Ok(NativeFolderOutcome::Registered { folder })
}

#[tauri::command]
pub async fn revoke_native_folder(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, FolderGate>,
    input: RevokeFolder,
) -> Result<FolderGrant, ApiError> {
    let client = NativeClient::new(
        &window,
        "folder_connection_failed",
        "folder_registration_uncertain",
    )?;
    let _selection = gate
        .0
        .try_lock()
        .map_err(|_| failure("folder_picker_busy"))?;
    client.post("/native/folder-revocations", &input).await
}
