//! Internal original-pin boundary and non-deserializable physical IO permits.
use super::verifier::{HistoricalPins, Sha256Pin};
use crate::{Error, Result};
pub use cowork_contracts::historical_import::*;
use uuid::Uuid;
pub const CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_INTAKE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_PROJECTED_BYTES: usize = 16 * 1024 * 1024;
pub fn checked_pins(value: &HistoricalPinSet) -> Result<HistoricalPins> {
    let pin = |v: &str| {
        Sha256Pin::parse(v).map_err(|_| Error::Validation("Invalid original historical pin".into()))
    };
    Ok(HistoricalPins {
        quarantine_manifest: pin(&value.quarantine_manifest)?,
        quarantine_database: pin(&value.quarantine_database)?,
        export_manifest: pin(&value.export_manifest)?,
        mapping: pin(&value.mapping)?,
        preview: pin(&value.preview)?,
        cold_target_snapshot: pin(&value.cold_target_snapshot)?,
    })
}
/// Returned only by persisted first admission. Original uncertain IO is reconciled, never resent.
pub struct HistoricalFileStagePermit {
    pub(crate) operation: HistoricalFileOperation,
    pub(crate) send: bool,
}
impl HistoricalFileStagePermit {
    pub fn operation(&self) -> &HistoricalFileOperation {
        &self.operation
    }
    pub fn should_send(&self) -> bool {
        self.send
    }
}
/// Created only by a native/S3 staging adapter after verified current-target IO.
/// Never Deserialize: a renderer cannot invent a physical confirmation.
pub struct HistoricalStagedFile {
    pub(crate) preview_id: Uuid,
    pub(crate) plan_sha256: String,
    pub(crate) source_id: String,
    pub(crate) target_id: String,
    pub(crate) project_id: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) payload: serde_json::Value,
    pub(crate) original_receipt: serde_json::Value,
}
impl HistoricalStagedFile {
    pub fn payload(&self) -> &serde_json::Value {
        &self.payload
    }
    pub fn original_receipt(&self) -> &serde_json::Value {
        &self.original_receipt
    }
    /// Trusted adapter boundary; SQL admission still verifies exact original plan, digest and target.
    pub fn from_confirmed_adapter(
        preview_id: Uuid,
        plan_sha256: String,
        source_id: String,
        target_id: String,
        project_id: String,
        size: u64,
        sha256: String,
        payload: serde_json::Value,
        original_receipt: serde_json::Value,
    ) -> Self {
        Self {
            preview_id,
            plan_sha256,
            source_id,
            target_id,
            project_id,
            size,
            sha256,
            payload,
            original_receipt,
        }
    }
}
