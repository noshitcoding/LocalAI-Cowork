use super::native_client::{NativeClient, failure};
use cowork_broker::delivery::PreparedDelivery;
use cowork_contracts::{ApiError, DownloadMetadata, UiLanguage};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Read;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use zeroize::Zeroizing;

#[derive(Default)]
pub struct RecoveryGate(pub tokio::sync::Mutex<()>);
#[derive(Serialize)]
struct PasswordRequest {
    expected_revision: i64,
    password: Zeroizing<String>,
}
#[derive(Serialize)]
struct RestoreRequest {
    expected_revision: i64,
    password: Zeroizing<String>,
    backup: Value,
}
async fn confirm(
    window: &tauri::WebviewWindow,
    german: bool,
    text_de: &str,
    text_en: &str,
) -> Result<bool, ApiError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .app_handle()
        .dialog()
        .message(if german { text_de } else { text_en })
        .title("LocalAI Cowork V2")
        .parent(window)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancel)
        .show(move |answer| {
            let _ = sender.send(answer);
        });
    receiver
        .await
        .map_err(|_| failure("secrets_connection_failed"))
}
fn private_password(password: String) -> Result<Zeroizing<String>, ApiError> {
    let password = Zeroizing::new(password);
    if password.len() < 12 || password.len() > 4096 {
        return Err(failure("invalid_request"));
    }
    Ok(password)
}
#[tauri::command]
pub async fn native_keys_backup(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, RecoveryGate>,
    expected_revision: i64,
    password: String,
    language: UiLanguage,
) -> Result<Option<Value>, ApiError> {
    let client = NativeClient::new(
        &window,
        "secrets_connection_failed",
        "secrets_setup_uncertain",
    )?;
    let _guard = gate.0.try_lock().map_err(|_| failure("secrets_busy"))?;
    let password = private_password(password)?;
    let german = matches!(language, UiLanguage::De);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .app_handle()
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(if german {
            "Verschlüsselte Schlüsselsicherung speichern"
        } else {
            "Save encrypted key backup"
        })
        .set_file_name("cowork-keys.coworkkeys")
        .add_filter("Cowork encrypted keys", &["coworkkeys"])
        .save_file(move |path| {
            let _ = sender.send(path);
        });
    let Some(selected) = receiver.await.map_err(|_| failure("save_failed"))? else {
        return Ok(None);
    };
    let target = selected
        .into_path()
        .map_err(|_| failure("save_forbidden"))?;
    let backup: Value = client
        .post(
            "/native/secrets/backup",
            &PasswordRequest {
                expected_revision,
                password,
            },
        )
        .await?;
    let bytes = serde_json::to_vec(&backup).map_err(|_| failure("save_failed"))?;
    let metadata = DownloadMetadata {
        file_name: target
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| failure("save_invalid"))?
            .into(),
        size: bytes.len() as u64,
        sha256: cowork_domain::hash(&bytes),
    };
    let protected =
        vec![super::private_data_dir(window.app_handle()).map_err(|_| failure("save_failed"))?];
    let saved = tauri::async_runtime::spawn_blocking(move || {
        PreparedDelivery::prepare(&target, &bytes, &metadata, &protected)?.commit()
    })
    .await
    .map_err(|_| failure("save_uncertain"))?
    .map_err(|_| failure("save_uncertain"))?;
    Ok(Some(json!({"state":"saved","file":saved})))
}
#[tauri::command]
pub async fn native_keys_restore(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, RecoveryGate>,
    expected_revision: i64,
    password: String,
    language: UiLanguage,
) -> Result<Option<Value>, ApiError> {
    let client = NativeClient::new(
        &window,
        "secrets_connection_failed",
        "secrets_setup_uncertain",
    )?;
    let _guard = gate.0.try_lock().map_err(|_| failure("secrets_busy"))?;
    let password = private_password(password)?;
    let german = matches!(language, UiLanguage::De);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .app_handle()
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(if german {
            "Schlüsselsicherung auswählen"
        } else {
            "Select encrypted key backup"
        })
        .add_filter("Cowork encrypted keys", &["coworkkeys"])
        .pick_file(move |path| {
            let _ = sender.send(path);
        });
    let Some(selected) = receiver.await.map_err(|_| failure("save_failed"))? else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|_| failure("invalid_request"))?;
    let backup = tauri::async_runtime::spawn_blocking(move || {
        if !path.is_absolute() {
            return Err(failure("invalid_request"));
        }
        for ancestor in path.ancestors() {
            let metadata =
                std::fs::symlink_metadata(ancestor).map_err(|_| failure("invalid_request"))?;
            if metadata.file_type().is_symlink() {
                return Err(failure("invalid_request"));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(failure("invalid_request"));
                }
            }
        }
        let file = std::fs::File::open(&path).map_err(|_| failure("invalid_request"))?;
        let before = file.metadata().map_err(|_| failure("invalid_request"))?;
        if !before.is_file() || before.len() > 1024 * 1024 {
            return Err(failure("invalid_request"));
        }
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| failure("invalid_request"))?;
        if bytes.len() as u64 != before.len() {
            return Err(failure("invalid_request"));
        }
        serde_json::from_slice::<Value>(&bytes).map_err(|_| failure("invalid_request"))
    })
    .await
    .map_err(|_| failure("invalid_request"))??;
    if !confirm(&window,german,
        "Fehlende ursprüngliche Schlüssel dieser Datenbank aus der Sicherung wiederherstellen? Vorhandene andere Schlüssel werden niemals überschrieben. Die ursprüngliche Datenbank muss bereits wiederhergestellt sein.",
        "Restore missing original keys for this database? Existing different keys are never overwritten. Restore the original database first.").await? {return Ok(None);}
    client
        .post(
            "/native/secrets/restore",
            &RestoreRequest {
                expected_revision,
                password,
                backup,
            },
        )
        .await
        .map(Some)
}
#[tauri::command]
pub async fn native_keys_reencrypt(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, RecoveryGate>,
    expected_revision: i64,
    language: UiLanguage,
) -> Result<Option<Value>, ApiError> {
    let client = NativeClient::new(
        &window,
        "secrets_connection_failed",
        "secrets_setup_uncertain",
    )?;
    let _guard = gate.0.try_lock().map_err(|_| failure("secrets_busy"))?;
    if !confirm(&window,matches!(language,UiLanguage::De),
        "Gespeicherte MCP-Zugangsdaten und Sitzungsspeicher auf den aktiven Schlüssel umschlüsseln? Beende laufende Aufträge vorher. Alte Schlüssel bleiben für Sicherungen erhalten.",
        "Re-encrypt stored MCP credentials and session checkpoints with the active key? Finish active runs first. Old keys remain available for backups.").await? {return Ok(None);}
    client
        .post(
            "/native/secrets/reencrypt",
            &json!({"expected_revision":expected_revision}),
        )
        .await
        .map(Some)
}

#[tauri::command]
pub async fn native_keys_cleanup(window: tauri::WebviewWindow) -> Result<Value, ApiError> {
    let client = NativeClient::new(
        &window,
        "secrets_connection_failed",
        "secrets_setup_uncertain",
    )?;
    client
        .result(
            client.request(reqwest::Method::GET, "/native/secrets/cleanup"),
            false,
        )
        .await
}
#[tauri::command]
pub async fn native_keys_retire_orphan(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, RecoveryGate>,
    expected_revision: i64,
    operation_id: String,
    idempotency_key: String,
    language: UiLanguage,
) -> Result<Option<Value>, ApiError> {
    let client = NativeClient::new(
        &window,
        "secrets_connection_failed",
        "secrets_setup_uncertain",
    )?;
    let _guard = gate.0.try_lock().map_err(|_| failure("secrets_busy"))?;
    if uuid::Uuid::parse_str(&operation_id).is_err()
        || expected_revision < 0
        || idempotency_key.is_empty()
        || idempotency_key.len() > 128
    {
        return Err(failure("invalid_request"));
    }
    let german = matches!(language, UiLanguage::De);
    let de = format!(
        "Den Schlüssel der aufgegebenen Einrichtung {operation_id} dauerhaft entfernen? Er wird ausschließlich zur Bereinigung geprüft und niemals aktiviert. Die App wartet auf eine noch laufende ursprüngliche OS-Einrichtung und prüft alle gespeicherten Datenverweise erneut."
    );
    let en = format!(
        "Permanently remove the key from abandoned setup {operation_id}? The key is inspected only for cleanup and is never activated. The app fences any original OS setup still in progress and rechecks all stored data references."
    );
    if !confirm(&window, german, &de, &en).await? {
        return Ok(None);
    }
    client.post("/native/secrets/orphans/retire",&json!({"expected_revision":expected_revision,"operation_id":operation_id,"idempotency_key":idempotency_key})).await.map(Some)
}
#[tauri::command]
pub async fn native_keys_retire(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, RecoveryGate>,
    expected_revision: i64,
    key: Value,
    idempotency_key: String,
    language: UiLanguage,
) -> Result<Option<Value>, ApiError> {
    let client = NativeClient::new(
        &window,
        "secrets_connection_failed",
        "secrets_setup_uncertain",
    )?;
    let _guard = gate.0.try_lock().map_err(|_| failure("secrets_busy"))?;
    let object = key
        .as_object()
        .filter(|v| v.len() == 2)
        .ok_or_else(|| failure("invalid_request"))?;
    for field in ["instance", "key"] {
        let value = object
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| failure("invalid_request"))?;
        if uuid::Uuid::parse_str(value).is_err() {
            return Err(failure("invalid_request"));
        }
    }
    if !confirm(&window,matches!(language,UiLanguage::De),
        "Diesen unbenutzten früheren Schlüssel dauerhaft aus der nativen Schlüsselablage entfernen? Frühere Datenbanksicherungen benötigen ihre eigene passende Schlüsselsicherung. Die App prüft alle heutigen Datenverweise erneut.",
        "Permanently remove this unused earlier key from native custody? Earlier database backups require their own matching key backup. The app rechecks all current data references.").await? {return Ok(None);}
    client.post("/native/secrets/cleanup",&json!({"expected_revision":expected_revision,"key":key,"idempotency_key":idempotency_key})).await.map(Some)
}
