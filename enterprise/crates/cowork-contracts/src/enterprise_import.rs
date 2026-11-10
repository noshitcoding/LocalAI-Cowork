//! Source-to-new-target imports preserve the source catalog and never confer old execution authority.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareInstanceImport {
    pub source_instance_id: Uuid,
    pub target_instance_id: Uuid,
    pub source_revision: i64,
    pub target_revision: i64,
    pub backup_id: Uuid,
    pub idempotency_key: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutoverInstanceImport {
    pub expected_revision: i64,
    pub expected_route_revision: i64,
    pub target_instance_id: Uuid,
    pub idempotency_key: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbortInstanceImport {
    pub expected_revision: i64,
    pub idempotency_key: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportStep {
    Quiesce,
    Export,
    Restore,
    Qualify,
}
impl ImportStep {
    pub fn action(self) -> &'static str {
        match self {
            Self::Quiesce => "import_quiesce",
            Self::Export => "import_export",
            Self::Restore => "import_restore",
            Self::Qualify => "import_qualify",
        }
    }
    pub fn from_action(action: &str) -> Option<Self> {
        match action {
            "import_quiesce" => Some(Self::Quiesce),
            "import_export" => Some(Self::Export),
            "import_restore" => Some(Self::Restore),
            "import_qualify" => Some(Self::Qualify),
            _ => None,
        }
    }
    pub fn phase(self) -> &'static str {
        match self {
            Self::Quiesce => "prepared",
            Self::Export => "source_quiescent",
            Self::Restore => "export_verified",
            Self::Qualify => "target_restored_paused",
        }
    }
    pub fn completed_phase(self) -> &'static str {
        match self {
            Self::Quiesce => "source_quiescent",
            Self::Export => "export_verified",
            Self::Restore => "target_restored_paused",
            Self::Qualify => "qualified",
        }
    }
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Quiesce => Some(Self::Export),
            Self::Export => Some(Self::Restore),
            Self::Restore => Some(Self::Qualify),
            Self::Qualify => None,
        }
    }
    pub fn target(self) -> bool {
        matches!(self, Self::Restore | Self::Qualify)
    }
}

/// Server-owned immutable binding. Receipt echoes are compared to this exact document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceImportBinding {
    pub version: u32,
    pub import_id: Uuid,
    pub organisation_id: Uuid,
    pub source: Value,
    pub target: Value,
    pub source_host_binding: Value,
    pub target_host_binding: Value,
    pub source_allocation: Value,
    pub target_allocation: Value,
    pub source_epoch: Uuid,
    pub target_epoch: Uuid,
    pub candidate_epoch: Uuid,
    pub backup: Value,
    pub reservation_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportExportProof {
    pub backup_id: Uuid,
    pub backup_sha256: String,
    pub backup_bytes: u64,
    pub manifest_sha256: String,
    pub images: Value,
    pub workspace_count: u32,
    pub inventory_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportTargetProof {
    pub instance_id: Uuid,
    pub execution_epoch: Uuid,
    pub manifest_sha256: String,
    pub inventory_sha256: String,
    pub credentials_rotated: bool,
    pub schedules_paused: bool,
    pub unfinished_runs_interrupted: bool,
    pub historical_resume_blocked: bool,
    pub workspace_inventory_complete: bool,
    pub bounded_job_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceImportReceipt {
    pub version: u32,
    pub import_id: Uuid,
    pub operation_id: Uuid,
    pub step: ImportStep,
    pub binding_sha256: String,
    pub source_inventory_sha256: String,
    pub source_stopped: bool,
    pub target_stopped: bool,
    pub export: Option<ImportExportProof>,
    pub target: Option<ImportTargetProof>,
}

pub fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

impl InstanceImportReceipt {
    pub fn validate(
        &self,
        binding: &InstanceImportBinding,
        operation: Uuid,
        step: ImportStep,
        digest: &str,
        original_manifest: Option<&str>,
    ) -> Result<(), &'static str> {
        if self.version != 1
            || self.import_id != binding.import_id
            || self.operation_id != operation
            || self.step != step
            || !sha256(digest)
            || self.binding_sha256 != digest
            || !sha256(&self.source_inventory_sha256)
            || !self.source_stopped
            || (step.target() && !self.target_stopped)
            || (!step.target() && self.target_stopped)
        {
            return Err("import receipt identity or physical stop mismatch");
        }
        if step == ImportStep::Export {
            let export = self
                .export
                .as_ref()
                .ok_or("verified original export is required")?;
            if self.target.is_some()
                || export.backup_id.to_string() != binding.backup["id"].as_str().unwrap_or("")
                || export.backup_sha256 != binding.backup["sha256"].as_str().unwrap_or("")
                || Some(export.backup_bytes) != binding.backup["size_bytes"].as_u64()
                || !sha256(&export.manifest_sha256)
                || !sha256(&export.inventory_sha256)
                || !export.images.is_object()
            {
                return Err("original backup catalog provenance mismatch");
            }
        } else if step.target() {
            let target = self
                .target
                .as_ref()
                .ok_or("verified paused target is required")?;
            if self.export.is_some()
                || Some(target.instance_id.to_string().as_str()) != binding.target["id"].as_str()
                || target.execution_epoch != binding.candidate_epoch
                || Some(target.manifest_sha256.as_str()) != original_manifest
                || !sha256(&target.inventory_sha256)
                || !(target.credentials_rotated
                    && target.schedules_paused
                    && target.unfinished_runs_interrupted
                    && target.historical_resume_blocked
                    && target.workspace_inventory_complete)
                || (step == ImportStep::Qualify
                    && !target.bounded_job_sha256.as_deref().is_some_and(sha256))
            {
                return Err("target import authority or qualification mismatch");
            }
        } else if self.export.is_some() || self.target.is_some() {
            return Err("quiescence receipt contains unexpected export or target proof");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_has_no_caller_supplied_host_epoch_credentials_or_receipt() {
        let input = serde_json::json!({"source_instance_id":Uuid::new_v4(),"target_instance_id":Uuid::new_v4(),
            "source_revision":1,"target_revision":1,"backup_id":Uuid::new_v4(),"idempotency_key":Uuid::new_v4(),"candidate_epoch":Uuid::new_v4()});
        assert!(serde_json::from_value::<PrepareInstanceImport>(input).is_err());
    }
    #[test]
    fn import_actions_never_alias_ordinary_restore() {
        assert_eq!(ImportStep::from_action("restore"), None);
        assert_eq!(ImportStep::Restore.action(), "import_restore");
        assert_eq!(ImportStep::Qualify.next(), None);
    }
    fn fixture(step: ImportStep) -> (InstanceImportBinding, InstanceImportReceipt) {
        let backup = Uuid::new_v4();
        let source = Uuid::new_v4();
        let target = Uuid::new_v4();
        let binding = InstanceImportBinding {
            version: 1,
            import_id: Uuid::new_v4(),
            organisation_id: Uuid::new_v4(),
            source: serde_json::json!({"id":source}),
            target: serde_json::json!({"id":target}),
            source_host_binding: Value::Null,
            target_host_binding: Value::Null,
            source_allocation: Value::Null,
            target_allocation: Value::Null,
            source_epoch: Uuid::new_v4(),
            target_epoch: Uuid::new_v4(),
            candidate_epoch: Uuid::new_v4(),
            backup: serde_json::json!({"id":backup,"sha256":"a".repeat(64),"size_bytes":123}),
            reservation_ids: vec![],
        };
        let receipt = InstanceImportReceipt {
            version: 1,
            import_id: binding.import_id,
            operation_id: Uuid::new_v4(),
            step,
            binding_sha256: "b".repeat(64),
            source_inventory_sha256: "c".repeat(64),
            source_stopped: true,
            target_stopped: step.target(),
            export: if step == ImportStep::Export {
                Some(ImportExportProof {
                    backup_id: backup,
                    backup_sha256: "a".repeat(64),
                    backup_bytes: 123,
                    manifest_sha256: "d".repeat(64),
                    images: serde_json::json!({"server":"pinned"}),
                    workspace_count: 0,
                    inventory_sha256: "e".repeat(64),
                })
            } else {
                None
            },
            target: if step.target() {
                Some(ImportTargetProof {
                    instance_id: target,
                    execution_epoch: binding.candidate_epoch,
                    manifest_sha256: "d".repeat(64),
                    inventory_sha256: "e".repeat(64),
                    credentials_rotated: true,
                    schedules_paused: true,
                    unfinished_runs_interrupted: true,
                    historical_resume_blocked: true,
                    workspace_inventory_complete: true,
                    bounded_job_sha256: if step == ImportStep::Qualify {
                        Some("f".repeat(64))
                    } else {
                        None
                    },
                })
            } else {
                None
            },
        };
        (binding, receipt)
    }
    fn valid(binding: &InstanceImportBinding, receipt: &InstanceImportReceipt) -> bool {
        receipt
            .validate(
                binding,
                receipt.operation_id,
                receipt.step,
                &"b".repeat(64),
                Some(&"d".repeat(64)),
            )
            .is_ok()
    }
    #[test]
    fn each_phase_requires_its_exact_complete_receipt() {
        for step in [
            ImportStep::Quiesce,
            ImportStep::Export,
            ImportStep::Restore,
            ImportStep::Qualify,
        ] {
            let (binding, receipt) = fixture(step);
            assert!(valid(&binding, &receipt));
            let mut forged = receipt.clone();
            forged.operation_id = Uuid::new_v4();
            assert!(forged
                .validate(
                    &binding,
                    receipt.operation_id,
                    step,
                    &"b".repeat(64),
                    Some(&"d".repeat(64))
                )
                .is_err());
            forged = receipt.clone();
            forged.source_stopped = false;
            assert!(!valid(&binding, &forged));
            forged = receipt.clone();
            forged.binding_sha256 = "0".repeat(64);
            assert!(!valid(&binding, &forged));
        }
    }
    #[test]
    fn original_source_catalog_cannot_be_reassigned_or_truncated() {
        let (binding, receipt) = fixture(ImportStep::Export);
        let mut forged = receipt.clone();
        forged.export.as_mut().unwrap().backup_id = Uuid::new_v4();
        assert!(!valid(&binding, &forged));
        forged = receipt.clone();
        forged.export.as_mut().unwrap().backup_bytes -= 1;
        assert!(!valid(&binding, &forged));
        forged = receipt.clone();
        forged.target_stopped = true;
        assert!(!valid(&binding, &forged));
    }
    #[test]
    fn copied_authority_and_missing_fences_refuse_qualification() {
        let (binding, receipt) = fixture(ImportStep::Qualify);
        for select in 0..7 {
            let mut forged = receipt.clone();
            let target = forged.target.as_mut().unwrap();
            match select {
                0 => target.execution_epoch = binding.source_epoch,
                1 => target.credentials_rotated = false,
                2 => target.schedules_paused = false,
                3 => target.unfinished_runs_interrupted = false,
                4 => target.historical_resume_blocked = false,
                5 => target.workspace_inventory_complete = false,
                _ => target.bounded_job_sha256 = None,
            };
            assert!(!valid(&binding, &forged));
        }
        let mut forged = receipt.clone();
        forged.target.as_mut().unwrap().manifest_sha256 = "a".repeat(64);
        assert!(!valid(&binding, &forged));
    }
    #[test]
    fn phase_order_is_forward_only_and_receipts_are_strict() {
        let mut step = ImportStep::Quiesce;
        let mut count = 1;
        while let Some(next) = step.next() {
            assert_eq!(step.completed_phase(), next.phase());
            step = next;
            count += 1;
        }
        assert_eq!(count, 4);
        assert_eq!(step.completed_phase(), "qualified");
        let (_, receipt) = fixture(ImportStep::Qualify);
        let mut value = serde_json::to_value(receipt).unwrap();
        value["target"]["caller_jwt"] = serde_json::json!("forged");
        assert!(serde_json::from_value::<InstanceImportReceipt>(value).is_err());
    }
}
