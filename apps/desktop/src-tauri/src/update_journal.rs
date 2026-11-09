//! Immutable, bounded update receipts. A GUI restart never clears an unknown effect.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const FORMAT: u32 = 3;
const MAX_RECORD: u64 = 64 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub format: u32,
    pub attempt: String,
    pub update_id: String,
    pub from: String,
    pub to: String,
    pub payload_sha256: String,
    pub signature_verified: bool,
    pub daemon: super::updates::Lifecycle,
    pub state: String,
}
impl Attempt {
    pub fn validate(&self) -> Result<(), String> {
        if self.format != FORMAT
            || !valid_id(&self.attempt)
            || !valid_sha(&self.update_id)
            || !self.attempt.starts_with(&format!("{}-", self.update_id))
            || !valid_sha(&self.payload_sha256)
            || !self.signature_verified
            || self.from != self.daemon.version
            || self.to.is_empty()
            || self.to.len() > 128
            || self.state != "shutdown_prepared"
            || !self.daemon.valid()
        {
            return Err("update_receipt_invalid_or_legacy".into());
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phase {
    format: u32,
    attempt: String,
    attempt_sha256: String,
    state: String,
}
pub fn valid_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn valid_id(value: &str) -> bool {
    value.len() == 101
        && value.is_ascii()
        && valid_sha(&value[..64])
        && value.as_bytes()[64] == b'-'
        && uuid::Uuid::parse_str(&value[65..])
            .is_ok_and(|id| !id.is_nil() && id.to_string() == value[65..])
}
fn checked_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options
        .open(path)
        .map_err(|_| "update_receipt_read_failed")?;
    let metadata = file.metadata().map_err(|_| "update_receipt_read_failed")?;
    if !metadata.is_file() || metadata.len() > MAX_RECORD || metadata.file_type().is_symlink() {
        return Err("update_receipt_invalid_or_legacy".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err("update_receipt_invalid_or_legacy".into());
        }
    }
    Ok(file)
}
pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    checked_file(path)?
        .take(MAX_RECORD + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "update_receipt_read_failed")?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err("update_receipt_invalid_or_legacy".into());
    }
    Ok(bytes)
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn folder(data: &Path) -> Result<PathBuf, String> {
    let path = data.join("update-receipts");
    std::fs::create_dir_all(&path).map_err(|_| "update_receipt_directory_failed")?;
    if std::fs::symlink_metadata(&path)
        .map_err(|_| "update_receipt_directory_failed")?
        .file_type()
        .is_symlink()
    {
        return Err("update_receipt_directory_failed".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "update_receipt_directory_failed")?;
    }
    path.canonicalize()
        .map_err(|_| "update_receipt_directory_failed".into())
}
pub fn lock(folder: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options
        .open(folder.join("update.lock"))
        .map_err(|_| "update_journal_lock_failed")?;
    if !file
        .metadata()
        .map_err(|_| "update_journal_lock_failed")?
        .is_file()
    {
        return Err("update_journal_lock_failed".into());
    }
    file.try_lock()
        .map_err(|_| "another_update_operation_is_active")?;
    Ok(file)
}
pub fn record(path: &Path, value: &impl Serialize) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "update_receipt_encode_failed")?;
    publish(path, &bytes, MAX_RECORD)
}
/// Publish an owned, complete payload without replacing any existing bytes.
pub fn publish(path: &Path, bytes: &[u8], maximum: u64) -> Result<String, String> {
    if bytes.len() as u64 > maximum {
        return Err("update_receipt_too_large".into());
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let folder = path.parent().ok_or("update_receipt_write_failed")?;
    let temporary = folder.join(format!(".record-{}.tmp", uuid::Uuid::new_v4()));
    let mut file = options
        .open(&temporary)
        .map_err(|_| "update_receipt_write_failed")?;
    file.write_all(bytes)
        .map_err(|_| "update_receipt_write_failed")?;
    file.sync_all().map_err(|_| "update_receipt_write_failed")?;
    drop(file);
    // Publish complete bytes atomically, without ever replacing an old receipt.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(temporary.as_os_str().as_bytes())
            .map_err(|_| "update_receipt_write_failed")?;
        let target = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "update_receipt_write_failed")?;
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
            return Err("update_receipt_write_failed".into());
        }
        File::open(folder)
            .and_then(|d| d.sync_all())
            .map_err(|_| "update_receipt_write_failed")?;
    }
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
            return Err("update_receipt_write_failed".into());
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        return Err("unsupported_update_platform".into());
    }
    Ok(hash(bytes))
}
pub fn load(folder: &Path, id: &str) -> Result<(Attempt, String), String> {
    if !valid_id(id) {
        return Err("update_attempt_invalid".into());
    }
    let bytes = read(&folder.join(format!("{id}.json")))?;
    let attempt: Attempt =
        serde_json::from_slice(&bytes).map_err(|_| "update_receipt_invalid_or_legacy")?;
    attempt.validate()?;
    if attempt.attempt != id {
        return Err("update_receipt_binding_changed".into());
    }
    Ok((attempt, hash(&bytes)))
}
fn phase_path(folder: &Path, id: &str, state: &str) -> PathBuf {
    folder.join(format!("{id}.{state}.json"))
}
pub fn has_phase(folder: &Path, id: &str, digest: &str, state: &str) -> Result<bool, String> {
    let path = phase_path(folder, id, state);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err("update_receipt_read_failed".into()),
        Ok(_) => {}
    }
    let phase: Phase =
        serde_json::from_slice(&read(&path)?).map_err(|_| "update_receipt_binding_changed")?;
    if phase.format != 1
        || phase.attempt != id
        || phase.attempt_sha256 != digest
        || phase.state != state
    {
        return Err("update_receipt_binding_changed".into());
    }
    Ok(true)
}
pub fn phase(folder: &Path, id: &str, digest: &str, state: &str) -> Result<(), String> {
    if has_phase(folder, id, digest, state)? {
        return Ok(());
    }
    record(
        &phase_path(folder, id, state),
        &Phase {
            format: 1,
            attempt: id.into(),
            attempt_sha256: digest.into(),
            state: state.into(),
        },
    )
    .map(|_| ())
}
#[derive(Serialize)]
pub struct PendingAttempt {
    pub attempt: String,
    pub from: String,
    pub to: String,
    pub state: String,
    pub can_reconcile: bool,
}
pub fn pending(folder: &Path) -> Result<Vec<PendingAttempt>, String> {
    let mut values = Vec::new();
    let mut count = 0;
    for entry in std::fs::read_dir(folder).map_err(|_| "update_receipt_read_failed")? {
        let entry = entry.map_err(|_| "update_receipt_read_failed")?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("update_receipt_invalid_or_legacy")?;
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        // Phase records contain another dot. Every original record must validate.
        if id.contains('.') {
            continue;
        }
        count += 1;
        if count > 4096 {
            return Err("update_journal_archive_required".into());
        }
        let (attempt, digest) = load(folder, id)?;
        let stopped = has_phase(folder, id, &digest, "daemon-stopped")?;
        let installed = has_phase(folder, id, &digest, "install-requested")?;
        let failed = has_phase(folder, id, &digest, "installer-failed")?;
        let rejected = has_phase(folder, id, &digest, "shutdown-rejected")?;
        let undispatched = has_phase(folder, id, &digest, "shutdown-not-dispatched")?;
        let resolved = has_phase(folder, id, &digest, "reconciled-without-replay")?;
        let applied = has_phase(folder, id, &digest, "install-applied")?;
        #[cfg(windows)]
        let (mut failed, mut applied) = (failed, applied);
        #[cfg(windows)]
        {
            let outcome = super::update_installer::outcome(folder, id, &digest)?;
            if let Some(outcome) = outcome {
                if (failed && outcome.successful()) || (applied && !outcome.successful()) {
                    return Err("update_receipt_binding_changed".into());
                }
                // The immutable native exit result remains authoritative even if
                // the observer dies before writing its following convenience phase.
                applied |= outcome.successful();
                failed |= !outcome.successful();
            } else if applied {
                return Err("installer_outcome_unconfirmed".into());
            }
        }
        if (installed && !stopped)
            || (failed && !installed)
            || (resolved && (!stopped || (installed && !failed)))
            || (applied && !installed)
            || ((rejected || undispatched) && installed)
        {
            return Err("update_receipt_binding_changed".into());
        }
        if rejected
            || undispatched
            || resolved
            || (applied && attempt.to == env!("CARGO_PKG_VERSION"))
        {
            continue;
        }
        let state = if applied {
            "installed-target-version-not-running"
        } else if installed && !failed {
            "installer-outcome-unconfirmed"
        } else if failed {
            "installer-failed"
        } else if stopped {
            "daemon-stopped-install-not-started"
        } else {
            "original-daemon-exit-unconfirmed"
        };
        values.push(PendingAttempt {
            attempt: id.into(),
            from: attempt.from,
            to: attempt.to,
            state: state.into(),
            can_reconcile: stopped && (!installed || failed),
        });
    }
    values.sort_by(|a, b| a.attempt.cmp(&b.attempt));
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("cowork-update-journal-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only the exact new fixture's regular entries are removed.
            if let Ok(entries) = std::fs::read_dir(&self.0) {
                for entry in entries.flatten() {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            let _ = std::fs::remove_dir(&self.0);
        }
    }
    fn attempt() -> Attempt {
        let update_id = "a".repeat(64);
        Attempt { format:FORMAT,attempt:format!("{update_id}-{}",uuid::Uuid::new_v4()),update_id,
            from:env!("CARGO_PKG_VERSION").into(),to:"2.0.0-alpha.6".into(),payload_sha256:"b".repeat(64),signature_verified:true,
            daemon:serde_json::from_value(serde_json::json!({"format":1,"product":"LocalAI Cowork V2","version":env!("CARGO_PKG_VERSION"),"schema_version":8,"pid":4321,"instance_id":uuid::Uuid::new_v4().to_string(),"closing":false})).unwrap(),
            state:"shutdown_prepared".into() }
    }
    #[test]
    fn original_attempt_survives_restart_and_requires_bound_exit_proof() {
        let fixture = Fixture::new();
        let value = attempt();
        let digest = record(&fixture.0.join(format!("{}.json", value.attempt)), &value).unwrap();
        let original = pending(&fixture.0).unwrap();
        assert_eq!(original.len(), 1);
        assert!(!original[0].can_reconcile);
        phase(&fixture.0, &value.attempt, &digest, "daemon-stopped").unwrap();
        assert!(pending(&fixture.0).unwrap()[0].can_reconcile);
        phase(
            &fixture.0,
            &value.attempt,
            &digest,
            "reconciled-without-replay",
        )
        .unwrap();
        assert!(pending(&fixture.0).unwrap().is_empty());
        assert!(!has_phase(&fixture.0, &value.attempt, &digest, "install-requested").unwrap());
    }
    #[test]
    fn unknown_installer_never_releases_gate_and_receipts_cannot_be_replaced() {
        let fixture = Fixture::new();
        let value = attempt();
        let path = fixture.0.join(format!("{}.json", value.attempt));
        let digest = record(&path, &value).unwrap();
        assert!(record(&path, &value).is_err());
        assert_eq!(load(&fixture.0, &value.attempt).unwrap().1, digest);
        phase(&fixture.0, &value.attempt, &digest, "daemon-stopped").unwrap();
        phase(&fixture.0, &value.attempt, &digest, "install-requested").unwrap();
        let current = pending(&fixture.0).unwrap();
        assert_eq!(current[0].state, "installer-outcome-unconfirmed");
        assert!(!current[0].can_reconcile);
        phase(
            &fixture.0,
            &value.attempt,
            &digest,
            "reconciled-without-replay",
        )
        .unwrap();
        assert!(pending(&fixture.0).is_err());
    }
    #[test]
    fn legacy_and_changed_proofs_block_and_unicode_ids_never_panic() {
        assert!(!valid_id(&format!(
            "{}-{}",
            "é".repeat(32),
            uuid::Uuid::new_v4()
        )));
        let fixture = Fixture::new();
        let value = attempt();
        let digest = record(&fixture.0.join(format!("{}.json", value.attempt)), &value).unwrap();
        phase(
            &fixture.0,
            &value.attempt,
            &"c".repeat(64),
            "daemon-stopped",
        )
        .unwrap();
        assert!(pending(&fixture.0).is_err());
        assert!(has_phase(&fixture.0, &value.attempt, &digest, "daemon-stopped").is_err());
        let legacy = Fixture::new();
        record(
            &legacy.0.join(format!("{}.json", value.attempt)),
            &serde_json::json!({"format":2}),
        )
        .unwrap();
        assert!(pending(&legacy.0).is_err());
    }
}
