//! One original root contains both signed inputs and all nine prepared trees.
//! Publication does not activate a runtime or admit any workload. Unknown I/O
//! effects preserve original process-local custody; there is no rename replay.
use super::*;
use std::os::fd::AsRawFd;
mod journal;
use journal::OriginalJournal;
pub use journal::{PublicationHistory, read_publication_history};

pub struct PublicationPolicy {
    input_bytes: u64,
    installation: InstallationLimits,
    descriptors: TargetDescriptorPolicy,
}
impl PublicationPolicy {
    pub fn new(
        input_bytes: u64,
        installation: InstallationLimits,
        descriptors: TargetDescriptorPolicy,
    ) -> Result<Self> {
        if input_bytes == 0 || input_bytes > MAX_PACKAGE {
            return Err(Error::Archive("publication_input_budget"));
        }
        Ok(Self {
            input_bytes,
            installation,
            descriptors,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationPhase {
    Prepared,
    IntentConfirmed,
    DispatchedUnconfirmed,
    Published,
}

struct Destination {
    parent: Dir,
    parent_id: native::Identity,
    name: String,
}

/// No Clone, Deserialize or constructor from receipt/path/record. This retains
/// the original root, original source files and every expanded target inode.
pub struct PublicationAttempt {
    root: StagingArea,
    prepared: PreparedInstallation,
    journal: OriginalJournal,
    destination: Option<Destination>,
    phase: PublicationPhase,
}

#[derive(Serialize)]
pub struct PublicationReceipt<'a> {
    installation: InstallationReceipt<'a>,
    publication_phase: PublicationPhase,
    version_directory: Option<&'a str>,
    journal_directory: &'a str,
    journal_confirmed: bool,
    published: bool,
    directory_sync_confirmed: bool,
    installed: bool,
    activated: bool,
    anti_rollback_applied: bool,
    qualified: bool,
    execution_authority: bool,
    native_custody_reconstructible: bool,
}

impl PublicationAttempt {
    pub fn prepare(
        root: StagingArea,
        source: &Dir,
        trust: &TrustRoot,
        platform: Platform,
        policy: PublicationPolicy,
    ) -> Result<Self> {
        if unsafe { libc::geteuid() } != 0 {
            return Err(Error::Archive("root_manager_required"));
        }
        root.check()?;
        exact_entries(&root.directory, &[])?;
        if policy.input_bytes > root.payload_limit {
            return Err(Error::Archive("publication_input_budget"));
        }
        let input =
            StagingArea::create(root.directory.try_clone()?, "package", policy.input_bytes)?;
        let inspected = input
            .stage(source, trust, platform)?
            .inspect_installation(policy.installation)?;
        let input_bytes =
            inspected
                .staged
                .manifest()
                .components
                .iter()
                .try_fold(0u64, |n, item| {
                    n.checked_add(item.bytes)
                        .ok_or(Error::Archive("publication_payload_budget"))
                })?;
        let available = root
            .payload_limit
            .checked_sub(input_bytes)
            .filter(|remaining| *remaining != 0)
            .ok_or(Error::Archive("publication_payload_budget"))?;
        if inspected.expanded_bytes > available {
            return Err(Error::Archive("publication_payload_budget"));
        }
        let output = StagingArea::create(root.directory.try_clone()?, "runtime", available)?;
        let mut prepared = inspected.prepare(output, policy.descriptors)?;
        native::sync_directory(&root.directory)?;
        let journal = OriginalJournal::initialize(&root, &mut prepared.staged, trust, platform)?;
        let mut attempt = Self {
            root,
            prepared,
            journal,
            destination: None,
            phase: PublicationPhase::Prepared,
        };
        attempt.verify()?;
        Ok(attempt)
    }

    /// This is a diagnostic state, not an execution/install/qualification grant.
    pub fn phase(&self) -> PublicationPhase {
        self.phase
    }

    /// Dispatch at most once. A failure after dispatch keeps this same object
    /// in DispatchedUnconfirmed, even for EEXIST. Call observe_publication only;
    /// a new path/receipt may never recreate this native operation authority.
    pub fn publish(&mut self, versions: Dir) -> Result<()> {
        self.begin_publication(versions)?;
        self.observe_publication()
    }

    /// First physical step only. Even a successful rename leaves the phase
    /// unconfirmed until observe_publication flushes both parents and checks
    /// the original root. This split also permits loss of the caller's reply
    /// without minting any second operation or reconstructing native custody.
    pub fn begin_publication(&mut self, versions: Dir) -> Result<()> {
        self.arm_publication(versions)?;
        self.dispatch_publication()
    }

    /// Durably bind intent before any rename. Only this live original journal
    /// owner can subsequently dispatch this exact destination.
    pub fn arm_publication(&mut self, versions: Dir) -> Result<()> {
        if self.phase != PublicationPhase::Prepared {
            return Err(Error::Archive("publication_already_dispatched"));
        }
        self.verify()?;
        if self.destination.is_some() {
            return Err(Error::Archive("publication_target_already_bound"));
        }
        native::private_directory(&versions)?;
        let parent_id = native::directory_identity(&versions)?;
        let name = format!("version-{}", self.prepared.staged.manifest_sha256());
        if parent_id == self.root.parent_id && name == self.root.name {
            return Err(Error::Archive("publication_source_equals_target"));
        }
        self.destination = Some(Destination {
            parent: versions,
            parent_id,
            name,
        });
        let target = self.destination.as_ref().ok_or(Error::Staging)?;
        self.journal
            .intent(&self.root, &self.prepared.staged, target)?;
        self.phase = PublicationPhase::IntentConfirmed;
        Ok(())
    }

    /// The journal is mandatory original custody, never a bool or loaded ID.
    pub fn dispatch_publication(&mut self) -> Result<()> {
        if self.phase != PublicationPhase::IntentConfirmed {
            return Err(Error::Archive("publication_intent_not_dispatchable"));
        }
        self.verify()?;
        let target = self.destination.as_ref().ok_or(Error::Staging)?;
        self.journal
            .authorize(&self.root, &self.prepared.staged, target)?;
        let old = std::ffi::CString::new(self.root.name.as_bytes()).map_err(|_| Error::Staging)?;
        let new = std::ffi::CString::new(target.name.as_bytes()).map_err(|_| Error::Staging)?;
        // Save all original capabilities before the first possible side effect.
        self.phase = PublicationPhase::DispatchedUnconfirmed;
        let target = self.destination.as_ref().ok_or(Error::Staging)?;
        // SAFETY: retained original directory FDs and single validated leaves.
        // No ambient path, replacement, copy fallback or foreign cleanup exists.
        if unsafe {
            libc::renameat2(
                self.root.parent.as_raw_fd(),
                old.as_ptr(),
                target.parent.as_raw_fd(),
                new.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    /// Re-observation uses only this original root and destination. It can
    /// flush/read back the same publication, but never dispatch rename again.
    /// No restart recovery or adoption from a serialized record is provided.
    pub fn observe_publication(&mut self) -> Result<()> {
        if matches!(
            self.phase,
            PublicationPhase::Prepared | PublicationPhase::IntentConfirmed
        ) {
            return Err(Error::Archive("publication_not_dispatched"));
        }
        self.verify_contents()?;
        self.verify_published_binding()?;
        let target = self.destination.as_ref().ok_or(Error::Staging)?;
        native::sync_directory(&self.root.parent)?;
        native::sync_directory(&target.parent)?;
        self.verify_contents()?;
        self.verify_published_binding()?;
        let target = self.destination.as_ref().ok_or(Error::Staging)?;
        self.journal
            .confirm(&self.root, &self.prepared.staged, target)?;
        self.phase = PublicationPhase::Published;
        Ok(())
    }

    pub fn verify(&mut self) -> Result<()> {
        self.verify_contents()?;
        match self.phase {
            PublicationPhase::Prepared => {
                self.root.check()?;
                self.journal.prepared(&self.root, &self.prepared.staged)
            }
            PublicationPhase::IntentConfirmed => {
                self.root.check()?;
                self.journal.authorize(
                    &self.root,
                    &self.prepared.staged,
                    self.destination.as_ref().ok_or(Error::Staging)?,
                )
            }
            PublicationPhase::DispatchedUnconfirmed => {
                Err(Error::Archive("publication_unconfirmed"))
            }
            PublicationPhase::Published => {
                self.verify_published_binding()?;
                self.journal.completed(
                    &self.root,
                    &self.prepared.staged,
                    self.destination.as_ref().ok_or(Error::Staging)?,
                )
            }
        }
    }

    fn verify_contents(&mut self) -> Result<()> {
        native::private_directory(&self.root.parent)?;
        native::private_directory(&self.root.directory)?;
        if native::directory_identity(&self.root.parent)? != self.root.parent_id
            || native::directory_identity(&self.root.directory)? != self.root.directory_id
        {
            return Err(Error::Staging);
        }
        exact_entries(&self.root.directory, &["package", "runtime"])?;
        self.prepared.verify()
    }

    fn verify_published_binding(&self) -> Result<()> {
        let target = self.destination.as_ref().ok_or(Error::Staging)?;
        native::private_directory(&target.parent)?;
        if native::directory_identity(&target.parent)? != target.parent_id
            || native::directory_identity(&target.parent.open_dir_nofollow(&target.name)?)?
                != self.root.directory_id
        {
            return Err(Error::Archive("publication_unconfirmed"));
        }
        match self.root.parent.symlink_metadata(&self.root.name) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(Error::Archive("publication_original_name_not_absent")),
        }
    }

    pub fn receipt(&mut self) -> Result<PublicationReceipt<'_>> {
        self.verify()?;
        Ok(PublicationReceipt {
            installation: self.prepared.receipt()?,
            publication_phase: self.phase,
            version_directory: self.destination.as_ref().map(|target| target.name.as_str()),
            journal_directory: self.journal.name(),
            journal_confirmed: true,
            published: self.phase == PublicationPhase::Published,
            directory_sync_confirmed: self.phase == PublicationPhase::Published,
            installed: false,
            activated: false,
            anti_rollback_applied: false,
            qualified: false,
            execution_authority: false,
            native_custody_reconstructible: false,
        })
    }
}
