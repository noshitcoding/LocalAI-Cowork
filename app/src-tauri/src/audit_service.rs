#![allow(dead_code)]

use crate::audit_sink::{AuditEvent, AuditRiskClass, AuditSink, AuditTarget};
use crate::context::RequestContext;
use crate::sensitive_data::{diagnostic_label, redact_and_bound_json_text, MAX_AUDIT_EVENT_BYTES};
use crate::service_error::ServiceResult;
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use std::fs::{self, create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_AUDIT_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_AUDIT_ARCHIVES: usize = 3;
const AUDIT_INTEGRITY_VERSION: u32 = 1;
const AUDIT_DETAILS_BYTES: usize = MAX_AUDIT_EVENT_BYTES - 4 * 1024;
const AUDIT_INTEGRITY_FAILED: &str = "audit integrity verification failed";
const AUDIT_SIGNING_KEY_UNAVAILABLE: &str = "audit signing key is unavailable";
const AUDIT_STORAGE_FAILED: &str = "audit storage operation failed";

#[cfg(all(not(test), target_os = "windows"))]
const AUDIT_KEY_SERVICE: &str = "com.open-cowork.desktop.audit.v1";
#[cfg(all(not(test), target_os = "windows"))]
const AUDIT_KEY_ACCOUNT: &str = "audit-signing-key-v1";
#[cfg(all(not(test), target_os = "windows"))]
const AUDIT_HISTORY_ACCOUNT: &str = "audit-signed-history-v1";

static AUDIT_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
#[cfg(all(not(test), target_os = "windows"))]
static AUDIT_SIGNING_KEY: OnceLock<[u8; 32]> = OnceLock::new();

fn archive_path(path: &Path, index: usize) -> PathBuf {
    path.with_file_name(format!("events.{index}.jsonl"))
}

fn rotate_if_needed(
    path: &Path,
    incoming_bytes: u64,
    max_file_bytes: u64,
    max_archives: usize,
) -> Result<(), String> {
    let current_bytes = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if current_bytes.saturating_add(incoming_bytes) <= max_file_bytes {
        return Ok(());
    }
    if max_archives == 0 {
        fs::remove_file(path).map_err(|error| error.to_string())?;
        return Ok(());
    }

    let oldest = archive_path(path, max_archives);
    if oldest.exists() {
        fs::remove_file(&oldest).map_err(|error| error.to_string())?;
    }
    for index in (1..max_archives).rev() {
        let source = archive_path(path, index);
        if source.exists() {
            fs::rename(&source, archive_path(path, index + 1))
                .map_err(|error| error.to_string())?;
        }
    }
    fs::rename(path, archive_path(path, 1)).map_err(|error| error.to_string())
}

pub struct AuditService<S>
where
    S: AuditSink,
{
    sink: S,
}

impl<S> AuditService<S>
where
    S: AuditSink,
{
    pub fn new(sink: S) -> Self {
        Self { sink }
    }

    pub fn record(&self, event: AuditEvent) -> ServiceResult<AuditEvent> {
        self.sink.emit(event.clone())?;
        Ok(event)
    }

    pub fn record_success(
        &self,
        context: &RequestContext,
        action: impl Into<String>,
        target: AuditTarget,
        risk_class: AuditRiskClass,
    ) -> ServiceResult<AuditEvent> {
        self.record(AuditEvent::success(context, action, target, risk_class))
    }

    pub fn record_denial(
        &self,
        context: &RequestContext,
        action: impl Into<String>,
        target: AuditTarget,
        risk_class: AuditRiskClass,
    ) -> ServiceResult<AuditEvent> {
        self.record(AuditEvent::denied(context, action, target, risk_class))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditIntegrityStatus {
    Empty,
    Ok,
    Legacy,
    Tampered,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditIntegrityReport {
    pub status: AuditIntegrityStatus,
    pub signed_events: u64,
    pub legacy_events: u64,
    pub files_checked: usize,
    pub first_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
    pub chain_complete: bool,
    pub error_code: Option<String>,
}

impl AuditIntegrityReport {
    fn empty() -> Self {
        Self {
            status: AuditIntegrityStatus::Empty,
            signed_events: 0,
            legacy_events: 0,
            files_checked: 0,
            first_sequence: None,
            last_sequence: None,
            chain_complete: true,
            error_code: None,
        }
    }

    fn failure(
        status: AuditIntegrityStatus,
        error_code: &'static str,
        files_checked: usize,
    ) -> Self {
        Self {
            status,
            files_checked,
            chain_complete: false,
            error_code: Some(error_code.to_string()),
            ..Self::empty()
        }
    }

    pub fn permits_read(&self) -> bool {
        matches!(
            self.status,
            AuditIntegrityStatus::Empty | AuditIntegrityStatus::Ok | AuditIntegrityStatus::Legacy
        )
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuditIntegrityEnvelope {
    version: u32,
    sequence: u64,
    previous_mac: Option<String>,
    mac: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SignedAuditRecord {
    timestamp: String,
    area: String,
    action: String,
    details: Value,
    integrity: AuditIntegrityEnvelope,
}

#[derive(Serialize)]
struct UnsignedAuditRecord<'a> {
    timestamp: &'a str,
    area: &'a str,
    action: &'a str,
    details: &'a Value,
    integrity: UnsignedAuditIntegrity<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsignedAuditIntegrity<'a> {
    version: u32,
    sequence: u64,
    previous_mac: Option<&'a str>,
}

struct AuditVerificationState {
    report: AuditIntegrityReport,
    last_mac: Option<String>,
    active_lines: Vec<String>,
}

fn audit_file_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("audit").join("events.jsonl")
}

fn retained_audit_paths(path: &Path, max_archives: usize) -> Vec<PathBuf> {
    let mut paths = (1..=max_archives)
        .rev()
        .map(|index| archive_path(path, index))
        .filter(|candidate| candidate.exists())
        .collect::<Vec<_>>();
    if path.exists() {
        paths.push(path.to_path_buf());
    }
    paths
}

fn has_audit_history(path: &Path, max_archives: usize) -> bool {
    retained_audit_paths(path, max_archives)
        .iter()
        .any(|candidate| {
            fs::metadata(candidate)
                .map(|metadata| metadata.len() > 0)
                .unwrap_or(true)
        })
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_mac(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut output = [0u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(output)
}

fn create_key_if_allowed<F>(has_history: bool, create: F) -> Result<[u8; 32], String>
where
    F: FnOnce() -> Result<[u8; 32], String>,
{
    if has_history {
        Err(AUDIT_SIGNING_KEY_UNAVAILABLE.to_string())
    } else {
        create()
    }
}

#[cfg(test)]
fn native_signing_key(_path: &Path) -> Result<[u8; 32], String> {
    Ok([0x42; 32])
}

#[cfg(all(not(test), target_os = "windows"))]
fn native_signing_key(path: &Path) -> Result<[u8; 32], String> {
    if let Some(key) = AUDIT_SIGNING_KEY.get() {
        return Ok(*key);
    }

    let entry = keyring::Entry::new(AUDIT_KEY_SERVICE, AUDIT_KEY_ACCOUNT)
        .map_err(|_| AUDIT_SIGNING_KEY_UNAVAILABLE.to_string())?;
    let key = match entry.get_password() {
        Ok(encoded) => {
            decode_mac(&encoded).ok_or_else(|| AUDIT_SIGNING_KEY_UNAVAILABLE.to_string())?
        }
        Err(keyring::Error::NoEntry) => {
            create_key_if_allowed(has_audit_history(path, MAX_AUDIT_ARCHIVES), || {
                let mut key = [0u8; 32];
                getrandom::fill(&mut key).map_err(|_| AUDIT_SIGNING_KEY_UNAVAILABLE.to_string())?;
                entry
                    .set_password(&encode_hex(&key))
                    .map_err(|_| AUDIT_SIGNING_KEY_UNAVAILABLE.to_string())?;
                Ok(key)
            })?
        }
        Err(_) => return Err(AUDIT_SIGNING_KEY_UNAVAILABLE.to_string()),
    };
    let _ = AUDIT_SIGNING_KEY.set(key);
    Ok(key)
}

#[cfg(all(not(test), not(target_os = "windows")))]
fn native_signing_key(_path: &Path) -> Result<[u8; 32], String> {
    Err(AUDIT_SIGNING_KEY_UNAVAILABLE.to_string())
}

#[cfg(test)]
fn native_history_marker() -> Result<bool, String> {
    Ok(false)
}

#[cfg(test)]
fn persist_native_history_marker() -> Result<(), String> {
    Ok(())
}

#[cfg(all(not(test), target_os = "windows"))]
fn native_history_marker() -> Result<bool, String> {
    let entry = keyring::Entry::new(AUDIT_KEY_SERVICE, AUDIT_HISTORY_ACCOUNT)
        .map_err(|_| AUDIT_SIGNING_KEY_UNAVAILABLE.to_string())?;
    match entry.get_password() {
        Ok(value) if value == "signed-history-present-v1" => Ok(true),
        Ok(_) => Err(AUDIT_SIGNING_KEY_UNAVAILABLE.to_string()),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(_) => Err(AUDIT_SIGNING_KEY_UNAVAILABLE.to_string()),
    }
}

#[cfg(all(not(test), target_os = "windows"))]
fn persist_native_history_marker() -> Result<(), String> {
    keyring::Entry::new(AUDIT_KEY_SERVICE, AUDIT_HISTORY_ACCOUNT)
        .map_err(|_| AUDIT_SIGNING_KEY_UNAVAILABLE.to_string())?
        .set_password("signed-history-present-v1")
        .map_err(|_| AUDIT_SIGNING_KEY_UNAVAILABLE.to_string())
}

#[cfg(all(not(test), not(target_os = "windows")))]
fn native_history_marker() -> Result<bool, String> {
    Err(AUDIT_SIGNING_KEY_UNAVAILABLE.to_string())
}

#[cfg(all(not(test), not(target_os = "windows")))]
fn persist_native_history_marker() -> Result<(), String> {
    Err(AUDIT_SIGNING_KEY_UNAVAILABLE.to_string())
}

fn canonical_record(record: &SignedAuditRecord) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&UnsignedAuditRecord {
        timestamp: &record.timestamp,
        area: &record.area,
        action: &record.action,
        details: &record.details,
        integrity: UnsignedAuditIntegrity {
            version: record.integrity.version,
            sequence: record.integrity.sequence,
            previous_mac: record.integrity.previous_mac.as_deref(),
        },
    })
    .map_err(|_| AUDIT_INTEGRITY_FAILED.to_string())
}

fn calculate_mac(key: &[u8; 32], record: &SignedAuditRecord) -> Result<String, String> {
    let canonical = canonical_record(record)?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| AUDIT_INTEGRITY_FAILED.to_string())?;
    mac.update(&canonical);
    Ok(encode_hex(&mac.finalize().into_bytes()))
}

fn tampered(error_code: &'static str, files_checked: usize) -> AuditVerificationState {
    AuditVerificationState {
        report: AuditIntegrityReport::failure(
            AuditIntegrityStatus::Tampered,
            error_code,
            files_checked,
        ),
        last_mac: None,
        active_lines: Vec::new(),
    }
}

fn unavailable(error_code: &'static str, files_checked: usize) -> AuditVerificationState {
    AuditVerificationState {
        report: AuditIntegrityReport::failure(
            AuditIntegrityStatus::Unavailable,
            error_code,
            files_checked,
        ),
        last_mac: None,
        active_lines: Vec::new(),
    }
}

fn reconcile_history_marker<F>(
    report: AuditIntegrityReport,
    marker_present: bool,
    persist_marker: F,
) -> AuditIntegrityReport
where
    F: FnOnce() -> Result<(), String>,
{
    if !report.permits_read() {
        return report;
    }
    if marker_present && report.signed_events == 0 {
        return AuditIntegrityReport::failure(
            AuditIntegrityStatus::Tampered,
            "audit_signed_history_missing",
            report.files_checked,
        );
    }
    if !marker_present && report.signed_events > 0 && persist_marker().is_err() {
        return AuditIntegrityReport::failure(
            AuditIntegrityStatus::Unavailable,
            "audit_history_marker_unavailable",
            report.files_checked,
        );
    }
    report
}

fn verify_chain(path: &Path, key: &[u8; 32], max_archives: usize) -> AuditVerificationState {
    let paths = retained_audit_paths(path, max_archives);
    let mut report = AuditIntegrityReport::empty();
    let mut last_mac: Option<String> = None;
    let mut signed_started = false;
    let mut active_lines = Vec::new();

    for candidate in paths {
        report.files_checked += 1;
        let bytes = match fs::read(&candidate) {
            Ok(bytes) => bytes,
            Err(_) => return unavailable("audit_file_unreadable", report.files_checked),
        };
        if bytes.is_empty() {
            continue;
        }
        if bytes.last() != Some(&b'\n') {
            return tampered("audit_truncated_record", report.files_checked);
        }
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(_) => return tampered("audit_invalid_encoding", report.files_checked),
        };
        if candidate == path {
            active_lines = text.lines().map(str::to_string).collect();
        }

        for line in text.lines() {
            if line.is_empty() {
                return tampered("audit_empty_record", report.files_checked);
            }
            let value = match serde_json::from_str::<Value>(line) {
                Ok(value) => value,
                Err(_) => return tampered("audit_malformed_record", report.files_checked),
            };
            let Some(object) = value.as_object() else {
                return tampered("audit_malformed_record", report.files_checked);
            };

            if !object.contains_key("integrity") {
                let legacy_shape_valid = object.get("timestamp").and_then(Value::as_str).is_some()
                    && object.get("area").and_then(Value::as_str).is_some()
                    && object.get("action").and_then(Value::as_str).is_some();
                if !legacy_shape_valid {
                    return tampered("audit_malformed_legacy_record", report.files_checked);
                }
                if signed_started {
                    return tampered("audit_legacy_after_signed", report.files_checked);
                }
                report.legacy_events += 1;
                continue;
            }

            let record = match serde_json::from_str::<SignedAuditRecord>(line) {
                Ok(record) => record,
                Err(_) => {
                    return tampered("audit_invalid_integrity_envelope", report.files_checked)
                }
            };
            if serde_json::to_string(&record).ok().as_deref() != Some(line) {
                return tampered("audit_noncanonical_record", report.files_checked);
            }
            if record.integrity.version != AUDIT_INTEGRITY_VERSION {
                return tampered("audit_unsupported_integrity_version", report.files_checked);
            }
            if record.integrity.sequence == 0
                || (record.integrity.sequence == 1 && record.integrity.previous_mac.is_some())
                || (record.integrity.sequence > 1 && record.integrity.previous_mac.is_none())
            {
                return tampered("audit_invalid_sequence_anchor", report.files_checked);
            }
            let Some(actual_mac) = decode_mac(&record.integrity.mac) else {
                return tampered("audit_invalid_mac_encoding", report.files_checked);
            };
            if record
                .integrity
                .previous_mac
                .as_deref()
                .is_some_and(|value| decode_mac(value).is_none())
            {
                return tampered("audit_invalid_mac_encoding", report.files_checked);
            }

            let canonical = match canonical_record(&record) {
                Ok(canonical) => canonical,
                Err(_) => return tampered("audit_canonicalization_failed", report.files_checked),
            };
            let mut verifier = match Hmac::<Sha256>::new_from_slice(key) {
                Ok(verifier) => verifier,
                Err(_) => return unavailable("audit_verifier_unavailable", report.files_checked),
            };
            verifier.update(&canonical);
            if verifier.verify_slice(&actual_mac).is_err() {
                return tampered("audit_mac_mismatch", report.files_checked);
            }

            if let Some(previous_mac) = &last_mac {
                if record.integrity.sequence != report.last_sequence.unwrap_or_default() + 1 {
                    return tampered("audit_sequence_gap", report.files_checked);
                }
                if record.integrity.previous_mac.as_deref() != Some(previous_mac.as_str()) {
                    return tampered("audit_previous_mac_mismatch", report.files_checked);
                }
            } else {
                report.first_sequence = Some(record.integrity.sequence);
            }

            signed_started = true;
            report.signed_events += 1;
            report.last_sequence = Some(record.integrity.sequence);
            last_mac = Some(record.integrity.mac);
        }
    }

    report.status = if report.signed_events == 0 && report.legacy_events == 0 {
        AuditIntegrityStatus::Empty
    } else if report.legacy_events > 0 {
        AuditIntegrityStatus::Legacy
    } else {
        AuditIntegrityStatus::Ok
    };
    report.chain_complete = report.legacy_events == 0 && report.first_sequence.unwrap_or(1) == 1;
    AuditVerificationState {
        report,
        last_mac,
        active_lines,
    }
}

fn sanitize_details(details: Option<Value>) -> Value {
    let serialized = redact_and_bound_json_text(
        &details.unwrap_or(Value::Null).to_string(),
        AUDIT_DETAILS_BYTES,
    );
    serde_json::from_str(&serialized).unwrap_or(Value::Null)
}

fn append_with_key(
    path: &Path,
    key: &[u8; 32],
    area: &str,
    action: &str,
    details: Option<Value>,
    max_file_bytes: u64,
    max_archives: usize,
) -> Result<(), String> {
    let state = verify_chain(path, key, max_archives);
    if !state.report.permits_read() {
        return Err(AUDIT_INTEGRITY_FAILED.to_string());
    }
    let sequence = state
        .report
        .last_sequence
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| AUDIT_INTEGRITY_FAILED.to_string())?;
    let mut record = SignedAuditRecord {
        timestamp: Utc::now().to_rfc3339(),
        area: diagnostic_label(area),
        action: diagnostic_label(action),
        details: sanitize_details(details),
        integrity: AuditIntegrityEnvelope {
            version: AUDIT_INTEGRITY_VERSION,
            sequence,
            previous_mac: state.last_mac,
            mac: String::new(),
        },
    };
    record.integrity.mac = calculate_mac(key, &record)?;
    let serialized =
        serde_json::to_string(&record).map_err(|_| AUDIT_STORAGE_FAILED.to_string())?;
    if serialized.len() > MAX_AUDIT_EVENT_BYTES {
        return Err(AUDIT_STORAGE_FAILED.to_string());
    }

    rotate_if_needed(
        path,
        serialized.len() as u64 + 1,
        max_file_bytes,
        max_archives,
    )
    .map_err(|_| AUDIT_STORAGE_FAILED.to_string())?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| AUDIT_STORAGE_FAILED.to_string())?;
    writeln!(file, "{serialized}").map_err(|_| AUDIT_STORAGE_FAILED.to_string())?;
    file.sync_data()
        .map_err(|_| AUDIT_STORAGE_FAILED.to_string())
}

fn verify_native_state(path: &Path) -> AuditVerificationState {
    let key = match native_signing_key(path) {
        Ok(key) => key,
        Err(_) => return unavailable("audit_signing_key_unavailable", 0),
    };
    let marker_present = match native_history_marker() {
        Ok(marker_present) => marker_present,
        Err(_) => return unavailable("audit_history_marker_unavailable", 0),
    };
    let mut state = verify_chain(path, &key, MAX_AUDIT_ARCHIVES);
    state.report =
        reconcile_history_marker(state.report, marker_present, persist_native_history_marker);
    if !state.report.permits_read() {
        state.active_lines.clear();
    }
    state
}

pub fn verify_audit_integrity(app_data_dir: &Path) -> AuditIntegrityReport {
    let Ok(_write_guard) = AUDIT_WRITE_LOCK.get_or_init(|| Mutex::new(())).lock() else {
        return AuditIntegrityReport::failure(
            AuditIntegrityStatus::Unavailable,
            "audit_writer_lock_unavailable",
            0,
        );
    };
    verify_native_state(&audit_file_path(app_data_dir)).report
}

pub fn verified_audit_tail(
    app_data_dir: &Path,
    limit: usize,
) -> Result<(AuditIntegrityReport, Vec<String>), String> {
    let _write_guard = AUDIT_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| AUDIT_STORAGE_FAILED.to_string())?;
    let mut state = verify_native_state(&audit_file_path(app_data_dir));
    if state.report.permits_read() {
        let start = state.active_lines.len().saturating_sub(limit);
        state.active_lines.drain(..start);
    } else {
        state.active_lines.clear();
    }
    Ok((state.report, state.active_lines))
}

#[derive(Debug, Clone)]
pub struct LegacyAuditJsonlService {
    app_data_dir: PathBuf,
}

impl LegacyAuditJsonlService {
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self { app_data_dir }
    }

    pub fn append_legacy_event(
        &self,
        area: &str,
        action: &str,
        details: Option<Value>,
    ) -> Result<(), String> {
        let _write_guard = AUDIT_WRITE_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| AUDIT_STORAGE_FAILED.to_string())?;
        let audit_dir = self.app_data_dir.join("audit");
        create_dir_all(&audit_dir).map_err(|_| AUDIT_STORAGE_FAILED.to_string())?;
        let file_path = audit_dir.join("events.jsonl");
        let key = native_signing_key(&file_path)?;
        let marker_present = native_history_marker()?;
        let current_report = reconcile_history_marker(
            verify_chain(&file_path, &key, MAX_AUDIT_ARCHIVES).report,
            marker_present,
            persist_native_history_marker,
        );
        if !current_report.permits_read() {
            return Err(AUDIT_INTEGRITY_FAILED.to_string());
        }
        append_with_key(
            &file_path,
            &key,
            area,
            action,
            details,
            MAX_AUDIT_FILE_BYTES,
            MAX_AUDIT_ARCHIVES,
        )?;
        persist_native_history_marker()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_sink::AuditSink;
    use crate::context::{ActorRole, RequestContext};
    use std::fs;
    use std::sync::Mutex;
    use uuid::Uuid;

    #[derive(Default)]
    struct RecordingAuditSink {
        events: Mutex<Vec<AuditEvent>>,
    }

    impl RecordingAuditSink {
        fn events(&self) -> Vec<AuditEvent> {
            self.events.lock().expect("events lock").clone()
        }
    }

    impl AuditSink for RecordingAuditSink {
        fn emit(&self, event: AuditEvent) -> ServiceResult<()> {
            self.events.lock().expect("events lock").push(event);
            Ok(())
        }
    }

    #[test]
    fn audit_service_records_success_event_without_tauri_runtime() {
        let sink = RecordingAuditSink::default();
        let service = AuditService::new(sink);
        let context = RequestContext::test_with_role(ActorRole::Admin);

        let event = service
            .record_success(
                &context,
                "workspace.update",
                AuditTarget::workspace(&context.workspace_id),
                AuditRiskClass::Medium,
            )
            .expect("success audit records");

        assert_eq!(event.action, "workspace.update");
        assert_eq!(event.tenant_id.as_str(), "test-tenant");
        assert_eq!(service.sink.events().len(), 1);
    }

    #[test]
    fn audit_service_records_denial_event_without_tauri_runtime() {
        let sink = RecordingAuditSink::default();
        let service = AuditService::new(sink);
        let context = RequestContext::test_with_role(ActorRole::Viewer);

        let event = service
            .record_denial(
                &context,
                "desktop.input.request",
                AuditTarget::new("desktop_agent", Some("agent_1".to_string())),
                AuditRiskClass::Critical,
            )
            .expect("denial audit records");

        assert_eq!(event.action, "desktop.input.request");
        assert_eq!(event.risk_class, AuditRiskClass::Critical);
        assert_eq!(service.sink.events()[0].outcome, event.outcome);
    }

    #[test]
    fn legacy_jsonl_service_preserves_existing_event_shape() {
        let root = std::env::temp_dir().join(format!("open_cowork_audit_{}", Uuid::new_v4()));
        let service = LegacyAuditJsonlService::new(root.clone());

        service
            .append_legacy_event(
                "test_area",
                "test_action",
                Some(serde_json::json!({ "key": "value" })),
            )
            .expect("legacy event writes");

        let contents =
            fs::read_to_string(root.join("audit").join("events.jsonl")).expect("audit file reads");
        let value: Value = serde_json::from_str(contents.trim()).expect("audit json parses");

        assert!(value["timestamp"].as_str().is_some());
        assert_eq!(value["area"], "test_area");
        assert_eq!(value["action"], "test_action");
        assert_eq!(value["details"]["key"], "value");
        assert_eq!(value["integrity"]["version"], 1);
        assert_eq!(value["integrity"]["sequence"], 1);
        assert!(value["integrity"]["mac"].as_str().is_some());

        let _ = fs::remove_dir_all(root);
    }

    fn test_audit_path() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("open_cowork_audit_{}", Uuid::new_v4()));
        let audit_dir = root.join("audit");
        fs::create_dir_all(&audit_dir).expect("audit dir creates");
        let path = audit_dir.join("events.jsonl");
        (root, path)
    }

    fn append_test_event(path: &Path, index: usize) {
        append_with_key(
            path,
            &[0x24; 32],
            "runtime",
            "test_event",
            Some(serde_json::json!({ "index": index, "status": "original" })),
            MAX_AUDIT_FILE_BYTES,
            MAX_AUDIT_ARCHIVES,
        )
        .expect("signed event appends");
    }

    #[test]
    fn signed_appends_verify_and_increment_sequence() {
        let (root, path) = test_audit_path();
        append_test_event(&path, 1);
        append_test_event(&path, 2);

        let state = verify_chain(&path, &[0x24; 32], MAX_AUDIT_ARCHIVES);
        assert_eq!(state.report.status, AuditIntegrityStatus::Ok);
        assert_eq!(state.report.signed_events, 2);
        assert_eq!(state.report.first_sequence, Some(1));
        assert_eq!(state.report.last_sequence, Some(2));
        assert!(state.report.chain_complete);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_new_service_instance_continues_the_persisted_chain() {
        let root = std::env::temp_dir().join(format!("open_cowork_audit_{}", Uuid::new_v4()));
        LegacyAuditJsonlService::new(root.clone())
            .append_legacy_event("runtime", "first_process", None)
            .expect("first process appends");
        LegacyAuditJsonlService::new(root.clone())
            .append_legacy_event("runtime", "second_process", None)
            .expect("second process appends");

        let report = verify_audit_integrity(&root);
        assert_eq!(report.status, AuditIntegrityStatus::Ok);
        assert_eq!(report.signed_events, 2);
        assert_eq!(report.last_sequence, Some(2));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn modified_details_are_detected_and_block_subsequent_appends() {
        let (root, path) = test_audit_path();
        append_test_event(&path, 1);
        let contents = fs::read_to_string(&path).expect("audit reads");
        fs::write(&path, contents.replace("original", "modified")).expect("tampered audit writes");

        let state = verify_chain(&path, &[0x24; 32], MAX_AUDIT_ARCHIVES);
        assert_eq!(state.report.status, AuditIntegrityStatus::Tampered);
        assert_eq!(
            state.report.error_code.as_deref(),
            Some("audit_mac_mismatch")
        );
        assert_eq!(
            append_with_key(
                &path,
                &[0x24; 32],
                "runtime",
                "blocked",
                None,
                MAX_AUDIT_FILE_BYTES,
                MAX_AUDIT_ARCHIVES,
            ),
            Err(AUDIT_INTEGRITY_FAILED.to_string())
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn noncanonical_signed_json_is_detected_even_when_values_are_unchanged() {
        let (root, path) = test_audit_path();
        append_test_event(&path, 1);
        let contents = fs::read_to_string(&path).expect("audit reads");
        fs::write(&path, contents.replacen('{', "{ ", 1)).expect("audit rewrites");

        let state = verify_chain(&path, &[0x24; 32], MAX_AUDIT_ARCHIVES);
        assert_eq!(state.report.status, AuditIntegrityStatus::Tampered);
        assert_eq!(
            state.report.error_code.as_deref(),
            Some("audit_noncanonical_record")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verified_tail_returns_only_bytes_from_the_verified_snapshot() {
        let root = std::env::temp_dir().join(format!("open_cowork_audit_{}", Uuid::new_v4()));
        let service = LegacyAuditJsonlService::new(root.clone());
        for index in 0..3 {
            service
                .append_legacy_event(
                    "runtime",
                    "snapshot",
                    Some(serde_json::json!({ "index": index })),
                )
                .expect("event appends");
        }

        let (report, lines) = verified_audit_tail(&root, 2).expect("tail verifies");
        assert_eq!(report.status, AuditIntegrityStatus::Ok);
        assert_eq!(lines.len(), 2);
        let first: Value = serde_json::from_str(&lines[0]).expect("first tail line parses");
        let second: Value = serde_json::from_str(&lines[1]).expect("second tail line parses");
        assert_eq!(first["integrity"]["sequence"], 2);
        assert_eq!(second["integrity"]["sequence"], 3);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn deletion_inside_the_retained_chain_is_detected() {
        let (root, path) = test_audit_path();
        for index in 0..3 {
            append_test_event(&path, index);
        }
        let mut lines = fs::read_to_string(&path)
            .expect("audit reads")
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        lines.remove(1);
        fs::write(&path, format!("{}\n", lines.join("\n"))).expect("audit rewrites");

        let state = verify_chain(&path, &[0x24; 32], MAX_AUDIT_ARCHIVES);
        assert_eq!(state.report.status, AuditIntegrityStatus::Tampered);
        assert_eq!(
            state.report.error_code.as_deref(),
            Some("audit_sequence_gap")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reordered_records_are_detected() {
        let (root, path) = test_audit_path();
        for index in 0..3 {
            append_test_event(&path, index);
        }
        let mut lines = fs::read_to_string(&path)
            .expect("audit reads")
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        lines.swap(0, 1);
        fs::write(&path, format!("{}\n", lines.join("\n"))).expect("audit rewrites");

        let state = verify_chain(&path, &[0x24; 32], MAX_AUDIT_ARCHIVES);
        assert_eq!(state.report.status, AuditIntegrityStatus::Tampered);
        assert_eq!(
            state.report.error_code.as_deref(),
            Some("audit_sequence_gap")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_prefix_can_be_migrated_but_remains_degraded() {
        let (root, path) = test_audit_path();
        fs::write(
            &path,
            "{\"timestamp\":\"2026-07-10T12:00:00Z\",\"area\":\"runtime\",\"action\":\"legacy\",\"details\":null}\n",
        )
        .expect("legacy fixture writes");
        append_test_event(&path, 1);

        let state = verify_chain(&path, &[0x24; 32], MAX_AUDIT_ARCHIVES);
        assert_eq!(state.report.status, AuditIntegrityStatus::Legacy);
        assert_eq!(state.report.legacy_events, 1);
        assert_eq!(state.report.signed_events, 1);
        assert!(!state.report.chain_complete);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_record_after_signed_history_is_detected() {
        let (root, path) = test_audit_path();
        append_test_event(&path, 1);
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("audit opens");
        writeln!(
            file,
            "{{\"timestamp\":\"2026-07-10T12:00:00Z\",\"area\":\"runtime\",\"action\":\"legacy\"}}"
        )
        .expect("legacy suffix writes");

        let state = verify_chain(&path, &[0x24; 32], MAX_AUDIT_ARCHIVES);
        assert_eq!(state.report.status, AuditIntegrityStatus::Tampered);
        assert_eq!(
            state.report.error_code.as_deref(),
            Some("audit_legacy_after_signed")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn truncated_last_record_is_detected() {
        let (root, path) = test_audit_path();
        append_test_event(&path, 1);
        let mut bytes = fs::read(&path).expect("audit reads");
        bytes.pop();
        fs::write(&path, bytes).expect("truncated audit writes");

        let state = verify_chain(&path, &[0x24; 32], MAX_AUDIT_ARCHIVES);
        assert_eq!(state.report.status, AuditIntegrityStatus::Tampered);
        assert_eq!(
            state.report.error_code.as_deref(),
            Some("audit_truncated_record")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_complete_record_is_detected() {
        let (root, path) = test_audit_path();
        fs::write(&path, "{not-json}\n").expect("malformed audit writes");

        let state = verify_chain(&path, &[0x24; 32], MAX_AUDIT_ARCHIVES);
        assert_eq!(state.report.status, AuditIntegrityStatus::Tampered);
        assert_eq!(
            state.report.error_code.as_deref(),
            Some("audit_malformed_record")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn retained_rotation_accepts_a_verified_partial_anchor() {
        let (root, path) = test_audit_path();
        for index in 0..5 {
            append_with_key(
                &path,
                &[0x24; 32],
                "runtime",
                "rotated",
                Some(serde_json::json!({ "index": index })),
                1,
                3,
            )
            .expect("rotated event appends");
        }

        let state = verify_chain(&path, &[0x24; 32], 3);
        assert_eq!(state.report.status, AuditIntegrityStatus::Ok);
        assert_eq!(state.report.signed_events, 4);
        assert_eq!(state.report.first_sequence, Some(2));
        assert_eq!(state.report.last_sequence, Some(5));
        assert!(!state.report.chain_complete);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_key_with_history_fails_closed_without_generating_a_replacement() {
        let generated = std::sync::atomic::AtomicBool::new(false);
        let result = create_key_if_allowed(true, || {
            generated.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok([0x55; 32])
        });

        assert_eq!(result, Err(AUDIT_SIGNING_KEY_UNAVAILABLE.to_string()));
        assert!(!generated.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn credential_marker_detects_complete_signed_history_deletion() {
        let report = reconcile_history_marker(AuditIntegrityReport::empty(), true, || Ok(()));

        assert_eq!(report.status, AuditIntegrityStatus::Tampered);
        assert_eq!(
            report.error_code.as_deref(),
            Some("audit_signed_history_missing")
        );
    }

    #[test]
    fn signed_history_fails_closed_when_its_marker_cannot_be_persisted() {
        let mut report = AuditIntegrityReport::empty();
        report.status = AuditIntegrityStatus::Ok;
        report.signed_events = 1;
        report.first_sequence = Some(1);
        report.last_sequence = Some(1);

        let report = reconcile_history_marker(report, false, || {
            Err(AUDIT_SIGNING_KEY_UNAVAILABLE.to_string())
        });

        assert_eq!(report.status, AuditIntegrityStatus::Unavailable);
        assert_eq!(
            report.error_code.as_deref(),
            Some("audit_history_marker_unavailable")
        );
    }

    #[test]
    fn serialized_integrity_report_exposes_only_fixed_path_free_fields() {
        let mut report = AuditIntegrityReport::empty();
        report.status = AuditIntegrityStatus::Tampered;
        report.error_code = Some("audit_mac_mismatch".to_string());
        let value = serde_json::to_value(report).expect("report serializes");
        let object = value.as_object().expect("report is object");
        let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
        keys.sort_unstable();

        assert_eq!(
            keys,
            vec![
                "chainComplete",
                "errorCode",
                "filesChecked",
                "firstSequence",
                "lastSequence",
                "legacyEvents",
                "signedEvents",
                "status",
            ]
        );
        let serialized = value.to_string();
        assert!(!serialized.contains("path"));
        assert!(!serialized.contains("previousMac"));
        assert!(!serialized.contains("signing"));
    }

    #[test]
    fn legacy_jsonl_service_redacts_secrets_before_writing() {
        let root = std::env::temp_dir().join(format!("open_cowork_audit_{}", Uuid::new_v4()));
        let service = LegacyAuditJsonlService::new(root.clone());

        service
            .append_legacy_event(
                "provider",
                "configured",
                Some(serde_json::json!({
                    "apiKey": "must-not-reach-disk",
                    "nested": { "authorization": "Bearer must-not-reach-disk" },
                    "status": "ok"
                })),
            )
            .expect("legacy event writes");

        let contents =
            fs::read_to_string(root.join("audit").join("events.jsonl")).expect("audit file reads");
        assert!(!contents.contains("must-not-reach-disk"));
        assert!(contents.contains("[REDACTED]"));
        assert!(contents.contains("\"status\":\"ok\""));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn audit_rotation_keeps_only_the_configured_archives() {
        let root = std::env::temp_dir().join(format!("open_cowork_audit_{}", Uuid::new_v4()));
        let audit_dir = root.join("audit");
        fs::create_dir_all(&audit_dir).expect("audit dir creates");
        let path = audit_dir.join("events.jsonl");

        for index in 0..5 {
            fs::write(&path, format!("event-{index}-too-large")).expect("active audit file writes");
            rotate_if_needed(&path, 10, 8, 3).expect("audit file rotates");
        }

        assert!(!path.exists());
        assert!(archive_path(&path, 1).exists());
        assert!(archive_path(&path, 2).exists());
        assert!(archive_path(&path, 3).exists());
        assert!(!archive_path(&path, 4).exists());

        let _ = fs::remove_dir_all(root);
    }
}
