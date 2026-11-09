//! Original, nonportable resource ownership after an explicitly accepted delivery.
//! A receipt or deserialized journal cannot instantiate this state machine.
use super::{
    tasks::{CompletedVolumeRetirementSeal, ManagedTaskOutcome},
    *,
};
use crate::volumes::{PublishedVolume, RetiredPublishedVolume, RetiringPublishedVolume};
use cowork_runner_protocol::{Receipt, Signed};

/// Local private evidence. Only its digest belongs in the portable signed status.
#[derive(Clone, Serialize)]
pub struct ManagedRetirementEvidence {
    format_version: u32,
    execution_receipt_sha256: String,
    physical_record_sha256: String,
    workspace: RetiredPublishedVolume,
    control: RetiredPublishedVolume,
}
impl ManagedRetirementEvidence {
    pub fn sha256(&self) -> String {
        cowork_domain::canonical_hash(self)
    }
    pub fn physical_record_sha256(&self) -> &str {
        &self.physical_record_sha256
    }
}

/// Owns the actual original outcome or its moved resource handles. It remains
/// process-local across a partial cleanup; no Clone, Deserialize or public ctor.
pub struct ManagedRetirement {
    receipt: Signed<Receipt>,
    physical: Option<Arc<ManagedTaskOutcome>>,
    resources: Option<RetiringTaskResources>,
    physical_record_sha256: String,
    completed: Option<ManagedRetirementEvidence>,
}
impl ManagedRetirement {
    pub(super) fn new(receipt: Signed<Receipt>, physical: Arc<ManagedTaskOutcome>) -> Self {
        let physical_record_sha256 = physical.physical_record_sha256.clone();
        Self {
            receipt,
            physical: Some(physical),
            resources: None,
            physical_record_sha256,
            completed: None,
        }
    }
    /// A busy original borrower causes no cleanup. Each completed component is
    /// cached while an unfinished sibling keeps its own actual reservation.
    pub fn retire(&mut self) -> Result<&ManagedRetirementEvidence> {
        if self.completed.is_some() {
            return self.completed.as_ref().ok_or(Error::ManagedUncertain);
        }
        if self.resources.is_none() {
            let shared = self.physical.take().ok_or(Error::ManagedUncertain)?;
            let physical = match Arc::try_unwrap(shared) {
                Ok(original) => original,
                Err(original) => {
                    self.physical = Some(original);
                    return Err(Error::ManagedBusy);
                }
            };
            let seal = match physical.retirement_seal(&self.receipt) {
                Ok(seal) => seal,
                Err(error) => {
                    self.physical = Some(Arc::new(physical));
                    return Err(error);
                }
            };
            self.resources = Some(physical.into_retirement_parts(seal));
        }
        let (workspace, control) = self
            .resources
            .as_mut()
            .ok_or(Error::ManagedUncertain)?
            .retire()?;
        self.completed = Some(ManagedRetirementEvidence {
            format_version: 1,
            execution_receipt_sha256: cowork_domain::canonical_hash(&self.receipt),
            physical_record_sha256: self.physical_record_sha256.clone(),
            workspace,
            control,
        });
        self.completed.as_ref().ok_or(Error::ManagedUncertain)
    }
    /// The service must persist the final signed status and local evidence in
    /// its bounded journal before collecting these original empty namespaces.
    pub fn collect_metadata(&mut self) -> Result<()> {
        if self.completed.is_none() {
            return Err(Error::ManagedUncertain);
        }
        self.resources
            .as_mut()
            .ok_or(Error::ManagedUncertain)?
            .collect_metadata()
    }
}

pub(super) struct RetiringTaskResources {
    seal: CompletedVolumeRetirementSeal,
    workspace: HeldVolume,
    control: HeldVolume,
}
impl RetiringTaskResources {
    pub(super) fn new(
        workspace: PublishedVolume,
        control: PublishedVolume,
        seal: CompletedVolumeRetirementSeal,
    ) -> Self {
        Self {
            seal,
            workspace: HeldVolume::new(workspace),
            control: HeldVolume::new(control),
        }
    }
    fn retire(&mut self) -> Result<(RetiredPublishedVolume, RetiredPublishedVolume)> {
        let workspace = self.workspace.retire(&self.seal, "workspace")?;
        let control = self.control.retire(&self.seal, "control")?;
        Ok((workspace, control))
    }
    fn collect_metadata(&mut self) -> Result<()> {
        self.workspace.collect_metadata("workspace_metadata")?;
        self.control.collect_metadata("control_metadata")
    }
}

struct HeldVolume {
    published: Option<Box<PublishedVolume>>,
    retiring: Option<Box<RetiringPublishedVolume>>,
}
impl HeldVolume {
    fn new(original: PublishedVolume) -> Self {
        Self {
            published: Some(Box::new(original)),
            retiring: None,
        }
    }
    fn retire(
        &mut self,
        seal: &CompletedVolumeRetirementSeal,
        stage: &'static str,
    ) -> Result<RetiredPublishedVolume> {
        if self.retiring.is_none() {
            let original = self.published.take().ok_or(Error::ManagedUncertain)?;
            match original.into_retirement(seal) {
                Ok(retiring) => self.retiring = Some(Box::new(retiring)),
                Err(failure) => {
                    let (error, original) = *failure;
                    self.published = Some(Box::new(original));
                    return Err(volume_error(stage, &error));
                }
            }
        }
        self.retiring
            .as_mut()
            .ok_or(Error::ManagedUncertain)?
            .retire()
            .map_err(|error| volume_error(stage, &error))
    }
    fn collect_metadata(&mut self, stage: &'static str) -> Result<()> {
        let original = self.retiring.as_mut().ok_or(Error::ManagedUncertain)?;
        original
            .collect_metadata()
            .map_err(|error| volume_error(stage, &error))
    }
}
fn volume_error(stage: &'static str, error: &impl std::fmt::Debug) -> Error {
    Error::Retirement {
        stage,
        detail: format!("{error:?}").chars().take(1024).collect(),
    }
}
