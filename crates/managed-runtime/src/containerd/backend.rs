//! Native service core over the single common Broker. Trusted bootstrap supplies
//! the pinned artifacts and owned storage/generation. Remote requests supply no
//! paths, OCI specs, native identities or reconstructed execution capabilities.
use super::{
    cgroups::{CgroupParent, TaskCgroup},
    input::VerifiedTaskInput,
    profile::ManagedProfile,
    receive::IncomingManagedInput,
    result_snapshot::ManagedOutputSnapshot,
    tasks::{ManagedTask, ManagedTaskOutcome, TaskResources},
    *,
};
use crate::volumes::{MountedVolume, VolumePool, VolumeRequest};
use cowork_contracts::{
    ExecutionEnvironmentRef, ExecutionTargetRef, ReadWorkspaceTransferChunk,
    WorkspaceTransferManifest,
};
use cowork_runner_protocol::{
    Admission, Broker, ExecutionState, LaunchRequest, ManagedInputGrant, Receipt, Signed,
};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub struct ManagedReceiver(Arc<Inner>);
struct Inner {
    profile: Arc<ManagedProfile>,
    broker: Broker,
    volumes: VolumePool,
    claim: crate::volumes::ReceiverClaim,
    groups: CgroupParent,
    daemon: DaemonExecutable,
    slot: Arc<Semaphore>,
    unresolved: AtomicBool,
}
pub enum BeginInput {
    Existing(Box<Signed<Receipt>>),
    Transfer(Box<ManagedTransfer>),
}
pub struct ManagedTransfer {
    owner: ManagedReceiver,
    launch: Signed<LaunchRequest>,
    incoming: IncomingManagedInput,
    slot: OwnedSemaphorePermit,
}
pub struct ManagedExecution {
    cancel: Arc<AtomicBool>,
    worker: tokio::task::JoinHandle<Result<ManagedCompletion>>,
}
pub struct ManagedCompletion {
    pub receipt: Signed<Receipt>,
    pub physical: Arc<ManagedTaskOutcome>,
    pub output: ManagedOutputStatus,
    // Original live-process anchor before durable completion/export. Never
    // serialize or reconstruct it from a historical wall-clock record.
    completion_anchor: std::time::Instant,
    completion_boot_anchor: Option<u64>,
}
/// Original in-memory suspend-aware expiry. No deserialization/restart adoption.
pub struct ManagedOutputRetention {
    deadline_ns: u64,
}
impl ManagedOutputRetention {
    pub fn current(&self) -> bool {
        super::deadline_process::boottime().is_ok_and(|now| self.current_at(now))
    }
    fn current_at(&self, now: u64) -> bool {
        now < self.deadline_ns
    }
}
impl ManagedCompletion {
    /// The service calls this only after its original delivery ACK is durable
    /// and it has excluded new/in-flight result readers. Dropping the snapshot
    /// here closes its independent mount handle before physical reclamation.
    pub fn into_retirement(self) -> super::retirement::ManagedRetirement {
        let Self {
            receipt,
            physical,
            output,
            ..
        } = self;
        drop(output);
        super::retirement::ManagedRetirement::new(receipt, physical)
    }
    pub fn output_retention_deadline(
        &self,
        duration: std::time::Duration,
    ) -> Option<std::time::Instant> {
        self.completion_anchor.checked_add(duration)
    }
    pub fn output_retention_fence(
        &self,
        duration: std::time::Duration,
    ) -> Option<ManagedOutputRetention> {
        Some(ManagedOutputRetention {
            deadline_ns: self
                .completion_boot_anchor?
                .checked_add(duration.as_nanos().try_into().ok()?)?,
        })
    }
}
pub enum ManagedOutputStatus {
    Ready(Box<ManagedOutputSnapshot>),
    Rejected(ManagedOutputRejection),
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct ManagedOutputRejection {
    pub reason: String,
    /// None explicitly means the rejection could not be durably recorded.
    /// The original completed physical receipt remains independently durable.
    pub record_sha256: Option<String>,
}
impl ManagedReceiver {
    /// Installation/qualification is a bootstrap concern. This constructor
    /// verifies the exact retained native objects, never creates a qualification.
    pub fn new(
        profile: Arc<ManagedProfile>,
        broker: Broker,
        volumes: VolumePool,
        groups: CgroupParent,
        daemon: DaemonExecutable,
    ) -> Result<Self> {
        let claim = volumes
            .claim_empty_receiver()
            .map_err(|_| Error::ManagedUncertain)?;
        let receiver = Self(Arc::new(Inner {
            profile,
            broker,
            volumes,
            claim,
            groups,
            daemon,
            slot: Arc::new(Semaphore::new(1)),
            unresolved: AtomicBool::new(false),
        }));
        receiver.current()?;
        Ok(receiver)
    }
    fn current(&self) -> Result<()> {
        if self.0.unresolved.load(Ordering::Acquire) {
            return Err(Error::ManagedUncertain);
        }
        if !self.0.broker.is_bound_to(&self.0.profile.binding) {
            return Err(Error::TaskAdmission);
        }
        self.0.profile.verify()?;
        self.0.daemon.verify()?;
        self.0.groups.require()?;
        self.0.claim.verify().map_err(|_| Error::ManagedUncertain)?;
        self.0.volumes.usage().map_err(|error| Error::InputStage {
            stage: "receiver_pool_usage",
            detail: format!("{error:?}").chars().take(1024).collect(),
        })?;
        Ok(())
    }
    /// Read-only readiness for the exact profile/recipient. It reserves neither
    /// input storage nor compute and does not substitute for start admission.
    pub fn ready(
        &self,
        environment: &ExecutionEnvironmentRef,
        target: &ExecutionTargetRef,
    ) -> Result<()> {
        self.current()?;
        if self.0.profile.reference != *environment || self.0.profile.target != *target {
            return Err(Error::TaskAdmission);
        }
        Ok(())
    }
    pub async fn receipt(&self, attempt: &str) -> Result<Signed<Receipt>> {
        self.0
            .broker
            .receipt(attempt)
            .await
            .map_err(|_| Error::ManagedUncertain)
    }
    /// Exactly one bounded transfer/execution can occupy this receiver. A
    /// retained uncertain physical effect keeps its compute slot permanently.
    pub async fn begin(
        &self,
        launch: Signed<LaunchRequest>,
        grant: Signed<ManagedInputGrant>,
        manifest: WorkspaceTransferManifest,
    ) -> Result<BeginInput> {
        launch
            .verify("launch", &self.0.profile.binding.control_public_key)
            .map_err(|_| Error::TaskAdmission)?;
        match self
            .0
            .broker
            .receipt(&launch.payload.process_attempt_id)
            .await
        {
            Ok(receipt) => {
                if receipt.payload.request != launch.payload
                    || receipt.payload.request_envelope_sha256
                        != cowork_runner_protocol::envelope_hash(&launch)
                {
                    return Err(Error::TaskAdmission);
                }
                return Ok(BeginInput::Existing(Box::new(receipt)));
            }
            Err(cowork_runner_protocol::Error::NotFound) => {}
            Err(_) => return Err(Error::ManagedUncertain),
        }
        self.current()?;
        grant
            .verify(
                "managed_input_grant",
                &self.0.profile.binding.control_public_key,
            )
            .map_err(|_| Error::Input)?;
        grant
            .payload
            .validate_for_launch(
                &self.0.profile.binding,
                &self.0.profile.target,
                &launch.payload,
                super::receive::wall()?,
            )
            .map_err(|_| Error::Input)?;
        cowork_domain::workspace_transfer::validate(&manifest).map_err(|_| Error::Input)?;
        if grant.payload.input_manifest_sha256 != manifest.sha256 {
            return Err(Error::Input);
        }
        let slot = self
            .0
            .slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::ManagedBusy)?;
        let permit = self
            .0
            .broker
            .authorize_input_transfer(&launch)
            .await
            .map_err(|_| Error::Input)?;
        let volume = self
            .0
            .volumes
            .reserve(VolumeRequest {
                request_sha256: launch.payload.request_sha256.clone(),
                bytes: self.0.profile.definition.limits.scratch_bytes,
                inodes: self.0.profile.maximum_files,
            })
            .map_err(|_| Error::Input)?;
        // A lost transfer handle can resume in this same original Broker. A
        // restart cannot take this path because durable custody denies it
        // before storage is opened. A malformed/partial record never falls
        // back to creating a new transfer over the old reservation.
        let retained = match volume.directory().symlink_metadata("input-custody.json") {
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        let incoming = if retained {
            IncomingManagedInput::reopen(
                self.0.profile.clone(),
                &self.0.broker,
                permit,
                grant,
                manifest,
                volume,
            )
            .await?
        } else {
            IncomingManagedInput::create(
                self.0.profile.clone(),
                &self.0.broker,
                permit,
                grant,
                manifest,
                volume,
            )
            .await?
        };
        Ok(BeginInput::Transfer(Box::new(ManagedTransfer {
            owner: self.clone(),
            launch,
            incoming,
            slot,
        })))
    }
}
impl ManagedTransfer {
    pub async fn missing_chunks(&self) -> Result<Vec<ReadWorkspaceTransferChunk>> {
        self.owner.current().map_err(|error| Error::InputStage {
            stage: "missing_receiver_current",
            detail: format!("{error:?}").chars().take(1024).collect(),
        })?;
        let broker = &self.owner.0.broker;
        self.incoming
            .missing_chunks(
                broker,
                broker
                    .authorize_input_transfer(&self.launch)
                    .await
                    .map_err(|error| Error::InputStage {
                        stage: "missing_authorize_input",
                        detail: format!("{error:?}").chars().take(1024).collect(),
                    })?,
            )
            .await
    }
    pub async fn accept_chunk(
        &self,
        input: &ReadWorkspaceTransferChunk,
        bytes: &[u8],
    ) -> Result<cowork_broker::workspace_receive::ChunkReceipt> {
        self.owner.current()?;
        let broker = &self.owner.0.broker;
        self.incoming
            .accept_chunk(
                broker,
                broker
                    .authorize_input_transfer(&self.launch)
                    .await
                    .map_err(|_| Error::Input)?,
                input,
                bytes,
            )
            .await
    }
    /// Verification finishes before admission. After this call the worker owns
    /// the action: dropping its handle or a disconnected HTTP future cannot
    /// cancel the future between durable dispatch and its actual spawn boundary.
    pub async fn seal_and_start(self) -> Result<ManagedExecution> {
        self.owner.current().map_err(|error| Error::InputStage {
            stage: "seal_receiver_current",
            detail: format!("{error:?}").chars().take(1024).collect(),
        })?;
        let broker = &self.owner.0.broker;
        let input = self
            .incoming
            .seal(
                broker,
                broker
                    .authorize_input_transfer(&self.launch)
                    .await
                    .map_err(|error| Error::InputStage {
                        stage: "seal_authorize_input",
                        detail: format!("{error:?}").chars().take(1024).collect(),
                    })?,
            )
            .await?;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker = tokio::spawn(async move {
            let mut slot = EffectSlot {
                owner: self.owner.clone(),
                permit: Some(self.slot),
                effect_possible: false,
            };
            self.owner
                .execute(self.launch, input, worker_cancel, &mut slot)
                .await
        });
        Ok(ManagedExecution { cancel, worker })
    }
}
impl ManagedExecution {
    /// A stop request is not a physical completion receipt.
    pub fn request_stop(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub async fn wait(self) -> Result<ManagedCompletion> {
        self.worker.await.map_err(|_| Error::ManagedUncertain)?
    }
}
impl ManagedReceiver {
    async fn execute(
        &self,
        launch: Signed<LaunchRequest>,
        input: VerifiedTaskInput,
        cancel: Arc<AtomicBool>,
        slot: &mut EffectSlot,
    ) -> Result<ManagedCompletion> {
        self.current()?;
        if cancel.load(Ordering::Acquire) {
            return Err(Error::TaskAdmission);
        }
        // Both native reservations precede common admission/dispatch. The guard
        // may retire only these never-published-to-task resources on early exit.
        let mut resources = UnlaunchedResources {
            owner: self.clone(),
            group: None,
            control: None,
        };
        resources.group = Some(self.0.groups.reserve(self.0.profile.limits())?);
        resources.control = Some(
            self.0
                .volumes
                .reserve(TaskResources::control_volume_request(
                    &launch.payload.request_sha256,
                )?)
                .map_err(|_| Error::TaskAdmission)?,
        );
        let Admission::Launch(permit) = self
            .0
            .broker
            .admit(launch.clone())
            .await
            .map_err(|_| Error::TaskAdmission)?
        else {
            return Err(Error::ManagedUncertain);
        };
        // A dropped/panicking future from this point must retain the slot even
        // if the dispatch reply itself is lost. It must never retry dispatch.
        slot.effect_possible = true;
        let dispatched = self
            .0
            .broker
            .dispatch_owned(permit)
            .await
            .map_err(|_| Error::ManagedUncertain)?;
        let native = TaskResources {
            group: resources.group.take().ok_or(Error::Cgroup)?,
            control: resources.control.take().ok_or(Error::TaskAdmission)?,
        };
        let task = ManagedTask::prepare(
            self.0.profile.clone(),
            dispatched,
            input,
            native,
            self.0.broker.clone(),
            cancel,
            &self.0.profile.target,
        )?;
        let executable = DaemonExecutable {
            file: self.0.daemon.file.try_clone()?,
            sha256: self.0.daemon.sha256.clone(),
        };
        let daemon = task.start_daemon(executable).await?;
        let physical = Arc::new(task.run(daemon).await?);
        // run returns only after exact task/root/control/EOF/native retirement
        // receipts. Disk remains reserved independently of this compute slot.
        slot.effect_possible = false;
        let completion_anchor = std::time::Instant::now();
        let completion_boot_anchor = super::deadline_process::boottime().ok();
        let receipt = self
            .0
            .broker
            .complete(
                &launch.payload.process_attempt_id,
                &launch.payload.request_sha256,
                physical.result.clone(),
            )
            .await
            .map_err(|_| Error::ManagedUncertain)?;
        if !matches!(receipt.payload.state, ExecutionState::Completed { .. }) {
            return Err(Error::ManagedUncertain);
        }
        // Physical completion is independent of portable-format support. Never
        // strand a proven stopped task in DispatchCommitted or rerun its guest
        // merely because output export is unsupported or its storage failed.
        let output = match ManagedOutputSnapshot::capture(
            physical.clone(),
            self.0.profile.maximum_files as usize,
            &receipt,
        ) {
            Ok(snapshot) => ManagedOutputStatus::Ready(Box::new(snapshot)),
            Err(error) => {
                let reason = match error {
                    Error::ResultExport { reason } => reason,
                    Error::Io(_) => "result_export_io_failed",
                    _ => "result_snapshot_unavailable_or_changed",
                };
                let record_sha256 = physical.persist_result_rejection(reason, &receipt).ok();
                ManagedOutputStatus::Rejected(ManagedOutputRejection {
                    reason: reason.into(),
                    record_sha256,
                })
            }
        };
        Ok(ManagedCompletion {
            receipt,
            physical,
            output,
            completion_anchor,
            completion_boot_anchor,
        })
    }
}
struct EffectSlot {
    owner: ManagedReceiver,
    permit: Option<OwnedSemaphorePermit>,
    effect_possible: bool,
}
impl Drop for EffectSlot {
    fn drop(&mut self) {
        if self.effect_possible {
            self.owner.0.unresolved.store(true, Ordering::Release);
            if let Some(permit) = self.permit.take() {
                permit.forget();
            }
        }
    }
}
struct UnlaunchedResources {
    owner: ManagedReceiver,
    group: Option<TaskCgroup>,
    control: Option<MountedVolume>,
}
impl Drop for UnlaunchedResources {
    fn drop(&mut self) {
        let group_failed = self
            .group
            .take()
            .is_some_and(|group| group.retire_unlaunched().is_err());
        let control_failed = self
            .control
            .take()
            .is_some_and(|volume| volume.retire_unlaunched().is_err());
        if group_failed || control_failed {
            self.owner.0.unresolved.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    #[test]
    fn original_boot_expiry_counts_suspend_and_cannot_be_reanchored_by_reads() {
        let retention = ManagedOutputRetention {
            deadline_ns: 8_000_000_000,
        };
        assert!(retention.current_at(7_999_999_999));
        // BOOTTIME includes time asleep even when Instant/CLOCK_MONOTONIC did
        // not advance. UTC and another read's expiry do not enter this check.
        assert!(!retention.current_at(8_000_000_000));
        assert!(!retention.current_at(20_000_000_000));
    }
}
