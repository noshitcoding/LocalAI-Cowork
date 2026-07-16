use crate::audit_service::{verified_audit_tail, verify_audit_integrity, LegacyAuditJsonlService};
pub use crate::audit_service::{AuditIntegrityReport, AuditIntegrityStatus};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn append_audit_event(
    app_data_dir: PathBuf,
    area: &str,
    action: &str,
    details: Option<Value>,
) -> Result<(), String> {
    LegacyAuditJsonlService::new(app_data_dir).append_legacy_event(area, action, details)
}

pub fn integrity_report(app_data_dir: &Path) -> AuditIntegrityReport {
    verify_audit_integrity(app_data_dir)
}

pub fn verified_tail(
    app_data_dir: &Path,
    limit: usize,
) -> Result<(AuditIntegrityReport, Vec<String>), String> {
    verified_audit_tail(app_data_dir, limit)
}
