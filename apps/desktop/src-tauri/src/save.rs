use cowork_broker::delivery::{PreparedDelivery, validate_delivery};
use cowork_contracts::{
    ApiError, MAX_DOWNLOAD_BYTES, NativeSaveOutcome, NativeSaveRequest, UiLanguage,
};
use cowork_domain::Error;
use tauri::{
    Manager,
    ipc::{InvokeBody, Request},
};
use tauri_plugin_dialog::DialogExt;

#[derive(Default)]
pub struct SaveGate(pub(crate) tokio::sync::Mutex<()>);

fn failure(code: &str) -> ApiError {
    ApiError {
        code: code.into(),
        message: "Native file delivery was not confirmed".into(),
        retryable: false,
        correlation_id: uuid::Uuid::new_v4().to_string(),
    }
}
fn delivery_failure(error: Error) -> ApiError {
    failure(match error {
        Error::Forbidden => "save_forbidden",
        Error::Conflict | Error::FileConflict => "save_conflict",
        Error::UncertainEffect => "save_uncertain",
        Error::Validation(_) => "save_invalid",
        _ => "save_failed",
    })
}

#[tauri::command]
pub async fn save_file(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, SaveGate>,
    request: Request<'_>,
) -> Result<NativeSaveOutcome, ApiError> {
    // Only the local application window may request the user-mediated save dialog.
    if !super::application_window(&window) {
        return Err(failure("save_forbidden"));
    }
    let _saving = gate.0.try_lock().map_err(|_| failure("save_busy"))?;
    let header = request
        .headers()
        .get("x-cowork-save")
        .and_then(|h| h.to_str().ok())
        .filter(|h| h.len() <= 8192)
        .ok_or_else(|| failure("save_invalid"))?;
    let json = percent_encoding::percent_decode_str(header)
        .decode_utf8()
        .map_err(|_| failure("save_invalid"))?;
    let input: NativeSaveRequest =
        serde_json::from_str(&json).map_err(|_| failure("save_invalid"))?;
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(failure("save_invalid"));
    };
    if bytes.len() as u64 > MAX_DOWNLOAD_BYTES || bytes.len() as u64 != input.file.size {
        return Err(failure("save_invalid"));
    }
    let bytes = bytes.clone();
    let metadata = input.file;
    let (bytes, metadata) = tauri::async_runtime::spawn_blocking(move || {
        validate_delivery(&bytes, &metadata)?;
        Ok::<_, Error>((bytes, metadata))
    })
    .await
    .map_err(|_| failure("save_failed"))?
    .map_err(delivery_failure)?;
    let title = match input.language {
        UiLanguage::De => "Datei speichern",
        UiLanguage::En => "Save file",
    };
    let app = window.app_handle();
    let protected = vec![super::private_data_dir(app).map_err(|_| failure("save_failed"))?];
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_parent(&window)
        .set_title(title)
        .set_file_name(&metadata.file_name)
        .save_file(move |path| {
            let _ = sender.send(path);
        });
    let Some(selected) = receiver.await.map_err(|_| failure("save_failed"))? else {
        return Ok(NativeSaveOutcome::Canceled);
    };
    let target = selected
        .into_path()
        .map_err(|_| failure("save_forbidden"))?;
    let saved = tauri::async_runtime::spawn_blocking(move || {
        PreparedDelivery::prepare(&target, &bytes, &metadata, &protected)?.commit()
    })
    .await
    .map_err(|_| failure("save_uncertain"))?
    .map_err(delivery_failure)?;
    Ok(NativeSaveOutcome::Saved { file: saved })
}
