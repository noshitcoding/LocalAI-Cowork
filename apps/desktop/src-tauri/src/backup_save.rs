//! Fixed local owner backup route; no renderer endpoint, token or ambient path input.
use super::native_client::{NativeClient, failure};
use cowork_contracts::{
    ApiError, DatabaseBackupSaveOutcome, DownloadMetadata, MAX_DATABASE_BACKUP_BYTES,
    MaintenanceStatus, NativeSaveDatabaseBackup, UiLanguage,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use tokio::io::AsyncWriteExt;

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn safe_private(path: &Path) -> Result<(), ApiError> {
    for ancestor in path.ancestors() {
        let meta =
            std::fs::symlink_metadata(ancestor).map_err(|_| failure("backup_save_failed"))?;
        if meta.is_symlink() {
            return Err(failure("backup_save_forbidden"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(failure("backup_save_forbidden"));
            }
        }
    }
    Ok(())
}
fn valid(input: &NativeSaveDatabaseBackup) -> bool {
    uuid::Uuid::parse_str(&input.id).is_ok_and(|id| id.to_string() == input.id && !id.is_nil())
        && input.expected_bytes > 0
        && input.expected_bytes <= MAX_DATABASE_BACKUP_BYTES
        && input.expected_sha256.len() == 64
        && input
            .expected_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
#[tauri::command]
pub async fn save_database_backup(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, super::save::SaveGate>,
    input: NativeSaveDatabaseBackup,
) -> Result<DatabaseBackupSaveOutcome, ApiError> {
    let client = NativeClient::new(&window, "backup_download_failed", "backup_save_uncertain")?;
    if !valid(&input) {
        return Err(failure("backup_save_invalid"));
    }
    let _saving = gate.0.try_lock().map_err(|_| failure("save_busy"))?;
    let status: MaintenanceStatus = client
        .result(client.request(reqwest::Method::GET, "/maintenance"), false)
        .await?;
    let backup = status
        .backups
        .into_iter()
        .find(|item| item.id == input.id)
        .ok_or_else(|| failure("backup_not_found"))?;
    if backup.bytes != input.expected_bytes
        || backup.sha256 != input.expected_sha256
        || !matches!(backup.format.as_str(), "sqlite3" | "postgres-custom")
    {
        return Err(failure("backup_changed"));
    }
    let file_name = format!("cowork-{}.{}", backup.id, backup.format);
    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(if matches!(input.language, UiLanguage::De) {
            "Datenbankbackup speichern"
        } else {
            "Save database backup"
        })
        .set_file_name(&file_name)
        .save_file(move |path| {
            let _ = send.send(path);
        });
    let Some(selected) = receive.await.map_err(|_| failure("backup_save_failed"))? else {
        return Ok(DatabaseBackupSaveOutcome::Canceled);
    };
    let target = selected
        .into_path()
        .map_err(|_| failure("backup_save_forbidden"))?;
    let private =
        super::private_data_dir(window.app_handle()).map_err(|_| failure("backup_save_failed"))?;
    safe_private(&private)?;
    let temporary = Temporary(private.join(format!(
        ".cowork-backup-download-{}.part",
        uuid::Uuid::new_v4()
    )));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let std_file = options
        .open(&temporary.0)
        .map_err(|_| failure("backup_save_failed"))?;
    let mut output = tokio::fs::File::from_std(std_file);
    let mut response = client
        .request(
            reqwest::Method::GET,
            &format!("/maintenance/backups/{}", backup.id),
        )
        .timeout(std::time::Duration::from_secs(3600))
        .send()
        .await
        .map_err(|_| failure("backup_download_failed"))?;
    if !response.status().is_success()
        || response.content_length() != Some(backup.bytes)
        || response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            != Some("application/octet-stream")
        || response
            .headers()
            .get("x-cowork-sha256")
            .and_then(|h| h.to_str().ok())
            != Some(backup.sha256.as_str())
        || response
            .headers()
            .get("x-cowork-file-name")
            .and_then(|h| h.to_str().ok())
            != Some(file_name.as_str())
    {
        return Err(failure("backup_integrity_failed"));
    }
    let mut hash = Sha256::new();
    let mut size = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("backup_download_failed"))?
    {
        size = size
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| failure("backup_integrity_failed"))?;
        if size > backup.bytes {
            return Err(failure("backup_integrity_failed"));
        }
        hash.update(&chunk);
        output
            .write_all(&chunk)
            .await
            .map_err(|_| failure("backup_save_failed"))?;
    }
    output
        .sync_all()
        .await
        .map_err(|_| failure("backup_save_failed"))?;
    drop(output);
    if size != backup.bytes || format!("{:x}", hash.finalize()) != backup.sha256 {
        return Err(failure("backup_integrity_failed"));
    }
    let metadata = DownloadMetadata {
        file_name,
        size,
        sha256: backup.sha256.clone(),
    };
    let saved = tauri::async_runtime::spawn_blocking(move || {
        let _cleanup = temporary;
        let mut source =
            std::fs::File::open(&_cleanup.0).map_err(|_| cowork_domain::Error::NotFound)?;
        cowork_broker::delivery::PreparedDelivery::prepare_backup(
            &target,
            &mut source,
            &metadata,
            &[private],
        )
        .and_then(|delivery| delivery.commit())
    })
    .await
    .map_err(|_| failure("backup_save_uncertain"))?
    .map_err(|error| {
        failure(if matches!(error, cowork_domain::Error::UncertainEffect) {
            "backup_save_uncertain"
        } else {
            "backup_save_failed"
        })
    })?;
    Ok(DatabaseBackupSaveOutcome::Saved {
        bytes: saved.size,
        sha256: saved.sha256,
    })
}
