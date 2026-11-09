//! Single owner of the audit lock, classification, append WAL and recovery journal.
//! No unverified event content or filesystem paths cross the diagnostic boundary.
use super::*;
use fs2::FileExt;
use sha2::Digest;
use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::sync::MutexGuard;
use std::time::{Duration, Instant};

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const RECOVERY_JOURNAL: &str = "recovery-active.json";
const PENDING: &str = "append-pending.json";
const ROTATION: &str = "rotation-active.json";

#[cfg(test)]
thread_local! { static FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }

fn checkpoint(_phase: &'static str) -> Result<(), String> {
    #[cfg(test)]
    if FAULT.with(|fault| {
        if fault.get() == Some(_phase) {
            fault.set(None);
            true
        } else {
            false
        }
    }) {
        return Err("audit_injected_interruption".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditIntegrityReportV2 {
    pub version: u32,
    pub status: String,
    pub trust_level: String,
    pub signed_events: u64,
    pub legacy_events: u64,
    pub malformed_events: u64,
    pub files_checked: usize,
    pub first_problem_line: Option<u64>,
    pub first_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
    pub chain_complete: bool,
    pub error_code: Option<String>,
    pub assessment_id: Option<String>,
    pub recovery_allowed: bool,
    pub recovery: Option<RecoveryMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryMetadata {
    pub recovery_id: String,
    pub manifest_sha256: String,
    pub files_preserved: usize,
    pub bytes_preserved: u64,
    pub prior_trust: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreservedFile {
    name: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryJournal {
    schema_version: u32,
    recovery_id: String,
    assessment_id: String,
    phase: String,
    prior_trust: String,
    legacy_events: u64,
    malformed_events: u64,
    files: Vec<PreservedFile>,
    manifest_sha256: Option<String>,
    new_chain: Option<String>,
    mac: String,
}

fn recovery_mac(journal: &RecoveryJournal, key: &[u8; 32]) -> Result<String, String> {
    let mut unsigned = journal.clone();
    unsigned.mac.clear();
    let bytes = serde_json::to_vec(&("audit-recovery-v1", unsigned)).map_err(io_error)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(io_error)?;
    mac.update(&bytes);
    Ok(encode_hex(&mac.finalize().into_bytes()))
}

fn write_recovery(
    path: &Path,
    journal: &mut RecoveryJournal,
    key: &[u8; 32],
) -> Result<(), String> {
    journal.mac = recovery_mac(journal, key)?;
    write_json(path, journal)
}

struct Assessment {
    report: AuditIntegrityReportV2,
    files: Vec<PreservedFile>,
    marker: bool,
}

struct LedgerGuard {
    file: File,
    _process: MutexGuard<'static, ()>,
}

impl Drop for LedgerGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn acquire(root: &Path, timeout: Duration) -> Result<LedgerGuard, String> {
    let started = Instant::now();
    let mutex = AUDIT_WRITE_LOCK.get_or_init(|| Mutex::new(()));
    let process = loop {
        match mutex.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err("audit_lock_unavailable".into())
            }
            Err(_) if started.elapsed() >= timeout => return Err("audit_lock_busy".into()),
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let dir = root.join("audit");
    create_dir_all(&dir).map_err(|_| AUDIT_STORAGE_FAILED.to_string())?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("ledger.lock"))
        .map_err(|_| "audit_lock_unavailable".to_string())?;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => {
                return Ok(LedgerGuard {
                    file,
                    _process: process,
                })
            }
            Err(error)
                if error.kind() != std::io::ErrorKind::WouldBlock
                    && error.raw_os_error() != fs2::lock_contended_error().raw_os_error() =>
            {
                return Err("audit_lock_unavailable".into())
            }
            Err(_) if started.elapsed() >= timeout => return Err("audit_lock_busy".into()),
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn sha256(bytes: &[u8]) -> String {
    encode_hex(&Sha256::digest(bytes))
}
fn io_error(_: impl std::fmt::Display) -> String {
    AUDIT_STORAGE_FAILED.to_string()
}

fn sync_directory(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(dir)
        .and_then(|file| file.sync_all())
        .map_err(io_error)?;
    #[cfg(not(unix))]
    let _ = dir; // Windows rename below uses MOVEFILE_WRITE_THROUGH.
    Ok(())
}

fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let from = source
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let to = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(io_error(std::io::Error::last_os_error()));
        }
    }
    #[cfg(not(windows))]
    fs::rename(source, destination).map_err(io_error)?;
    sync_directory(destination.parent().ok_or(AUDIT_STORAGE_FAILED)?)
}

fn durable_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_file_name(format!(".audit-write-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .map_err(io_error)?;
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        drop(file);
        replace_file(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    durable_write(path, &serde_json::to_vec(value).map_err(io_error)?)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&fs::read(path).map_err(io_error)?)
        .map_err(|_| "audit_journal_invalid".into())
}

fn remove_synced(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => sync_directory(path.parent().ok_or(AUDIT_STORAGE_FAILED)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

fn valid_event_name(name: &str) -> bool {
    name == "events.jsonl"
        || (1..=MAX_AUDIT_ARCHIVES).any(|index| name == format!("events.{index}.jsonl"))
}

fn validate_files(files: &[PreservedFile]) -> Result<(), String> {
    let mut names = std::collections::HashSet::new();
    if files.len() > MAX_AUDIT_ARCHIVES + 1
        || files.iter().any(|file| {
            !valid_event_name(&file.name)
                || decode_mac(&file.sha256).is_none()
                || !names.insert(&file.name)
        })
    {
        return Err("audit_journal_invalid".into());
    }
    Ok(())
}

fn checked_bytes(root: &Path, file: &PreservedFile) -> Result<Vec<u8>, String> {
    let bytes = fs::read(root.join(&file.name)).map_err(io_error)?;
    if bytes.len() as u64 != file.size || sha256(&bytes) != file.sha256 {
        return Err("audit_recovery_source_changed".into());
    }
    Ok(bytes)
}

fn empty_report() -> AuditIntegrityReportV2 {
    AuditIntegrityReportV2 {
        version: 2,
        status: "empty".into(),
        trust_level: "verified".into(),
        signed_events: 0,
        legacy_events: 0,
        malformed_events: 0,
        files_checked: 0,
        first_problem_line: None,
        first_sequence: None,
        last_sequence: None,
        chain_complete: true,
        error_code: None,
        assessment_id: None,
        recovery_allowed: false,
        recovery: None,
    }
}

fn fail_report(code: String) -> AuditIntegrityReportV2 {
    AuditIntegrityReportV2 {
        status: "unavailable".into(),
        trust_level: "unavailable".into(),
        error_code: Some(code),
        chain_complete: false,
        ..empty_report()
    }
}

fn assess(root: &Path) -> Result<Assessment, String> {
    let path = audit_file_path(root);
    let marker = native_history_marker()?;
    let mut report = empty_report();
    let mut files = Vec::new();
    let mut line_number = 0;
    for candidate in retained_audit_paths(&path, MAX_AUDIT_ARCHIVES) {
        let bytes = fs::read(&candidate).map_err(|_| "audit_file_unreadable".to_string())?;
        files.push(PreservedFile {
            name: candidate
                .file_name()
                .ok_or(AUDIT_STORAGE_FAILED)?
                .to_string_lossy()
                .into(),
            size: bytes.len() as u64,
            sha256: sha256(&bytes),
        });
        if bytes.is_empty() {
            continue;
        }
        for line in bytes.split_inclusive(|byte| *byte == b'\n') {
            line_number += 1;
            let value = serde_json::from_slice::<Value>(line).ok();
            let object = value.as_ref().and_then(Value::as_object);
            if object.is_some_and(|object| object.contains_key("integrity")) {
                report.signed_events += 1;
            } else if line.last() == Some(&b'\n')
                && object.is_some_and(|object| {
                    ["timestamp", "area", "action"]
                        .iter()
                        .all(|key| object.get(*key).and_then(Value::as_str).is_some())
                })
            {
                report.legacy_events += 1;
            } else {
                report.malformed_events += 1;
                report.first_problem_line.get_or_insert(line_number);
            }
        }
    }
    report.files_checked = files.len();
    if report.signed_events > 0 {
        match native_signing_key_allowed(false) {
            Ok(key) => {
                let checked = verify_chain(&path, &key, MAX_AUDIT_ARCHIVES).report;
                report.status = if checked.status == AuditIntegrityStatus::Ok {
                    "ok"
                } else {
                    "tampered"
                }
                .into();
                report.trust_level = if checked.status == AuditIntegrityStatus::Ok {
                    "verified"
                } else {
                    "broken"
                }
                .into();
                report.first_sequence = checked.first_sequence;
                report.last_sequence = checked.last_sequence;
                report.chain_complete = checked.chain_complete;
                report.error_code = checked.error_code;
                // A formerly mixed legacy/signed chain is never presented as verified.
                if checked.status == AuditIntegrityStatus::Legacy {
                    report.error_code = Some("audit_mixed_legacy_history".into());
                }
            }
            Err(_) if native_key_missing()? => {
                report.status = "tampered".into();
                report.trust_level = "broken".into();
                report.chain_complete = false;
                report.error_code = Some("audit_signing_key_missing".into());
            }
            Err(_) => {
                report.status = "unavailable".into();
                report.trust_level = "unavailable".into();
                report.chain_complete = false;
                report.error_code = Some("audit_signing_key_unavailable".into());
            }
        }
    } else if marker {
        report.status = "tampered".into();
        report.trust_level = "broken".into();
        report.chain_complete = false;
        report.error_code = Some("audit_signed_history_missing".into());
    } else if report.malformed_events > 0 {
        report.status = "legacy_damaged".into();
        report.trust_level = "broken".into();
        report.chain_complete = false;
        report.error_code = Some("audit_malformed_legacy_record".into());
    } else if report.legacy_events > 0 {
        report.status = "legacy_clean".into();
        report.trust_level = "legacy_unverified".into();
        report.chain_complete = false;
    }
    // Keyring/lock failures cannot be repaired by rewriting the ledger.
    report.recovery_allowed = matches!(
        report.status.as_str(),
        "legacy_clean" | "legacy_damaged" | "tampered"
    );
    let identity = serde_json::to_vec(&(2, &files, marker, &report.status, &report.error_code))
        .map_err(io_error)?;
    report.assessment_id = Some(sha256(&identity));
    if root.join("audit").join(RECOVERY_JOURNAL).exists() {
        let journal: RecoveryJournal = read_json(&root.join("audit").join(RECOVERY_JOURNAL))?;
        let key = native_signing_key_allowed(false)?;
        if journal.mac != recovery_mac(&journal, &key)? {
            return Err("audit_recovery_journal_untrusted".into());
        }
        report.status = "recovering".into();
        report.trust_level = "unavailable".into();
        report.error_code = Some("audit_recovery_pending".into());
        report.recovery_allowed = true;
    } else if root.join("audit").join(PENDING).exists()
        || root.join("audit").join(ROTATION).exists()
    {
        report.status = "recovering".into();
        report.trust_level = "unavailable".into();
        report.error_code = Some("audit_append_pending".into());
        report.recovery_allowed = false;
    }
    if let Ok(metadata) =
        read_json::<RecoveryMetadata>(&root.join("audit").join("last-recovery.json"))
    {
        if uuid::Uuid::parse_str(&metadata.recovery_id).is_ok()
            && decode_mac(&metadata.manifest_sha256).is_some()
            && matches!(
                metadata.prior_trust.as_str(),
                "verified" | "legacy_unverified" | "broken" | "unavailable"
            )
        {
            report.recovery = Some(metadata);
        }
    }
    Ok(Assessment {
        report,
        files,
        marker,
    })
}

pub struct AuditLedger {
    root: PathBuf,
}

impl AuditLedger {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    pub fn inspect(&self) -> AuditIntegrityReportV2 {
        match acquire(&self.root, LOCK_TIMEOUT)
            .and_then(|_guard| assess(&self.root).map(|value| value.report))
        {
            Ok(report) => report,
            Err(error) => fail_report(error),
        }
    }

    pub fn legacy_report(&self) -> AuditIntegrityReport {
        as_legacy(&self.inspect())
    }

    pub fn verified_tail(
        &self,
        limit: usize,
    ) -> Result<(AuditIntegrityReport, Vec<String>), String> {
        let _guard = acquire(&self.root, LOCK_TIMEOUT)?;
        let assessment = assess(&self.root)?;
        let report = as_legacy(&assessment.report);
        // Unverified legacy bytes are never included in support bundles or log tails.
        if assessment.report.status != "ok" {
            return Ok((report, Vec::new()));
        }
        let key = native_signing_key_allowed(false)?;
        let state = verify_chain(&audit_file_path(&self.root), &key, MAX_AUDIT_ARCHIVES);
        let start = state.active_lines.len().saturating_sub(limit);
        Ok((report, state.active_lines.into_iter().skip(start).collect()))
    }

    pub fn append(&self, area: &str, action: &str, details: Option<Value>) -> Result<(), String> {
        let _guard = acquire(&self.root, LOCK_TIMEOUT)?;
        let path = audit_file_path(&self.root);
        if self.root.join("audit").join(RECOVERY_JOURNAL).exists() {
            self.resume_recovery()?;
        }
        if self.root.join("audit").join(ROTATION).exists() {
            resume_rotation(&path, &native_signing_key_allowed(false)?)?;
        }
        if self.root.join("audit").join(PENDING).exists() {
            replay_pending(
                &path,
                &native_signing_key_allowed(false)?,
                MAX_AUDIT_ARCHIVES,
            )?;
        }
        let assessment = assess(&self.root)?;
        if assessment.report.status == "legacy_clean" && !assessment.marker {
            self.begin_recovery(assessment)?;
        } else if !matches!(assessment.report.status.as_str(), "empty" | "ok") {
            return Err(assessment
                .report
                .error_code
                .unwrap_or_else(|| AUDIT_INTEGRITY_FAILED.into()));
        }
        let key = native_signing_key_allowed(!has_audit_history(&path, MAX_AUDIT_ARCHIVES))?;
        // Persist the rollback/deletion sentinel before the first signed disk write.
        // A crash here fails closed, rather than permitting silent loss of signed history.
        persist_native_history_marker()?;
        append_with_key(
            &path,
            &key,
            area,
            action,
            details,
            MAX_AUDIT_FILE_BYTES,
            MAX_AUDIT_ARCHIVES,
        )?;
        Ok(())
    }

    /// assessmentId binds the user's confirmation to the exact byte snapshot.
    pub fn recover(&self, assessment_id: &str) -> Result<AuditIntegrityReportV2, String> {
        let _guard = acquire(&self.root, LOCK_TIMEOUT)?;
        let assessment = assess(&self.root)?;
        if assessment.report.assessment_id.as_deref() != Some(assessment_id) {
            return Err("audit_assessment_stale".into());
        }
        if !assessment.report.recovery_allowed {
            return Err("audit_recovery_not_allowed".into());
        }
        if self.root.join("audit").join(RECOVERY_JOURNAL).exists() {
            self.resume_recovery()?;
        } else {
            self.begin_recovery(assessment)?;
        }
        Ok(assess(&self.root)?.report)
    }

    pub fn preserved_directory(&self, recovery_id: &str) -> Result<PathBuf, String> {
        uuid::Uuid::parse_str(recovery_id).map_err(|_| "audit_recovery_id_invalid".to_string())?;
        let dir = self.root.join("audit").join("preserved").join(recovery_id);
        if !dir.join("manifest.json").is_file() {
            return Err("audit_recovery_not_found".into());
        }
        Ok(dir)
    }

    fn begin_recovery(&self, assessment: Assessment) -> Result<(), String> {
        // Only this authorized entry (explicit confirmation or clean legacy upgrade)
        // may create a key. Resuming an on-disk journal must never create one.
        let key = native_signing_key_allowed(true)?;
        let mut journal = RecoveryJournal {
            schema_version: 1,
            recovery_id: uuid::Uuid::new_v4().to_string(),
            assessment_id: assessment
                .report
                .assessment_id
                .ok_or(AUDIT_INTEGRITY_FAILED)?,
            phase: "assessed".into(),
            prior_trust: assessment.report.trust_level,
            legacy_events: assessment.report.legacy_events,
            malformed_events: assessment.report.malformed_events,
            files: assessment.files,
            manifest_sha256: None,
            new_chain: None,
            mac: String::new(),
        };
        write_recovery(
            &self.root.join("audit").join(RECOVERY_JOURNAL),
            &mut journal,
            &key,
        )?;
        checkpoint("assessed")?;
        self.resume_recovery()
    }

    fn resume_recovery(&self) -> Result<(), String> {
        let dir = self.root.join("audit");
        let journal_path = dir.join(RECOVERY_JOURNAL);
        let mut journal: RecoveryJournal = read_json(&journal_path)?;
        let key = native_signing_key_allowed(false)?;
        if journal.mac != recovery_mac(&journal, &key)? {
            return Err("audit_recovery_journal_untrusted".into());
        }
        if journal.schema_version != 1
            || uuid::Uuid::parse_str(&journal.recovery_id).is_err()
            || decode_mac(&journal.assessment_id).is_none()
        {
            return Err("audit_journal_invalid".into());
        }
        validate_files(&journal.files)?;
        let archive = dir.join("preserved").join(&journal.recovery_id);
        create_dir_all(&archive).map_err(io_error)?;
        if journal.phase == "assessed" {
            for file in &journal.files {
                // Original remains untouched until every archive byte and its manifest are durable.
                let bytes = checked_bytes(&dir, file)?;
                durable_write(&archive.join(&file.name), &bytes)?;
            }
            journal.phase = "files_moved".into();
            write_recovery(&journal_path, &mut journal, &key)?;
            checkpoint("files_moved")?;
        }
        for file in &journal.files {
            checked_bytes(&archive, file)?;
        }
        if journal.phase == "files_moved" {
            let manifest = serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1, "recoveryId": journal.recovery_id, "assessmentId": journal.assessment_id,
                "priorTrust": journal.prior_trust, "legacyEvents": journal.legacy_events,
                "malformedEvents": journal.malformed_events, "files": journal.files,
            })).map_err(io_error)?;
            durable_write(&archive.join("manifest.json"), &manifest)?;
            journal.manifest_sha256 = Some(sha256(&manifest));
            journal.phase = "manifest_synced".into();
            write_recovery(&journal_path, &mut journal, &key)?;
            checkpoint("manifest_synced")?;
        }
        let manifest_hash = sha256(&fs::read(archive.join("manifest.json")).map_err(io_error)?);
        if journal.manifest_sha256.as_deref() != Some(&manifest_hash) {
            return Err("audit_preserved_manifest_changed".into());
        }
        if journal.phase == "manifest_synced" {
            let mut record = SignedAuditRecord {
                timestamp: Utc::now().to_rfc3339(),
                area: "audit".into(),
                action: "audit_legacy_history_preserved".into(),
                details: serde_json::json!({ "recoveryId": journal.recovery_id, "manifestSha256": manifest_hash,
                    "priorTrust": journal.prior_trust, "legacyEvents": journal.legacy_events,
                    "malformedEvents": journal.malformed_events, "filesPreserved": journal.files.len() }),
                integrity: AuditIntegrityEnvelope {
                    version: 1,
                    sequence: 1,
                    previous_mac: None,
                    mac: String::new(),
                },
            };
            record.integrity.mac = calculate_mac(&key, &record)?;
            journal.new_chain = Some(format!(
                "{}\n",
                serde_json::to_string(&record).map_err(io_error)?
            ));
            journal.phase = "key_ready".into();
            write_recovery(&journal_path, &mut journal, &key)?;
            checkpoint("key_ready")?;
        }
        let chain = journal.new_chain.clone().ok_or("audit_journal_invalid")?;
        let record: SignedAuditRecord = serde_json::from_str(chain.trim_end_matches('\n'))
            .map_err(|_| "audit_journal_invalid".to_string())?;
        if record.integrity.sequence != 1
            || record.integrity.previous_mac.is_some()
            || record.integrity.mac != calculate_mac(&key, &record)?
            || record.details["manifestSha256"] != manifest_hash
            || record.details["recoveryId"] != journal.recovery_id
        {
            return Err("audit_journal_invalid".into());
        }
        if journal.phase == "key_ready" {
            // Every original is checked again. Restart tolerates only already removed originals
            // or the exact new signed anchor; a concurrently edited file fails closed.
            for file in &journal.files {
                let original = dir.join(&file.name);
                if !original.exists() {
                    continue;
                }
                let bytes = fs::read(&original).map_err(io_error)?;
                if file.name == "events.jsonl" && bytes == chain.as_bytes() {
                    continue;
                }
                if sha256(&bytes) != file.sha256 || bytes.len() as u64 != file.size {
                    return Err("audit_recovery_source_changed".into());
                }
                remove_synced(&original)?;
                checkpoint("original_removed")?;
            }
            let active = dir.join("events.jsonl");
            if active.exists() && fs::read(&active).map_err(io_error)? != chain.as_bytes() {
                return Err("audit_recovery_source_changed".into());
            }
            durable_write(&active, chain.as_bytes())?;
            journal.phase = "new_chain_synced".into();
            write_recovery(&journal_path, &mut journal, &key)?;
            checkpoint("new_chain_synced")?;
        }
        if journal.phase != "new_chain_synced" && journal.phase != "complete" {
            return Err("audit_journal_invalid".into());
        }
        if fs::read(dir.join("events.jsonl")).map_err(io_error)? != chain.as_bytes() {
            return Err("audit_recovery_source_changed".into());
        }
        persist_native_history_marker()?;
        let metadata = RecoveryMetadata {
            recovery_id: journal.recovery_id.clone(),
            manifest_sha256: manifest_hash,
            files_preserved: journal.files.len(),
            bytes_preserved: journal.files.iter().map(|file| file.size).sum(),
            prior_trust: journal.prior_trust.clone(),
        };
        write_json(&dir.join("last-recovery.json"), &metadata)?;
        journal.phase = "complete".into();
        write_recovery(&journal_path, &mut journal, &key)?;
        checkpoint("complete")?;
        replace_file(&journal_path, &archive.join("completed.json"))
    }
}

fn as_legacy(report: &AuditIntegrityReportV2) -> AuditIntegrityReport {
    AuditIntegrityReport {
        status: match report.status.as_str() {
            "empty" => AuditIntegrityStatus::Empty,
            "ok" => AuditIntegrityStatus::Ok,
            "legacy_clean" => AuditIntegrityStatus::Legacy,
            "legacy_damaged" | "tampered" => AuditIntegrityStatus::Tampered,
            _ => AuditIntegrityStatus::Unavailable,
        },
        signed_events: report.signed_events,
        legacy_events: report.legacy_events,
        files_checked: report.files_checked,
        first_sequence: report.first_sequence,
        last_sequence: report.last_sequence,
        chain_complete: report.chain_complete,
        error_code: report.error_code.clone(),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingAppend {
    prefix_len: u64,
    prefix_sha256: String,
    record: String,
}

pub(super) fn write_pending(path: &Path, record: &str) -> Result<(), String> {
    let prefix = if path.exists() {
        fs::read(path).map_err(io_error)?
    } else {
        Vec::new()
    };
    write_json(
        &path.with_file_name(PENDING),
        &PendingAppend {
            prefix_len: prefix.len() as u64,
            prefix_sha256: sha256(&prefix),
            record: format!("{record}\n"),
        },
    )?;
    checkpoint("pending_synced")
}

pub(super) fn replay_pending(
    path: &Path,
    key: &[u8; 32],
    max_archives: usize,
) -> Result<(), String> {
    let pending_path = path.with_file_name(PENDING);
    if !pending_path.exists() {
        return Ok(());
    }
    let pending: PendingAppend = read_json(&pending_path)?;
    if pending.record.len() > MAX_AUDIT_EVENT_BYTES + 1 || !pending.record.ends_with('\n') {
        return Err("audit_pending_invalid".into());
    }
    let record: SignedAuditRecord = serde_json::from_str(pending.record.trim_end_matches('\n'))
        .map_err(|_| "audit_pending_invalid".to_string())?;
    if record.integrity.mac != calculate_mac(key, &record)?
        || record.integrity.version != 1
        || format!("{}\n", serde_json::to_string(&record).map_err(io_error)?) != pending.record
    {
        return Err("audit_pending_invalid".into());
    }
    let current = if path.exists() {
        fs::read(path).map_err(io_error)?
    } else {
        Vec::new()
    };
    let prefix_len = usize::try_from(pending.prefix_len).map_err(|_| "audit_pending_invalid")?;
    if current.len() < prefix_len
        || sha256(&current[..prefix_len]) != pending.prefix_sha256
        || !pending
            .record
            .as_bytes()
            .starts_with(&current[prefix_len..])
    {
        return Err("audit_pending_conflict".into());
    }
    let state = verify_chain_snapshot(path, key, max_archives, Some(&current[..prefix_len]));
    if !state.report.permits_read()
        || record.integrity.sequence != state.report.last_sequence.unwrap_or(0) + 1
        || record.integrity.previous_mac != state.last_mac
    {
        return Err("audit_pending_chain_conflict".into());
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(io_error)?;
    file.set_len(pending.prefix_len).map_err(io_error)?;
    file.seek(SeekFrom::Start(pending.prefix_len))
        .map_err(io_error)?;
    file.write_all(pending.record.as_bytes())
        .map_err(io_error)?;
    checkpoint("append_written")?;
    file.sync_all().map_err(io_error)?;
    checkpoint("append_synced")?;
    drop(file);
    remove_synced(&pending_path)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RotationJournal {
    id: String,
    max_archives: usize,
    files: Vec<PreservedFile>,
    mac: String,
}

fn rotation_mac(journal: &RotationJournal, key: &[u8; 32]) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(
        "audit-rotation-v1",
        &journal.id,
        journal.max_archives,
        &journal.files,
    ))
    .map_err(io_error)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(io_error)?;
    mac.update(&bytes);
    Ok(encode_hex(&mac.finalize().into_bytes()))
}

pub(super) fn rotate_journaled(
    path: &Path,
    key: &[u8; 32],
    incoming: u64,
    max_bytes: u64,
    max_archives: usize,
) -> Result<(), String> {
    if !path.exists()
        || fs::metadata(path)
            .map_err(io_error)?
            .len()
            .saturating_add(incoming)
            <= max_bytes
    {
        return Ok(());
    }
    if max_archives > MAX_AUDIT_ARCHIVES {
        return Err("audit_rotation_invalid".into());
    }
    let dir = path.parent().ok_or(AUDIT_STORAGE_FAILED)?;
    let id = uuid::Uuid::new_v4().to_string();
    let staging = dir.join(format!(".rotation-{id}"));
    create_dir_all(&staging).map_err(io_error)?;
    let mut files = Vec::new();
    for source in retained_audit_paths(path, max_archives) {
        let bytes = fs::read(&source).map_err(io_error)?;
        let name = source
            .file_name()
            .ok_or(AUDIT_STORAGE_FAILED)?
            .to_string_lossy()
            .to_string();
        durable_write(&staging.join(&name), &bytes)?;
        files.push(PreservedFile {
            name,
            size: bytes.len() as u64,
            sha256: sha256(&bytes),
        });
    }
    let mut journal = RotationJournal {
        id,
        max_archives,
        files,
        mac: String::new(),
    };
    journal.mac = rotation_mac(&journal, key)?;
    write_json(&dir.join(ROTATION), &journal)?;
    checkpoint("rotation_journal_synced")?;
    resume_rotation(path, key)
}

pub(super) fn resume_rotation(path: &Path, key: &[u8; 32]) -> Result<(), String> {
    let dir = path.parent().ok_or(AUDIT_STORAGE_FAILED)?;
    let journal_path = dir.join(ROTATION);
    if !journal_path.exists() {
        return Ok(());
    }
    let journal: RotationJournal = read_json(&journal_path)?;
    if uuid::Uuid::parse_str(&journal.id).is_err()
        || journal.max_archives > MAX_AUDIT_ARCHIVES
        || journal.mac != rotation_mac(&journal, key)?
    {
        return Err("audit_rotation_invalid".into());
    }
    validate_files(&journal.files)?;
    let staging = dir.join(format!(".rotation-{}", journal.id));
    for file in &journal.files {
        checked_bytes(&staging, file)?;
    }
    // A crash permits an original state or an already assigned rotation state,
    // never unrelated changes. Validate ALL destinations before changing any.
    for index in 0..=journal.max_archives {
        let name = if index == 0 {
            "events.jsonl".into()
        } else {
            format!("events.{index}.jsonl")
        };
        let original = journal.files.iter().find(|file| file.name == name);
        let previous = if index == 1 {
            "events.jsonl".into()
        } else {
            format!("events.{}.jsonl", index.saturating_sub(1))
        };
        let assigned = if index == 0 {
            None
        } else {
            journal.files.iter().find(|file| file.name == previous)
        };
        let target = dir.join(name);
        if target.exists() {
            let bytes = fs::read(&target).map_err(io_error)?;
            let digest = sha256(&bytes);
            if ![original, assigned]
                .into_iter()
                .flatten()
                .any(|file| file.size == bytes.len() as u64 && file.sha256 == digest)
            {
                return Err("audit_rotation_conflict".into());
            }
        } else if original.is_some() && assigned.is_some() {
            return Err("audit_rotation_conflict".into());
        }
    }
    // Immutable staged originals make each assignment repeatable after an interruption.
    for index in (1..=journal.max_archives).rev() {
        let source_name = if index == 1 {
            "events.jsonl".into()
        } else {
            format!("events.{}.jsonl", index - 1)
        };
        let target = archive_path(path, index);
        if let Some(source) = journal.files.iter().find(|file| file.name == source_name) {
            durable_write(&target, &checked_bytes(&staging, source)?)?;
            checkpoint("rotation_target_written")?;
        } else {
            remove_synced(&target)?;
        }
    }
    remove_synced(path)?;
    checkpoint("rotation_active_removed")?;
    remove_synced(&journal_path)?;
    // Staging cleanup happens only after journal completion, never before durable targets.
    for file in &journal.files {
        remove_synced(&staging.join(&file.name))?;
    }
    fs::remove_dir(&staging).map_err(io_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(damaged: bool) -> (tempfile::TempDir, Vec<u8>) {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("audit")).unwrap();
        let mut bytes = Vec::new();
        for index in 1..=949 {
            if damaged && index == 896 {
                bytes.extend_from_slice(b"backend_panic{\"action\":\"interleaved\"}\n");
            } else {
                bytes.extend_from_slice(format!("{{\"timestamp\":\"2026-07-02T12:00:00Z\",\"area\":\"runtime\",\"action\":\"legacy-{index}\"}}\n").as_bytes());
            }
        }
        fs::write(audit_file_path(root.path()), &bytes).unwrap();
        (root, bytes)
    }

    #[test]
    fn damaged_legacy_is_classified_without_key_or_mutation() {
        let (root, original) = fixture(true);
        let ledger = AuditLedger::new(root.path());
        let report = ledger.inspect();
        assert_eq!(report.status, "legacy_damaged");
        assert_eq!(report.legacy_events, 948);
        assert_eq!(report.malformed_events, 1);
        assert_eq!(report.first_problem_line, Some(896));
        assert!(report.recovery_allowed);
        assert!(ledger.append("test", "blocked", None).is_err());
        assert_eq!(fs::read(audit_file_path(root.path())).unwrap(), original);
        assert!(!root.path().join("audit/preserved").exists());
        assert!(ledger.verified_tail(1000).unwrap().1.is_empty());
    }

    #[test]
    fn confirmed_recovery_preserves_exact_bytes_and_binds_manifest_in_first_record() {
        let (root, original) = fixture(true);
        let ledger = AuditLedger::new(root.path());
        let before = ledger.inspect();
        let report = ledger
            .recover(before.assessment_id.as_deref().unwrap())
            .unwrap();
        assert_eq!(report.status, "ok");
        assert_eq!(report.first_sequence, Some(1));
        let metadata = report.recovery.unwrap();
        let archive = ledger.preserved_directory(&metadata.recovery_id).unwrap();
        assert_eq!(fs::read(archive.join("events.jsonl")).unwrap(), original);
        let manifest = fs::read(archive.join("manifest.json")).unwrap();
        assert_eq!(sha256(&manifest), metadata.manifest_sha256);
        let active: Value =
            serde_json::from_slice(&fs::read(audit_file_path(root.path())).unwrap()).unwrap();
        assert_eq!(
            active["details"]["manifestSha256"],
            metadata.manifest_sha256
        );
        assert_eq!(active["details"]["malformedEvents"], 1);
        assert_eq!(active["details"]["legacyEvents"], 948);
        ledger.append("test", "after_recovery", None).unwrap();
        assert_eq!(ledger.inspect().last_sequence, Some(2));
    }

    #[test]
    fn clean_legacy_is_preserved_automatically_but_never_exported_as_verified() {
        let (root, original) = fixture(false);
        let ledger = AuditLedger::new(root.path());
        assert_eq!(ledger.inspect().status, "legacy_clean");
        assert!(ledger.verified_tail(1000).unwrap().1.is_empty());
        ledger.append("runtime", "upgraded", None).unwrap();
        let report = ledger.inspect();
        assert_eq!(report.status, "ok");
        assert_eq!(report.last_sequence, Some(2));
        let archive = ledger
            .preserved_directory(&report.recovery.unwrap().recovery_id)
            .unwrap();
        assert_eq!(fs::read(archive.join("events.jsonl")).unwrap(), original);
    }

    #[test]
    fn stale_confirmation_and_invalid_archive_ids_fail_closed() {
        let (root, _) = fixture(true);
        let ledger = AuditLedger::new(root.path());
        let report = ledger.inspect();
        OpenOptions::new()
            .append(true)
            .open(audit_file_path(root.path()))
            .unwrap()
            .write_all(b"changed\n")
            .unwrap();
        assert_eq!(
            ledger
                .recover(report.assessment_id.as_deref().unwrap())
                .unwrap_err(),
            "audit_assessment_stale"
        );
        assert!(ledger.preserved_directory("../../private").is_err());
        assert!(!root.path().join("audit/preserved").exists());
    }

    #[test]
    fn every_durable_recovery_phase_is_restartable() {
        for phase in [
            "assessed",
            "files_moved",
            "manifest_synced",
            "key_ready",
            "original_removed",
            "new_chain_synced",
            "complete",
        ] {
            let (root, original) = fixture(true);
            let ledger = AuditLedger::new(root.path());
            let report = ledger.inspect();
            FAULT.with(|fault| fault.set(Some(phase)));
            assert_eq!(
                ledger
                    .recover(report.assessment_id.as_deref().unwrap())
                    .unwrap_err(),
                "audit_injected_interruption",
                "{phase}"
            );
            let restarted = AuditLedger::new(root.path());
            let pending = restarted.inspect();
            assert_eq!(pending.status, "recovering", "{phase}");
            let done = restarted
                .recover(pending.assessment_id.as_deref().unwrap())
                .unwrap();
            assert_eq!(done.status, "ok", "{phase}");
            let archive = restarted
                .preserved_directory(&done.recovery.unwrap().recovery_id)
                .unwrap();
            assert_eq!(
                fs::read(archive.join("events.jsonl")).unwrap(),
                original,
                "{phase}"
            );
            assert!(!root.path().join("audit").join(RECOVERY_JOURNAL).exists());
        }
    }

    #[test]
    fn pending_append_recovers_once_at_each_write_boundary() {
        for phase in ["pending_synced", "append_written", "append_synced"] {
            let root = tempfile::tempdir().unwrap();
            let ledger = AuditLedger::new(root.path());
            ledger.append("test", "first", None).unwrap();
            FAULT.with(|fault| fault.set(Some(phase)));
            assert!(ledger.append("test", "second", None).is_err());
            ledger.append("test", "third", None).unwrap();
            let report = ledger.inspect();
            assert_eq!(report.status, "ok", "{phase}");
            assert_eq!(report.signed_events, 3, "{phase}");
        }
    }

    #[test]
    fn authenticated_wal_repairs_only_its_exact_partial_tail() {
        let root = tempfile::tempdir().unwrap();
        let ledger = AuditLedger::new(root.path());
        ledger.append("test", "first", None).unwrap();
        FAULT.with(|fault| fault.set(Some("pending_synced")));
        assert!(ledger.append("test", "second", None).is_err());
        let path = audit_file_path(root.path());
        let pending: PendingAppend = read_json(&path.with_file_name(PENDING)).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&pending.record.as_bytes()[..43])
            .unwrap();
        ledger.append("test", "third", None).unwrap();
        assert_eq!(ledger.inspect().signed_events, 3);
        FAULT.with(|fault| fault.set(Some("pending_synced")));
        assert!(ledger.append("test", "fourth", None).is_err());
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"unrelated-tamper")
            .unwrap();
        let before = fs::read(&path).unwrap();
        assert!(ledger.append("test", "must-not-write", None).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn rotation_is_restartable_at_each_durable_boundary() {
        for phase in [
            "rotation_journal_synced",
            "rotation_target_written",
            "rotation_active_removed",
        ] {
            let root = tempfile::tempdir().unwrap();
            fs::create_dir(root.path().join("audit")).unwrap();
            let path = audit_file_path(root.path());
            append_with_key(&path, &[0x42; 32], "test", "first", None, 1, 3).unwrap();
            FAULT.with(|fault| fault.set(Some(phase)));
            assert!(append_with_key(&path, &[0x42; 32], "test", "second", None, 1, 3).is_err());
            append_with_key(&path, &[0x42; 32], "test", "retry", None, 1, 3).unwrap();
            let report = verify_chain(&path, &[0x42; 32], 3).report;
            assert_eq!(report.status, AuditIntegrityStatus::Ok, "{phase}");
            assert_eq!(report.last_sequence, Some(2), "{phase}");
        }
    }

    #[test]
    fn lock_timeout_does_not_write_event_or_recovery_files() {
        let (root, original) = fixture(true);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.path().join("audit/ledger.lock"))
            .unwrap();
        file.lock_exclusive().unwrap();
        assert!(
            matches!(acquire(root.path(), Duration::from_millis(30)), Err(error) if error == "audit_lock_busy")
        );
        assert_eq!(fs::read(audit_file_path(root.path())).unwrap(), original);
        assert!(!root.path().join("audit").join(RECOVERY_JOURNAL).exists());
        FileExt::unlock(&file).unwrap();
    }

    #[test]
    fn report_has_no_event_contents_paths_or_authentication_material() {
        let (root, _) = fixture(true);
        let text = serde_json::to_string(&AuditLedger::new(root.path()).inspect()).unwrap();
        for forbidden in [
            "backend_panic",
            "interleaved",
            "previousMac",
            "signingKey",
            "C:\\",
            "events.jsonl",
        ] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn forged_recovery_journal_cannot_authorize_a_new_chain() {
        let (root, original) = fixture(true);
        let assessment = assess(root.path()).unwrap();
        let fake = RecoveryJournal {
            schema_version: 1,
            recovery_id: uuid::Uuid::new_v4().to_string(),
            assessment_id: assessment.report.assessment_id.unwrap(),
            phase: "assessed".into(),
            prior_trust: "broken".into(),
            legacy_events: 948,
            malformed_events: 1,
            files: assessment.files,
            manifest_sha256: None,
            new_chain: None,
            mac: "0".repeat(64),
        };
        write_json(&root.path().join("audit").join(RECOVERY_JOURNAL), &fake).unwrap();
        let ledger = AuditLedger::new(root.path());
        assert!(!ledger.inspect().recovery_allowed);
        assert_eq!(
            ledger.append("test", "attack", None).unwrap_err(),
            "audit_recovery_journal_untrusted"
        );
        assert_eq!(fs::read(audit_file_path(root.path())).unwrap(), original);
        assert!(!root.path().join("audit/preserved").exists());
    }

    #[test]
    fn rotation_resume_never_overwrites_unrelated_modified_files() {
        for name in ["events.jsonl", "events.1.jsonl"] {
            let root = tempfile::tempdir().unwrap();
            fs::create_dir(root.path().join("audit")).unwrap();
            let path = audit_file_path(root.path());
            for _ in 0..2 {
                append_with_key(&path, &[0x42; 32], "test", "first", None, 1, 3).unwrap();
            }
            FAULT.with(|fault| fault.set(Some("rotation_journal_synced")));
            assert!(
                append_with_key(&path, &[0x42; 32], "test", "interrupted", None, 1, 3).is_err()
            );
            let target = path.with_file_name(name);
            fs::write(&target, "tampered-after-interruption\n").unwrap();
            let before = retained_audit_paths(&path, 3)
                .iter()
                .map(|file| (file.clone(), fs::read(file).unwrap()))
                .collect::<Vec<_>>();
            assert_eq!(
                resume_rotation(&path, &[0x42; 32]).unwrap_err(),
                "audit_rotation_conflict"
            );
            for (file, bytes) in before {
                assert_eq!(fs::read(file).unwrap(), bytes);
            }
        }
    }

    #[test]
    #[ignore = "worker for the isolated multi-process test"]
    fn ledger_process_writer_worker() {
        let Some(root) = std::env::var_os("LACOWORK_AUDIT_TEST_ROOT") else {
            return;
        };
        let ledger = AuditLedger::new(Path::new(&root));
        for index in 0..200 {
            let started = Instant::now();
            loop {
                match ledger.append("test", "parallel", Some(serde_json::json!({"index":index}))) {
                    Ok(()) => break,
                    Err(error)
                        if error == "audit_lock_busy"
                            && started.elapsed() < Duration::from_secs(60) =>
                    {
                        continue
                    }
                    Err(error) => panic!("parallel append failed: {error}"),
                }
            }
            // Cooperative clients back off between writes; lock contention itself
            // is a documented retryable outcome, not an integrity violation.
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    #[test]
    #[ignore = "slow 8-process durability regression; run explicitly"]
    fn eight_processes_append_1600_verified_records() {
        let root = tempfile::tempdir().unwrap();
        let executable = std::env::current_exe().unwrap();
        let mut children = Vec::new();
        for _ in 0..8 {
            let mut command = std::process::Command::new(&executable);
            command
                .args([
                    "--ignored",
                    "--exact",
                    "audit_service::ledger::tests::ledger_process_writer_worker",
                    "--nocapture",
                ])
                .env("LACOWORK_AUDIT_TEST_ROOT", root.path())
                .stdout(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            children.push(command.spawn().unwrap());
        }
        let statuses = children
            .into_iter()
            .map(|mut child| child.wait().unwrap())
            .collect::<Vec<_>>();
        assert!(
            statuses.iter().all(|status| status.success()),
            "worker outcomes: {statuses:?}"
        );
        let report = AuditLedger::new(root.path()).inspect();
        assert_eq!(report.status, "ok");
        assert_eq!(report.signed_events, 1600);
        assert_eq!(report.first_sequence, Some(1));
        assert_eq!(report.last_sequence, Some(1600));
    }
}
