//! Native operator selection, confirmation and immutable replay of model imports.
use super::native_client::{NativeClient, failure};
use cowork_contracts::{
    ApiError, ImportModelQualification, ModelRegistryRef, ModelRegistryView, UiLanguage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

const MAX_SOURCE: u64 = 60 * 1024 * 1024;
const MAX_RECORD: u64 = 64 * 1024 * 1024;
const MAX_RECEIPTS: usize = 1024;
const MAX_JOURNAL: u64 = 2 * 1024 * 1024 * 1024;
#[derive(Default)]
pub struct RegistryGate(pub tokio::sync::Mutex<()>);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportSelection {
    idempotency_key: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenImport {
    format: u32,
    source_sha256: String,
    request_sha256: String,
    request: ImportModelQualification,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptedImport {
    format: u32,
    request_sha256: String,
    response: ModelRegistryView,
}
fn valid_key(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
pub(crate) fn bundle_identity(bundle: &Value) -> Result<(String, u64), ApiError> {
    let id = bundle["id"]
        .as_str()
        .ok_or_else(|| failure("invalid_request"))?;
    let version = bundle["version"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= i64::MAX as u64)
        .ok_or_else(|| failure("invalid_request"))?;
    if bundle["format"] != 1
        || id.is_empty()
        || id.len() > 512
        || id.chars().any(char::is_control)
        || !super::update_journal::valid_sha(bundle["provider_sha256"].as_str().unwrap_or(""))
        || !bundle["tokenizer_artifact"].is_string()
        || !bundle["chat_template_artifact"].is_string()
        || !bundle["qualification"].is_object()
        || !bundle["evidence"].is_object()
    {
        return Err(failure("invalid_request"));
    }
    Ok((id.into(), version))
}
fn validate_frozen(value: &FrozenImport, key: &str) -> Result<(), ApiError> {
    if value.format != 1
        || value.request.idempotency_key != key
        || !valid_key(key)
        || !super::update_journal::valid_sha(&value.source_sha256)
        || cowork_domain::canonical_hash(&value.request) != value.request_sha256
    {
        return Err(failure("model_registry_receipt_invalid"));
    }
    bundle_identity(&value.request.qualification_bundle)?;
    Ok(())
}
fn validate_reply(frozen: &FrozenImport, value: &ModelRegistryView) -> Result<(), ApiError> {
    let (id, version) = bundle_identity(&frozen.request.qualification_bundle)?;
    if value.binding.id != id
        || value.binding.version != version
        || value.provider_revision <= 0
        || value.provider_sha256
            != frozen.request.qualification_bundle["provider_sha256"]
                .as_str()
                .unwrap_or("")
        || !super::update_journal::valid_sha(&value.binding.sha256)
    {
        return Err(failure("model_registry_operation_uncertain"));
    }
    Ok(())
}
fn ordinary(path: &Path, directory: bool) -> Result<(), ApiError> {
    let meta =
        std::fs::symlink_metadata(path).map_err(|_| failure("model_registry_connection_failed"))?;
    if meta.file_type().is_symlink()
        || meta.is_dir() != directory
        || (!directory && !meta.is_file())
    {
        return Err(failure("model_registry_receipt_invalid"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(failure("model_registry_receipt_invalid"));
        }
    }
    Ok(())
}
/// Keep ancestor identities pinned on Windows; never follow a reparse point.
fn directory_pins(path: &Path) -> Result<Vec<File>, ApiError> {
    if !path.is_absolute() {
        return Err(failure("invalid_request"));
    }
    let mut pins = Vec::new();
    for directory in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        ordinary(directory, true)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
            let file = OpenOptions::new()
                .access_mode(0x0002_0080)
                .share_mode(3)
                .custom_flags(0x0220_0000)
                .open(directory)
                .map_err(|_| failure("model_registry_connection_failed"))?;
            let meta = file
                .metadata()
                .map_err(|_| failure("model_registry_connection_failed"))?;
            if !meta.is_dir() || meta.file_attributes() & 0x400 != 0 {
                return Err(failure("model_registry_receipt_invalid"));
            }
            pins.push(file);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
                .open(directory)
                .map_err(|_| failure("model_registry_connection_failed"))?;
            pins.push(file);
        }
    }
    Ok(pins)
}
pub(crate) fn read_selected(path: &Path, limit: u64, private: bool) -> Result<Vec<u8>, ApiError> {
    let _pins = directory_pins(path.parent().ok_or_else(|| failure("invalid_request"))?)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| failure("model_registry_connection_failed"))?;
    let before = file
        .metadata()
        .map_err(|_| failure("model_registry_connection_failed"))?;
    if !before.is_file() || before.len() > limit {
        return Err(failure("invalid_request"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if before.file_attributes() & 0x400 != 0
            || unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
            || info.nNumberOfLinks != 1
        {
            return Err(failure("model_registry_receipt_invalid"));
        }
        if private {
            super::private_secret_file::validate(&file)
                .map_err(|_| failure("model_registry_receipt_invalid"))?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.nlink() != 1
            || (private
                && (before.uid() != unsafe { libc::geteuid() } || before.mode() & 0o077 != 0))
        {
            return Err(failure("model_registry_receipt_invalid"));
        }
    }
    let mut bytes = Vec::new();
    (&file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| failure("model_registry_connection_failed"))?;
    let after = file
        .metadata()
        .map_err(|_| failure("model_registry_connection_failed"))?;
    if bytes.len() as u64 != before.len()
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(failure("model_registry_receipt_invalid"));
    }
    Ok(bytes)
}
fn journal(data: &Path) -> Result<(PathBuf, Vec<File>, File), ApiError> {
    journal_named(data, "model-registry-imports")
}
pub(crate) fn journal_named(
    data: &Path,
    name: &str,
) -> Result<(PathBuf, Vec<File>, File), ApiError> {
    if !matches!(
        name,
        "model-registry-imports" | "actor-model-registry-imports" | "historical-imports"
    ) {
        return Err(failure("invalid_request"));
    }
    let mut pins = directory_pins(data)?;
    #[cfg(windows)]
    super::private_secret_file::validate_directory(
        pins.last()
            .ok_or_else(|| failure("model_registry_receipt_invalid"))?,
    )
    .map_err(|_| failure("model_registry_data_not_private"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for ancestor in data.ancestors() {
            let meta = std::fs::metadata(ancestor)
                .map_err(|_| failure("model_registry_connection_failed"))?;
            if (meta.uid() != 0 && meta.uid() != unsafe { libc::geteuid() })
                || (meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0)
            {
                return Err(failure("model_registry_receipt_invalid"));
            }
        }
    }
    let folder = data.join(name);
    if !folder
        .try_exists()
        .map_err(|_| failure("model_registry_connection_failed"))?
    {
        #[cfg(windows)]
        let created = super::private_secret_file::create_private_directory(&folder);
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&folder)
        };
        if let Err(e) = created
            && e.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(failure("model_registry_connection_failed"));
        }
    }
    pins.extend(directory_pins(&folder)?);
    #[cfg(windows)]
    super::private_secret_file::validate(
        pins.last()
            .ok_or_else(|| failure("model_registry_receipt_invalid"))?,
    )
    .map_err(|_| failure("model_registry_journal_not_private"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta =
            std::fs::metadata(&folder).map_err(|_| failure("model_registry_connection_failed"))?;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(failure("model_registry_receipt_invalid"));
        }
    }
    let lock_path = folder.join("registry.lock");
    if !lock_path
        .try_exists()
        .map_err(|_| failure("model_registry_connection_failed"))?
    {
        #[cfg(windows)]
        let created = super::private_secret_file::create_private_file(&lock_path);
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&lock_path)
        };
        if let Err(e) = created
            && e.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(failure("model_registry_connection_failed"));
        }
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(3).custom_flags(0x0020_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let lock = options
        .open(&lock_path)
        .map_err(|_| failure("model_registry_connection_failed"))?;
    if !lock
        .metadata()
        .map_err(|_| failure("model_registry_receipt_invalid"))?
        .is_file()
    {
        return Err(failure("model_registry_receipt_invalid"));
    }
    #[cfg(windows)]
    super::private_secret_file::validate(&lock)
        .map_err(|_| failure("model_registry_receipt_invalid"))?;
    lock.try_lock()
        .map_err(|_| failure("model_registry_busy"))?;
    Ok((folder, pins, lock))
}
pub(crate) fn publish(
    folder: &Path,
    path: &Path,
    value: &impl Serialize,
    limit: u64,
) -> Result<(), ApiError> {
    let bytes = serde_json::to_vec(value).map_err(|_| failure("invalid_request"))?;
    if bytes.len() as u64 > limit {
        return Err(failure("invalid_request"));
    }
    let mut total = 0u64;
    let mut requests = 0usize;
    for entry in
        std::fs::read_dir(folder).map_err(|_| failure("model_registry_connection_failed"))?
    {
        let entry = entry.map_err(|_| failure("model_registry_connection_failed"))?;
        ordinary(&entry.path(), false)?;
        let meta = entry
            .metadata()
            .map_err(|_| failure("model_registry_connection_failed"))?;
        total = total
            .checked_add(meta.len())
            .ok_or_else(|| failure("model_registry_retention_limit"))?;
        if entry
            .file_name()
            .to_string_lossy()
            .ends_with(".request.json")
        {
            requests += 1;
        }
    }
    if total
        .checked_add(bytes.len() as u64)
        .is_none_or(|v| v > MAX_JOURNAL)
        || (path.to_string_lossy().ends_with(".request.json") && requests >= MAX_RECEIPTS)
    {
        return Err(failure("model_registry_retention_limit"));
    }
    let temporary = folder.join(format!("publish-{}.tmp", uuid::Uuid::new_v4()));
    #[cfg(windows)]
    let mut file = super::private_secret_file::create_private_file(&temporary)
        .map_err(|_| failure("model_registry_connection_failed"))?;
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(|_| failure("model_registry_connection_failed"))?
    };
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| failure("model_registry_operation_uncertain"))?;
    drop(file);
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                source.as_ptr(),
                target.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(failure("model_registry_operation_uncertain"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(temporary.as_os_str().as_bytes())
            .map_err(|_| failure("invalid_request"))?;
        let target = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| failure("invalid_request"))?;
        if unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(failure("model_registry_operation_uncertain"));
        }
        File::open(folder)
            .and_then(|f| f.sync_all())
            .map_err(|_| failure("model_registry_operation_uncertain"))?;
    }
    Ok(())
}
pub(crate) async fn confirm(
    window: &tauri::WebviewWindow,
    german: bool,
    text: String,
) -> Result<bool, ApiError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .app_handle()
        .dialog()
        .message(text)
        .title(if german {
            "Modellqualifikation bestätigen"
        } else {
            "Confirm model qualification"
        })
        .parent(window)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancel)
        .show(move |value| {
            let _ = sender.send(value);
        });
    receiver
        .await
        .map_err(|_| failure("model_registry_connection_failed"))
}
#[tauri::command]
pub async fn native_model_registry_import(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, RegistryGate>,
    input: ImportSelection,
    language: Option<UiLanguage>,
) -> Result<Option<ModelRegistryView>, ApiError> {
    if !super::application_window(&window) {
        return Err(failure("forbidden"));
    }
    if !valid_key(&input.idempotency_key) {
        return Err(failure("invalid_request"));
    }
    let _gate = gate
        .0
        .try_lock()
        .map_err(|_| failure("model_registry_busy"))?;
    super::connection_info(
        window.app_handle().clone(),
        window.state::<tokio::sync::Mutex<()>>(),
    )
    .await
    .map_err(|_| failure("model_registry_connection_failed"))?;
    let client = NativeClient::new(
        &window,
        "model_registry_connection_failed",
        "model_registry_operation_uncertain",
    )?;
    let data = super::private_data_dir(window.app_handle())
        .map_err(|_| failure("model_registry_connection_failed"))?;
    let (folder, _pins, _lock) = journal(&data)?;
    let path = folder.join(format!("{}.request.json", input.idempotency_key));
    let reply_path = folder.join(format!("{}.result.json", input.idempotency_key));
    let replay = path
        .try_exists()
        .map_err(|_| failure("model_registry_connection_failed"))?;
    let german = matches!(language, Some(UiLanguage::De));
    let frozen: FrozenImport = if replay {
        serde_json::from_slice(&read_selected(&path, MAX_RECORD, true)?)
            .map_err(|_| failure("model_registry_receipt_invalid"))?
    } else {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        window
            .app_handle()
            .dialog()
            .file()
            .set_parent(&window)
            .set_title(if german {
                "Geprüftes Modellqualifikationspaket auswählen"
            } else {
                "Select reviewed model qualification bundle"
            })
            .add_filter("Cowork model qualification", &["json"])
            .pick_file(move |file| {
                let _ = sender.send(file);
            });
        let Some(selected) = receiver
            .await
            .map_err(|_| failure("model_registry_connection_failed"))?
        else {
            return Ok(None);
        };
        let path = selected
            .into_path()
            .map_err(|_| failure("invalid_request"))?;
        let bytes =
            tauri::async_runtime::spawn_blocking(move || read_selected(&path, MAX_SOURCE, false))
                .await
                .map_err(|_| failure("model_registry_connection_failed"))??;
        let bundle: Value =
            serde_json::from_slice(&bytes).map_err(|_| failure("invalid_request"))?;
        bundle_identity(&bundle)?;
        let request = ImportModelQualification {
            idempotency_key: input.idempotency_key.clone(),
            qualification_bundle: bundle,
        };
        FrozenImport {
            format: 1,
            source_sha256: cowork_domain::hash(&bytes),
            request_sha256: cowork_domain::canonical_hash(&request),
            request,
        }
    };
    validate_frozen(&frozen, &input.idempotency_key)?;
    if reply_path
        .try_exists()
        .map_err(|_| failure("model_registry_connection_failed"))?
    {
        let receipt: AcceptedImport =
            serde_json::from_slice(&read_selected(&reply_path, 64 * 1024, true)?)
                .map_err(|_| failure("model_registry_receipt_invalid"))?;
        if receipt.format != 1 || receipt.request_sha256 != frozen.request_sha256 {
            return Err(failure("model_registry_receipt_invalid"));
        }
        validate_reply(&frozen, &receipt.response)?;
        return Ok(Some(receipt.response));
    }
    let (id, version) = bundle_identity(&frozen.request.qualification_bundle)?;
    let text = if german {
        format!(
            "Betreiberqualifikation {id} · v{version}\nDatei-SHA256: {}\n\nBestätige die unabhängig geprüften Modell-, Tokenizer-, Deployment- und Referenzbelege. Die Anwendung prüft Artefakte und Parität; sie erfindet keine Qualifikation. Neue Läufe müssen diese Version ausdrücklich wählen.{}",
            frozen.source_sha256,
            if replay {
                "\n\nDies setzt ausschließlich den gespeicherten Import mit identischen Originaldaten fort."
            } else {
                ""
            }
        )
    } else {
        format!(
            "Operator qualification {id} · v{version}\nFile SHA256: {}\n\nConfirm independently reviewed model, tokenizer, deployment and reference evidence. The application checks artifacts and parity; it does not invent qualification. New runs must explicitly select this version.{}",
            frozen.source_sha256,
            if replay {
                "\n\nThis continues only the recorded import with identical original data."
            } else {
                ""
            }
        )
    };
    if !confirm(&window, german, text).await? {
        return if replay {
            Err(failure("model_registry_retry_cancelled"))
        } else {
            Ok(None)
        };
    }
    if !replay {
        publish(&folder, &path, &frozen, MAX_RECORD)?;
    }
    let response: ModelRegistryView = client
        .result(
            client
                .request(reqwest::Method::POST, "/native-model-registry/import")
                .timeout(std::time::Duration::from_secs(60))
                .header("content-type", "application/json")
                .body(serde_json::to_vec(&frozen.request).map_err(|_| failure("invalid_request"))?),
            true,
        )
        .await?;
    validate_reply(&frozen, &response)?;
    publish(
        &folder,
        &reply_path,
        &AcceptedImport {
            format: 1,
            request_sha256: frozen.request_sha256,
            response: response.clone(),
        },
        64 * 1024,
    )?;
    Ok(Some(response))
}
#[tauri::command]
pub async fn native_model_registry_revoke(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, RegistryGate>,
    input: ModelRegistryRef,
    language: Option<UiLanguage>,
) -> Result<Value, ApiError> {
    if !super::application_window(&window) {
        return Err(failure("forbidden"));
    }
    if input.id.is_empty()
        || input.id.len() > 512
        || input.id.chars().any(char::is_control)
        || input.version == 0
        || input.version > i64::MAX as u64
        || !super::update_journal::valid_sha(&input.sha256)
    {
        return Err(failure("invalid_request"));
    }
    let _gate = gate
        .0
        .try_lock()
        .map_err(|_| failure("model_registry_busy"))?;
    super::connection_info(
        window.app_handle().clone(),
        window.state::<tokio::sync::Mutex<()>>(),
    )
    .await
    .map_err(|_| failure("model_registry_connection_failed"))?;
    let client = NativeClient::new(
        &window,
        "model_registry_connection_failed",
        "model_registry_operation_uncertain",
    )?;
    let german = matches!(language, Some(UiLanguage::De));
    let text = if german {
        format!(
            "Modellqualifikation {} · v{}\nSHA256: {}\n\nDiese exakte Version für neue Modellanfragen widerrufen? Ursprüngliche Lauf- und Importbelege bleiben erhalten.",
            input.id, input.version, input.sha256
        )
    } else {
        format!(
            "Model qualification {} · v{}\nSHA256: {}\n\nRevoke this exact version for new model requests? Original run and import records remain preserved.",
            input.id, input.version, input.sha256
        )
    };
    if !confirm(&window, german, text).await? {
        return Err(failure("model_registry_retry_cancelled"));
    }
    client.post("/native-model-registry/revoke", &input).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn frozen(key: &str) -> FrozenImport {
        let request = ImportModelQualification {
            idempotency_key: key.into(),
            qualification_bundle: json!({
                "format":1,"id":"operator-model","version":1,"provider_sha256":"a".repeat(64),
                "tokenizer_artifact":"{}","chat_template_artifact":"{}","qualification":{},"evidence":{}
            }),
        };
        FrozenImport {
            format: 1,
            source_sha256: "b".repeat(64),
            request_sha256: cowork_domain::canonical_hash(&request),
            request,
        }
    }
    #[test]
    fn immutable_import_survives_restart_and_refuses_replacement_or_wrong_binding() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let data = {
            let data = directory.path().join("private-native-data");
            super::super::private_secret_file::create_private_directory(&data).unwrap();
            data
        };
        #[cfg(unix)]
        let data = directory.path().to_path_buf();
        let key = uuid::Uuid::new_v4().to_string();
        let original = frozen(&key);
        let path;
        {
            let (folder, _pins, _lock) = journal(&data).unwrap();
            path = folder.join(format!("{key}.request.json"));
            publish(&folder, &path, &original, MAX_RECORD).unwrap();
            let second = journal(&data);
            assert_eq!(second.err().unwrap().code, "model_registry_busy");
        }
        let (folder, _pins, _lock) = journal(&data).unwrap();
        let bytes = read_selected(&path, MAX_RECORD, true).unwrap();
        let loaded: FrozenImport = serde_json::from_slice(&bytes).unwrap();
        validate_frozen(&loaded, &key).unwrap();
        assert_eq!(
            serde_json::to_vec(&loaded.request).unwrap(),
            serde_json::to_vec(&original.request).unwrap()
        );
        assert!(validate_frozen(&loaded, &uuid::Uuid::new_v4().to_string()).is_err());
        let mut altered = frozen(&key);
        altered.request.qualification_bundle["version"] = json!(2);
        assert!(validate_frozen(&altered, &key).is_err());
        assert!(publish(&folder, &path, &altered, MAX_RECORD).is_err());
        assert_eq!(read_selected(&path, MAX_RECORD, true).unwrap(), bytes);
    }
    #[test]
    fn file_reader_refuses_hardlinks_and_limits_import_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.json");
        std::fs::write(&source, b"123456789").unwrap();
        assert!(read_selected(&source, 8, false).is_err());
        std::fs::hard_link(&source, directory.path().join("alias.json")).unwrap();
        assert!(read_selected(&source, MAX_SOURCE, false).is_err());
    }
}
