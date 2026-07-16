use crate::audit_service::AuditIntegrityReport;
use crate::db::StartupRecoveryReport;
use crate::sensitive_data::diagnostic_label;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use uuid::Uuid;
use zip::write::FileOptions;

const SUPPORT_BUNDLE_SCHEMA_VERSION: u32 = 1;
const MAX_AUDIT_TAIL_EVENTS: usize = 500;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportBundleResponse {
    pub path: String,
    pub size_bytes: u64,
    pub created_at: String,
    pub file_count: usize,
}

struct BundleEntry {
    path: &'static str,
    bytes: Vec<u8>,
}

fn safe_timestamp(value: Option<&Value>) -> Value {
    value
        .and_then(Value::as_str)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| Value::String(timestamp.to_rfc3339()))
        .unwrap_or(Value::Null)
}

fn safe_bool(value: Option<&Value>) -> Value {
    Value::Bool(value.and_then(Value::as_bool).unwrap_or(false))
}

fn safe_count(value: Option<&Value>) -> Value {
    Value::Number(value.and_then(Value::as_u64).unwrap_or(0).into())
}

fn safe_label(value: Option<&Value>) -> Value {
    Value::String(diagnostic_label(
        value.and_then(Value::as_str).unwrap_or_default(),
    ))
}

fn whitelist_rows(value: &Value, fields: &[(&str, fn(Option<&Value>) -> Value)]) -> Value {
    let rows = value
        .as_array()
        .into_iter()
        .flatten()
        .take(100)
        .filter_map(Value::as_object)
        .map(|row| {
            Value::Object(
                fields
                    .iter()
                    .map(|(field, sanitizer)| ((*field).to_string(), sanitizer(row.get(*field))))
                    .collect::<Map<_, _>>(),
            )
        })
        .collect::<Vec<_>>();
    Value::Array(rows)
}

fn whitelist_database_diagnostics(input: &Value) -> Value {
    const COUNT_FIELDS: &[&str] = &[
        "chatThreads",
        "engineRuns",
        "engineRunEvents",
        "scheduledTasks",
        "scheduledRuns",
        "crewRuns",
        "crewRunLogs",
        "crewRunEvents",
        "auditEvents",
    ];
    let empty = Map::new();
    let input = input.as_object().unwrap_or(&empty);
    let count_input = input
        .get("counts")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let counts = COUNT_FIELDS
        .iter()
        .map(|field| ((*field).to_string(), safe_count(count_input.get(*field))))
        .collect::<Map<_, _>>();

    json!({
        "schemaVersion": input.get("schemaVersion").and_then(Value::as_u64).unwrap_or(0),
        "counts": counts,
        "recentEngineRuns": whitelist_rows(
            input.get("recentEngineRuns").unwrap_or(&Value::Null),
            &[
                ("status", safe_label),
                ("phase", safe_label),
                ("createdAt", safe_timestamp),
                ("updatedAt", safe_timestamp),
                ("endedAt", safe_timestamp),
                ("hasError", safe_bool),
            ],
        ),
        "recentEngineEvents": whitelist_rows(
            input.get("recentEngineEvents").unwrap_or(&Value::Null),
            &[
                ("eventType", safe_label),
                ("redactionLevel", safe_label),
                ("createdAt", safe_timestamp),
            ],
        ),
        "recentScheduledRuns": whitelist_rows(
            input.get("recentScheduledRuns").unwrap_or(&Value::Null),
            &[
                ("status", safe_label),
                ("startedAt", safe_timestamp),
                ("finishedAt", safe_timestamp),
                ("hasError", safe_bool),
            ],
        ),
        "recentCrewRuns": whitelist_rows(
            input.get("recentCrewRuns").unwrap_or(&Value::Null),
            &[
                ("process", safe_label),
                ("status", safe_label),
                ("startedAt", safe_timestamp),
                ("finishedAt", safe_timestamp),
                ("hasError", safe_bool),
            ],
        ),
    })
}

fn whitelist_startup_recovery(report: &StartupRecoveryReport) -> Value {
    let recovered_at = Value::String(report.recovered_at.clone());
    json!({
        "recoveredAt": safe_timestamp(Some(&recovered_at)),
        "engineRuns": report.engine_runs,
        "legacyTasks": report.legacy_tasks,
        "taskSteps": report.task_steps,
        "workTasks": report.work_tasks,
        "scheduledRuns": report.scheduled_runs,
        "crewRuns": report.crew_runs,
        "workerSandboxes": report.worker_sandboxes,
        "managedProcesses": report.managed_processes,
        "terminalBackends": report.terminal_backends,
    })
}

fn whitelist_audit_integrity(report: &AuditIntegrityReport) -> Value {
    json!({
        "status": report.status,
        "signedEvents": report.signed_events,
        "legacyEvents": report.legacy_events,
        "filesChecked": report.files_checked,
        "firstSequence": report.first_sequence,
        "lastSequence": report.last_sequence,
        "chainComplete": report.chain_complete,
        "errorCode": report.error_code,
    })
}

fn audit_tail(audit_lines: &[String], audit_integrity: &AuditIntegrityReport) -> Vec<u8> {
    if !audit_integrity.permits_read() || audit_lines.is_empty() {
        return Vec::new();
    }

    let mut lines = audit_lines
        .iter()
        .rev()
        .take(MAX_AUDIT_TAIL_EVENTS)
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|event| {
            let event = event.as_object()?;
            Some(
                json!({
                    "timestamp": safe_timestamp(event.get("timestamp")),
                    "area": safe_label(event.get("area")),
                    "action": safe_label(event.get("action")),
                })
                .to_string(),
            )
        })
        .collect::<Vec<_>>();
    lines.reverse();
    let mut output = lines.join("\n").into_bytes();
    if !output.is_empty() {
        output.push(b'\n');
    }
    output
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_target(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("support bundle path must be absolute".to_string());
    }
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| !extension.eq_ignore_ascii_case("zip"))
        .unwrap_or(true)
    {
        return Err("support bundle path must use the .zip extension".to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "support bundle path has no parent directory".to_string())?;
    if !parent.is_dir() {
        return Err("support bundle parent directory does not exist".to_string());
    }
    if path.is_dir() {
        return Err("support bundle target is a directory".to_string());
    }
    Ok(())
}

pub fn create(
    target: &Path,
    app_version: &str,
    database_diagnostics: &Value,
    startup_recovery: &StartupRecoveryReport,
    audit_lines: &[String],
    audit_integrity: &AuditIntegrityReport,
) -> Result<SupportBundleResponse, String> {
    validate_target(target)?;
    let created_at = Utc::now().to_rfc3339();
    let system = json!({
        "appVersion": diagnostic_label(app_version),
        "bundleSchemaVersion": SUPPORT_BUNDLE_SCHEMA_VERSION,
        "createdAt": created_at,
        "platform": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "startupRecovery": whitelist_startup_recovery(startup_recovery),
        "auditIntegrity": whitelist_audit_integrity(audit_integrity),
    });
    let database = whitelist_database_diagnostics(database_diagnostics);
    let readme = concat!(
        "Open_Cowork support bundle\n\n",
        "Included: app/runtime metadata, aggregate database diagnostics, recent run states, ",
        "audit integrity status, and verified audit event names/timestamps.\n",
        "Excluded: database files, prompts, responses, tool input/output, file contents, paths, ",
        "provider configuration, environment variables, credentials, and audit details.\n",
    );
    let mut entries = vec![
        BundleEntry {
            path: "README.txt",
            bytes: readme.as_bytes().to_vec(),
        },
        BundleEntry {
            path: "diagnostics/system.json",
            bytes: serde_json::to_vec_pretty(&system).map_err(|error| error.to_string())?,
        },
        BundleEntry {
            path: "diagnostics/database.json",
            bytes: serde_json::to_vec_pretty(&database).map_err(|error| error.to_string())?,
        },
        BundleEntry {
            path: "logs/audit.jsonl",
            bytes: audit_tail(audit_lines, audit_integrity),
        },
    ];
    let manifest_files = entries
        .iter()
        .map(|entry| {
            json!({
                "path": entry.path,
                "bytes": entry.bytes.len(),
                "sha256": sha256_hex(&entry.bytes),
            })
        })
        .collect::<Vec<_>>();
    let manifest = json!({
        "schemaVersion": SUPPORT_BUNDLE_SCHEMA_VERSION,
        "createdAt": created_at,
        "redactionPolicy": "whitelist-v1",
        "files": manifest_files,
    });
    entries.push(BundleEntry {
        path: "manifest.json",
        bytes: serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    });

    let temporary_path = target.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let write_result = (|| -> Result<(), String> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .map_err(|error| error.to_string())?;
        let mut zip = zip::ZipWriter::new(file);
        let options: FileOptions<'_, ()> = FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o600);
        for entry in &entries {
            zip.start_file(entry.path, options)
                .map_err(|error| error.to_string())?;
            zip.write_all(&entry.bytes)
                .map_err(|error| error.to_string())?;
        }
        let file = zip.finish().map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }

    let previous_path = target.with_extension(format!("{}.previous", Uuid::new_v4()));
    let had_previous = target.exists();
    if had_previous {
        fs::rename(target, &previous_path).map_err(|error| {
            let _ = fs::remove_file(&temporary_path);
            error.to_string()
        })?;
    }
    if let Err(error) = fs::rename(&temporary_path, target) {
        let _ = fs::remove_file(&temporary_path);
        if had_previous {
            let _ = fs::rename(&previous_path, target);
        }
        return Err(error.to_string());
    }
    if had_previous {
        let _ = fs::remove_file(previous_path);
    }
    let size_bytes = fs::metadata(target)
        .map_err(|error| error.to_string())?
        .len();

    Ok(SupportBundleResponse {
        path: target.display().to_string(),
        size_bytes,
        created_at,
        file_count: entries.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_service::AuditIntegrityStatus;
    use std::io::Read;

    fn audit_integrity(status: AuditIntegrityStatus) -> AuditIntegrityReport {
        AuditIntegrityReport {
            status,
            signed_events: 0,
            legacy_events: 1,
            files_checked: 1,
            first_sequence: None,
            last_sequence: None,
            chain_complete: false,
            error_code: None,
        }
    }

    #[test]
    fn bundle_is_whitelisted_manifested_and_contains_no_input_secrets() {
        let root = std::env::temp_dir().join(format!("open_cowork_bundle_{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("test root creates");
        let audit_lines = vec![
            r#"{"timestamp":"2026-07-10T12:00:00Z","area":"runtime","action":"failed","details":{"unknown":"SENTINEL_AUDIT_SECRET","apiKey":"SENTINEL_KEY"}}"#.to_string(),
        ];
        let target = root.join("support.zip");
        let recovery = StartupRecoveryReport {
            recovered_at: "2026-07-10T12:00:00Z".to_string(),
            engine_runs: 1,
            ..StartupRecoveryReport::default()
        };
        let diagnostics = json!({
            "schemaVersion": 23,
            "counts": { "engineRuns": 2, "unknown": "SENTINEL_COUNT_SECRET" },
            "recentEngineRuns": [{
                "status": "failed",
                "phase": "execute",
                "createdAt": "2026-07-10T12:00:00Z",
                "updatedAt": "2026-07-10T12:01:00Z",
                "endedAt": "2026-07-10T12:01:00Z",
                "hasError": true,
                "unknown": "SENTINEL_DATABASE_SECRET"
            }],
            "unknown": "SENTINEL_ROOT_SECRET"
        });

        let integrity = audit_integrity(AuditIntegrityStatus::Legacy);
        let response = create(
            &target,
            "0.1.0",
            &diagnostics,
            &recovery,
            &audit_lines,
            &integrity,
        )
        .expect("support bundle creates");
        assert_eq!(response.file_count, 5);

        let file = fs::File::open(&target).expect("bundle opens");
        let mut archive = zip::ZipArchive::new(file).expect("bundle is zip");
        let names = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .collect::<Vec<_>>();
        assert!(names.contains(&"manifest.json".to_string()));
        assert!(names.contains(&"diagnostics/database.json".to_string()));
        assert!(!names.iter().any(|name| name.ends_with(".db")));

        let mut combined = String::new();
        for index in 0..archive.len() {
            archive
                .by_index(index)
                .expect("entry opens")
                .read_to_string(&mut combined)
                .expect("entry reads");
        }
        assert!(!combined.contains("SENTINEL_"));
        assert!(combined.contains("whitelist-v1"));
        assert!(combined.contains("\"engineRuns\": 2"));
        assert!(combined.contains("\"startupRecovery\""));
        assert!(combined.contains("\"auditIntegrity\""));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bundle_rejects_relative_and_non_zip_targets() {
        let diagnostics = json!({});
        let recovery = StartupRecoveryReport::default();
        let integrity = audit_integrity(AuditIntegrityStatus::Empty);
        assert!(create(
            Path::new("support.zip"),
            "0.1.0",
            &diagnostics,
            &recovery,
            &[],
            &integrity,
        )
        .is_err());
        let root = std::env::temp_dir();
        assert!(create(
            &root.join("support.txt"),
            "0.1.0",
            &diagnostics,
            &recovery,
            &[],
            &integrity,
        )
        .is_err());
    }

    #[test]
    fn tampered_audit_is_reported_but_its_event_tail_is_omitted() {
        let root = std::env::temp_dir().join(format!("open_cowork_bundle_{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("test root creates");
        let audit_lines = vec![
            "{\"timestamp\":\"2026-07-10T12:00:00Z\",\"area\":\"SENTINEL_TAMPERED_AREA\",\"action\":\"failed\"}".to_string(),
        ];
        let target = root.join("support.zip");
        let mut integrity = audit_integrity(AuditIntegrityStatus::Tampered);
        integrity.error_code = Some("audit_mac_mismatch".to_string());

        create(
            &target,
            "0.1.0",
            &json!({}),
            &StartupRecoveryReport::default(),
            &audit_lines,
            &integrity,
        )
        .expect("support bundle creates");

        let file = fs::File::open(&target).expect("bundle opens");
        let mut archive = zip::ZipArchive::new(file).expect("bundle is zip");
        let mut audit = String::new();
        archive
            .by_name("logs/audit.jsonl")
            .expect("audit entry exists")
            .read_to_string(&mut audit)
            .expect("audit entry reads");
        assert!(audit.is_empty());

        let mut system = String::new();
        archive
            .by_name("diagnostics/system.json")
            .expect("system entry exists")
            .read_to_string(&mut system)
            .expect("system entry reads");
        assert!(system.contains("\"status\": \"tampered\""));
        assert!(system.contains("audit_mac_mismatch"));
        assert!(!system.contains("SENTINEL_TAMPERED_AREA"));

        let _ = fs::remove_dir_all(root);
    }
}
