//! Bounded original native delivery history outside every disposable job volume.
//! Fixed files are fully allocated before admission; no ACK grows a SQLite WAL.
//! Serialized records never reconstruct execution or physical cleanup permits.
use anyhow::{Context, Result, bail, ensure};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use cowork_contracts::{ExecutionRunnerIdentity, ExecutionTargetRef};
use cowork_domain::{canonical_hash, workspace_transfer::valid_sha256};
use cowork_runner_protocol::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, MetadataExt, PermissionsExt},
    },
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const DIRECTORY: &str = "managed-delivery-journal";
const HEADER: &str = "header.bin";
const LOCK: &str = "owner.lock";
const HEADER_BYTES: usize = 512 * 1024;
const ACCEPT_BYTES: usize = 16 * 1024;
const STATUS_BYTES: usize = 8 * 1024;
const SLOT_BYTES: usize = ACCEPT_BYTES + 2 * STATUS_BYTES;
const MAX_SLOTS: usize = 4096;
const FRAME_HEADER: usize = 36;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LocalId {
    dev: u64,
    ino: u64,
}
impl LocalId {
    fn of(file: &File) -> Result<Self> {
        let m = file.metadata()?;
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
        })
    }
    fn directory(dir: &Dir) -> Result<Self> {
        Self::of(&dir.open(".")?.into_std())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    format_version: u32,
    directory: LocalId,
    header: LocalId,
    lock: LocalId,
    binding_sha256: String,
    slots: Vec<LocalId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Source {
    runtime_instance_sha256: String,
    execution_plan_id: String,
    execution_plan_sha256: String,
    process_attempt_id: String,
    run_id: String,
    launch_envelope_sha256: String,
    grant_envelope_sha256: String,
    input_manifest_sha256: String,
    runner: ExecutionRunnerIdentity,
    target: ExecutionTargetRef,
}
impl Source {
    fn from_begin(
        begin: &ManagedBeginRequest,
        binding: &Binding,
        instance: &str,
        now: i64,
    ) -> Result<Self> {
        begin.launch.verify("launch", &binding.control_public_key)?;
        begin
            .grant
            .verify("managed_input_grant", &binding.control_public_key)?;
        begin
            .readiness
            .verify("managed_readiness", &binding.runner_public_key)?;
        let grant = &begin.grant.payload;
        grant.validate_for_launch(binding, &grant.target, &begin.launch.payload, now)?;
        cowork_domain::workspace_transfer::validate(&begin.manifest)?;
        ensure!(
            begin.format_version == 1
                && begin.manifest.sha256 == grant.input_manifest_sha256
                && begin.readiness.payload.runtime_instance_sha256 == instance
                && begin.readiness.payload.runner == grant.runner
                && begin.readiness.payload.target == grant.target,
            "Invalid original delivery source"
        );
        let value = Self {
            runtime_instance_sha256: instance.into(),
            execution_plan_id: grant.execution_plan_id.clone(),
            execution_plan_sha256: grant.execution_plan_sha256.clone(),
            process_attempt_id: grant.process_attempt_id.clone(),
            run_id: grant.run_id.clone(),
            launch_envelope_sha256: envelope_hash(&begin.launch),
            grant_envelope_sha256: canonical_hash(&begin.grant),
            input_manifest_sha256: grant.input_manifest_sha256.clone(),
            runner: grant.runner.clone(),
            target: grant.target.clone(),
        };
        value.validate(binding)?;
        Ok(value)
    }
    fn validate(&self, binding: &Binding) -> Result<()> {
        let expected = ExecutionRunnerIdentity {
            runner_id: binding.runner_id.clone(),
            generation: binding.generation,
            adapter_build_sha256: binding.adapter_build_sha256.clone(),
            control_public_key: binding.control_public_key.clone(),
            runner_public_key: binding.runner_public_key.clone(),
        };
        ensure!(
            self.runner == expected
                && self.target.revision > 0
                && self.target.target_id
                    == cowork_domain::execution_targets::execution_target_id(&expected)?
                && self.target.identity_sha256
                    == cowork_domain::execution_targets::runner_identity_hash(&expected)?
                && [
                    &self.execution_plan_id,
                    &self.process_attempt_id,
                    &self.run_id
                ]
                .iter()
                .all(|s| token(s))
                && [
                    &self.runtime_instance_sha256,
                    &self.execution_plan_sha256,
                    &self.launch_envelope_sha256,
                    &self.grant_envelope_sha256,
                    &self.input_manifest_sha256
                ]
                .iter()
                .all(|s| valid_sha256(s)),
            "Original delivery source changed"
        );
        Ok(())
    }
    fn matches(&self, request: &ManagedResultDeliveryRequest) -> bool {
        self.runtime_instance_sha256 == request.runtime_instance_sha256
            && self.execution_plan_id == request.execution_plan_id
            && self.execution_plan_sha256 == request.execution_plan_sha256
            && self.process_attempt_id == request.process_attempt_id
            && self.run_id == request.run_id
            && self.launch_envelope_sha256 == request.launch_envelope_sha256
            && self.runner == request.runner
            && self.target == request.target
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Record {
    Reserved {
        source: Source,
    },
    Accepted {
        source: Source,
        receipt: Box<Signed<ManagedResultDeliveryReceipt>>,
        initial_status: Box<Signed<ManagedResultRetirementReceipt>>,
    },
}
impl Record {
    fn source(&self) -> &Source {
        match self {
            Self::Reserved { source } | Self::Accepted { source, .. } => source,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusRecord {
    status: Signed<ManagedResultRetirementReceipt>,
    /// Native identities, when present, stay only inside this root-private file.
    local_evidence: Option<serde_json::Value>,
}

#[derive(Default)]
struct Index {
    attempts: BTreeMap<String, usize>,
    requests: BTreeMap<String, usize>,
    used: BTreeSet<usize>,
}

/// In-memory custody of our own attempted write, not a record that can be
/// deserialized, adopted after restart or used to repair arbitrary frames.
struct StatusWrite {
    slot: File,
    slot_id: LocalId,
    index: usize,
    offset: u64,
    receipt_sha256: String,
    payload: Vec<u8>,
}
#[derive(Default)]
struct StatusWrites {
    pending: BTreeMap<String, StatusWrite>,
    reconciled: BTreeMap<String, String>,
}
struct AcceptWrite {
    slot: File,
    slot_id: LocalId,
    index: usize,
    payload: Vec<u8>,
    receipt: Signed<ManagedResultDeliveryReceipt>,
    pending: Signed<ManagedResultRetirementReceipt>,
}

pub(super) struct Journal {
    parent: Dir,
    directory: Dir,
    header: File,
    owner: File,
    metadata: Header,
    binding: Binding,
    index: Mutex<Index>,
    failed: AtomicBool,
    status_writes: Mutex<StatusWrites>,
    accept_write: Mutex<Option<AcceptWrite>>,
    #[cfg(test)]
    lose_status_write_reply: AtomicBool,
    #[cfg(test)]
    lose_accept_write_reply: AtomicBool,
}
pub(super) struct Acceptance {
    pub receipt: Signed<ManagedResultDeliveryReceipt>,
    pub newly_accepted: bool,
}

impl Journal {
    /// Explicit first initialization only; opening a missing/replaced journal
    /// never calls this as a fallback. The parent is the proven bounded state FS.
    pub(super) fn initialize(parent: &Dir, binding: &Binding, count: usize) -> Result<Self> {
        ensure!(
            unsafe { libc::geteuid() } == 0 && (1..=MAX_SLOTS).contains(&count),
            "Invalid journal initialization"
        );
        parent.create_dir(DIRECTORY)?;
        parent.set_permissions(
            DIRECTORY,
            cap_std::fs::Permissions::from_std(std::fs::Permissions::from_mode(0o700)),
        )?;
        let directory = parent.open_dir_nofollow(DIRECTORY)?;
        require_directory(&directory)?;
        let owner = create_file(&directory, LOCK, 0)?;
        owner
            .try_lock()
            .context("Native journal owner already held")?;
        let header = create_file(&directory, HEADER, HEADER_BYTES)?;
        let mut slots = Vec::with_capacity(count);
        for n in 0..count {
            let file = create_file(&directory, &slot_name(n), SLOT_BYTES)?;
            slots.push(LocalId::of(&file)?);
        }
        let metadata = Header {
            format_version: 1,
            directory: LocalId::directory(&directory)?,
            header: LocalId::of(&header)?,
            lock: LocalId::of(&owner)?,
            binding_sha256: canonical_hash(binding),
            slots,
        };
        write_frame(&header, 0, HEADER_BYTES, &serde_json::to_vec(&metadata)?)?;
        directory.open(".")?.into_std().sync_all()?;
        parent.open(".")?.into_std().sync_all()?;
        let journal = Self {
            parent: parent.try_clone()?,
            directory,
            header,
            owner,
            metadata,
            binding: binding.clone(),
            index: Mutex::new(Index::default()),
            failed: AtomicBool::new(false),
            status_writes: Mutex::new(StatusWrites::default()),
            accept_write: Mutex::new(None),
            #[cfg(test)]
            lose_status_write_reply: AtomicBool::new(false),
            #[cfg(test)]
            lose_accept_write_reply: AtomicBool::new(false),
        };
        journal.verify()?;
        Ok(journal)
    }

    pub(super) fn open(parent: &Dir, binding: &Binding) -> Result<Self> {
        ensure!(
            unsafe { libc::geteuid() } == 0,
            "Native journal requires runtime owner"
        );
        let directory = parent.open_dir_nofollow(DIRECTORY)?;
        require_directory(&directory)?;
        let owner = open_file(&directory, LOCK, 0)?;
        owner
            .try_lock()
            .context("Native journal owner already held")?;
        let header = open_file(&directory, HEADER, HEADER_BYTES)?;
        let metadata: Header = serde_json::from_slice(
            &read_frame(&header, 0, HEADER_BYTES)?.context("Original journal header missing")?,
        )?;
        let journal = Self {
            parent: parent.try_clone()?,
            directory,
            header,
            owner,
            metadata,
            binding: binding.clone(),
            index: Mutex::new(Index::default()),
            failed: AtomicBool::new(false),
            status_writes: Mutex::new(StatusWrites::default()),
            accept_write: Mutex::new(None),
            #[cfg(test)]
            lose_status_write_reply: AtomicBool::new(false),
            #[cfg(test)]
            lose_accept_write_reply: AtomicBool::new(false),
        };
        journal.verify()?;
        let mut index = Index::default();
        for n in 0..journal.metadata.slots.len() {
            let record = match journal.record(n) {
                Ok(Some(record)) => record,
                Ok(None) => continue,
                Err(_) => {
                    journal.failed.store(true, Ordering::Release);
                    index.used.insert(n);
                    continue;
                }
            };
            if journal.validate_record(&record).is_err() {
                journal.failed.store(true, Ordering::Release);
                index.used.insert(n);
                continue;
            }
            ensure!(
                index
                    .attempts
                    .insert(record.source().process_attempt_id.clone(), n)
                    .is_none(),
                "Conflicting original journal attempts"
            );
            if let Record::Accepted { receipt, .. } = record {
                // Opening original history makes no first acceptance and
                // restores no native capability. Require positive durability
                // before exposing a valid historical frame after restart.
                let slot = journal.slot(n)?;
                let before = read_frame(&slot, 0, ACCEPT_BYTES)?
                    .context("Original accepted frame missing")?;
                slot.sync_all()?;
                journal.verify()?;
                ensure!(
                    read_frame(&journal.slot(n)?, 0, ACCEPT_BYTES)?.as_deref()
                        == Some(before.as_slice()),
                    "Original accepted history changed during durability observation"
                );
                ensure!(
                    index
                        .requests
                        .insert(receipt.payload.request.payload.request_id, n)
                        .is_none(),
                    "Conflicting original journal requests"
                );
            }
            index.used.insert(n);
            if journal.status_at(n).is_err() {
                journal.failed.store(true, Ordering::Release);
            }
        }
        *journal
            .index
            .lock()
            .map_err(|_| anyhow::anyhow!("Native journal lock poisoned"))? = index;
        Ok(journal)
    }

    pub(super) fn can_reserve(&self) -> Result<bool> {
        self.verify()?;
        self.require_writable()?;
        let index = self
            .index
            .lock()
            .map_err(|_| anyhow::anyhow!("Native journal lock poisoned"))?;
        Ok(index.used.len() < self.metadata.slots.len())
    }

    /// Startup only. A fresh process cannot reconstruct the native custody for
    /// any reserved, Pending or Unconfirmed original. Serve history in that case.
    pub(super) fn can_start_new_receiver(&self) -> Result<bool> {
        self.verify()?;
        self.require_writable()?;
        let index = self
            .index
            .lock()
            .map_err(|_| anyhow::anyhow!("Native journal lock poisoned"))?;
        for n in &index.used {
            let original = self
                .record(*n)?
                .context("Original delivery source missing")?;
            if matches!(original, Record::Reserved { .. }) {
                return Ok(false);
            }
            let status = self
                .status_at(*n)?
                .context("Original retirement status missing")?;
            if status.payload.state != ManagedResultRetirementState::Retired {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn reserve(
        &self,
        begin: &ManagedBeginRequest,
        instance: &str,
        now: i64,
    ) -> Result<()> {
        let source = Source::from_begin(begin, &self.binding, instance, now)?;
        self.verify()?;
        self.require_writable()?;
        let mut index = self
            .index
            .lock()
            .map_err(|_| anyhow::anyhow!("Native journal lock poisoned"))?;
        if let Some(n) = index.attempts.get(&source.process_attempt_id) {
            let original = self
                .record(*n)?
                .context("Original reserved journal slot missing")?;
            ensure!(
                original.source() == &source,
                "Original journal reservation conflicts"
            );
            return Ok(());
        }
        let n = (0..self.metadata.slots.len())
            .find(|n| !index.used.contains(n))
            .context("Native delivery journal capacity exhausted")?;
        ensure!(
            self.record(n)?.is_none(),
            "Native journal free slot changed"
        );
        let file = self.slot(n)?;
        let record = Record::Reserved {
            source: source.clone(),
        };
        self.persist(&file, 0, ACCEPT_BYTES, &serde_json::to_vec(&record)?)?;
        index.used.insert(n);
        index.attempts.insert(source.process_attempt_id, n);
        Ok(())
    }

    /// A missing original slot is uncertain, never evidence of non-acceptance.
    pub(super) fn existing(
        &self,
        request: &Signed<ManagedResultDeliveryRequest>,
    ) -> Result<Option<Signed<ManagedResultDeliveryReceipt>>> {
        request.verify(
            "managed_result_delivery_request",
            &self.binding.control_public_key,
        )?;
        self.verify()?;
        let index = self
            .index
            .lock()
            .map_err(|_| anyhow::anyhow!("Native journal lock poisoned"))?;
        let n = *index
            .attempts
            .get(&request.payload.process_attempt_id)
            .context("Original delivery journal source unavailable")?;
        self.require_resolved_accept(n)?;
        if let Some(other) = index.requests.get(&request.payload.request_id) {
            ensure!(
                *other == n,
                "Delivery request ID conflicts with original attempt"
            );
        }
        let record = self
            .record(n)?
            .context("Original delivery journal slot missing")?;
        self.validate_record(&record)?;
        ensure!(
            record.source().matches(&request.payload),
            "Delivery source changed"
        );
        match record {
            Record::Reserved { .. } => {
                ensure!(
                    !index.requests.values().any(|slot| *slot == n),
                    "Previously accepted delivery journal regressed"
                );
                self.require_writable()?;
                Ok(None)
            }
            Record::Accepted { receipt, .. } => {
                ensure!(
                    receipt.payload.request == *request,
                    "Delivery conflicts with original accepted request"
                );
                Ok(Some(*receipt))
            }
        }
    }

    pub(super) fn accept(
        &self,
        receipt: Signed<ManagedResultDeliveryReceipt>,
        pending: Signed<ManagedResultRetirementReceipt>,
    ) -> Result<Acceptance> {
        self.validate_accepted(&receipt, &pending)?;
        self.verify()?;
        let mut index = self
            .index
            .lock()
            .map_err(|_| anyhow::anyhow!("Native journal lock poisoned"))?;
        let wanted = &receipt.payload.request.payload;
        let n = *index
            .attempts
            .get(&wanted.process_attempt_id)
            .context("Original delivery source unavailable")?;
        let mut held = self
            .accept_write
            .lock()
            .map_err(|_| anyhow::anyhow!("Original acceptance write custody poisoned"))?;
        ensure!(
            held.as_ref().is_none_or(|original| original.index != n),
            "Original acceptance write remains unconfirmed"
        );
        if let Some(other) = index.requests.get(&wanted.request_id) {
            ensure!(*other == n, "Delivery request ID conflicts");
        }
        let original = self.record(n)?.context("Original delivery slot missing")?;
        self.validate_record(&original)?;
        ensure!(
            original.source().matches(wanted),
            "Original delivery source changed"
        );
        if let Record::Accepted { receipt: old, .. } = original {
            ensure!(
                old.payload.request == receipt.payload.request,
                "Accepted delivery conflicts"
            );
            return Ok(Acceptance {
                receipt: *old,
                newly_accepted: false,
            });
        }
        self.require_writable()?;
        let next = Record::Accepted {
            source: original.source().clone(),
            receipt: Box::new(receipt.clone()),
            initial_status: Box::new(pending.clone()),
        };
        let payload = serde_json::to_vec(&next)?;
        ensure!(
            payload.len() <= ACCEPT_BYTES - FRAME_HEADER && held.is_none(),
            "Original acceptance write quota or ownership changed"
        );
        let slot = self.slot(n)?;
        *held = Some(AcceptWrite {
            slot_id: LocalId::of(&slot)?,
            slot,
            index: n,
            payload,
            receipt: receipt.clone(),
            pending,
        });
        let original = held.as_ref().unwrap();
        let result = write_frame(&original.slot, 0, ACCEPT_BYTES, &original.payload);
        #[cfg(test)]
        let result = result.and_then(|()| {
            if self.lose_accept_write_reply.swap(false, Ordering::AcqRel) {
                bail!("Controlled lost acceptance reply after actual write/fsync/readback")
            }
            Ok(())
        });
        if let Err(error) = result {
            self.failed.store(true, Ordering::Release);
            return Err(error);
        }
        held.take();
        index.requests.insert(wanted.request_id.clone(), n);
        Ok(Acceptance {
            receipt,
            newly_accepted: true,
        })
    }

    /// Continuation only of this process's original attempted acceptance write.
    /// History/query paths never call it and saved JSON cannot create its FD.
    pub(super) fn reobserve_acceptance(
        &self,
        receipt: &Signed<ManagedResultDeliveryReceipt>,
        pending: &Signed<ManagedResultRetirementReceipt>,
    ) -> Result<()> {
        self.validate_accepted(receipt, pending)?;
        self.verify()?;
        let mut index = self
            .index
            .lock()
            .map_err(|_| anyhow::anyhow!("Native journal lock poisoned"))?;
        let mut held = self
            .accept_write
            .lock()
            .map_err(|_| anyhow::anyhow!("Original acceptance write custody poisoned"))?;
        let original = held
            .as_ref()
            .context("No original attempted acceptance capability")?;
        let request = &receipt.payload.request.payload;
        ensure!(
            original.receipt == *receipt
                && original.pending == *pending
                && index.attempts.get(&request.process_attempt_id) == Some(&original.index)
                && index
                    .requests
                    .get(&request.request_id)
                    .is_none_or(|n| *n == original.index)
                && original.slot_id == LocalId::of(&original.slot)?
                && original.slot_id == LocalId::of(&self.slot(original.index)?)?,
            "Original acceptance continuation changed"
        );
        ensure!(
            read_frame(&original.slot, 0, ACCEPT_BYTES)?.as_deref()
                == Some(original.payload.as_slice()),
            "Original attempted acceptance frame remains uncertain"
        );
        for i in 0..2 {
            ensure!(
                read_frame(
                    &original.slot,
                    (ACCEPT_BYTES + i * STATUS_BYTES) as u64,
                    STATUS_BYTES
                )?
                .is_none(),
                "Unconfirmed original acceptance has foreign status frames"
            );
        }
        original.slot.sync_all()?;
        self.verify()?;
        ensure!(
            original.slot_id == LocalId::of(&self.slot(original.index)?)?
                && read_frame(&original.slot, 0, ACCEPT_BYTES)?.as_deref()
                    == Some(original.payload.as_slice()),
            "Original acceptance durability readback changed"
        );
        let loaded: Record = serde_json::from_slice(&original.payload)?;
        self.validate_record(&loaded)?;
        self.status_writes
            .lock()
            .map_err(|_| anyhow::anyhow!("Original status custody poisoned"))?
            .reconciled
            .insert(request.process_attempt_id.clone(), canonical_hash(receipt));
        index
            .requests
            .insert(request.request_id.clone(), original.index);
        held.take();
        Ok(())
    }

    fn require_resolved_accept(&self, n: usize) -> Result<()> {
        let held = self
            .accept_write
            .lock()
            .map_err(|_| anyhow::anyhow!("Original acceptance write custody poisoned"))?;
        ensure!(
            held.as_ref().is_none_or(|original| original.index != n),
            "Original acceptance durability remains unconfirmed"
        );
        Ok(())
    }

    pub(super) fn status(
        &self,
        query: &Signed<ManagedResultRetirementQuery>,
        now: i64,
    ) -> Result<Signed<ManagedResultRetirementReceipt>> {
        query.verify(
            "managed_result_retirement_query",
            &self.binding.control_public_key,
        )?;
        self.verify()?;
        let index = self
            .index
            .lock()
            .map_err(|_| anyhow::anyhow!("Native journal lock poisoned"))?;
        let n = *index
            .attempts
            .get(&query.payload.process_attempt_id)
            .context("Original delivery source unavailable")?;
        self.require_resolved_accept(n)?;
        let record = self.record(n)?.context("Original delivery slot missing")?;
        let Record::Accepted { receipt, .. } = record else {
            bail!("Delivery has not been accepted")
        };
        query.payload.validate(&receipt, &self.binding, now)?;
        self.status_at(n)?
            .context("Original retirement status unavailable")
    }

    pub(super) fn record_status(
        &self,
        status: &Signed<ManagedResultRetirementReceipt>,
        evidence: Option<serde_json::Value>,
    ) -> Result<()> {
        self.verify()?;
        let index = self
            .index
            .lock()
            .map_err(|_| anyhow::anyhow!("Native journal lock poisoned"))?;
        let n = *index
            .attempts
            .get(&status.payload.process_attempt_id)
            .context("Original retirement source unavailable")?;
        self.require_resolved_accept(n)?;
        let record = self.record(n)?.context("Original delivery slot missing")?;
        let Record::Accepted {
            receipt,
            initial_status,
            ..
        } = record
        else {
            bail!("Delivery has not been accepted")
        };
        let next = StatusRecord {
            status: status.clone(),
            local_evidence: evidence,
        };
        self.validate_status(
            &next,
            &receipt,
            &initial_status.payload.physical_completion_sha256,
        )?;
        let payload = serde_json::to_vec(&next)?;
        let attempt = &status.payload.process_attempt_id;
        let receipt_sha256 = canonical_hash(&receipt);
        let mut writes = self
            .status_writes
            .lock()
            .map_err(|_| anyhow::anyhow!("Original status write custody poisoned"))?;
        if let Some(original) = writes.pending.get(attempt) {
            ensure!(
                original.index == n
                    && original.receipt_sha256 == receipt_sha256
                    && original.payload == payload
                    && original.slot_id == LocalId::of(&self.slot(n)?)?
                    && original.slot_id == LocalId::of(&original.slot)?,
                "Original status write candidate changed"
            );
            // Never overwrite a partial or foreign frame. Only a complete,
            // identical candidate written through this original retained FD
            // can resolve its lost fsync/readback reply. Admission stays closed.
            ensure!(
                read_frame(&original.slot, original.offset, STATUS_BYTES)?.as_deref()
                    == Some(payload.as_slice()),
                "Original attempted status write remains uncertain"
            );
            original.slot.sync_all()?;
            self.verify()?;
            ensure!(
                original.slot_id == LocalId::of(&self.slot(n)?)?
                    && read_frame(&original.slot, original.offset, STATUS_BYTES)?.as_deref()
                        == Some(payload.as_slice()),
                "Original status write readback changed"
            );
            writes.pending.remove(attempt);
            writes
                .reconciled
                .insert(attempt.clone(), receipt_sha256.clone());
        }
        let slot = self.slot(n)?;
        // An invalid sibling frame never becomes a repair target, including
        // when status_at could still return an older intact signed revision.
        for i in 0..2 {
            if let Some(bytes) = read_frame(
                &slot,
                (ACCEPT_BYTES + i * STATUS_BYTES) as u64,
                STATUS_BYTES,
            )? {
                let retained: StatusRecord = serde_json::from_slice(&bytes)?;
                self.validate_status(
                    &retained,
                    &receipt,
                    &initial_status.payload.physical_completion_sha256,
                )?;
            }
        }
        let previous = self
            .status_at(n)?
            .context("Original retirement status missing")?;
        if previous.payload.revision == status.payload.revision {
            ensure!(previous == *status, "Retirement status revision conflicts");
            return Ok(());
        }
        if self.failed.load(Ordering::Acquire) {
            ensure!(
                writes.reconciled.get(attempt) == Some(&receipt_sha256),
                "Native journal is closed to unrelated new writes"
            );
        }
        ensure!(
            previous.payload.state != ManagedResultRetirementState::Retired
                && previous.payload.revision.checked_add(1) == Some(status.payload.revision)
                && status.payload.recorded_at >= previous.payload.recorded_at
                && status.payload.state != ManagedResultRetirementState::Pending,
            "Retirement status cannot regress"
        );
        let offset = ACCEPT_BYTES + (status.payload.revision as usize % 2) * STATUS_BYTES;
        ensure!(
            payload.len() <= STATUS_BYTES - FRAME_HEADER,
            "Original status frame exceeds quota"
        );
        let pending = StatusWrite {
            slot_id: LocalId::of(&slot)?,
            slot,
            index: n,
            offset: offset as u64,
            receipt_sha256,
            payload,
        };
        writes.pending.insert(attempt.clone(), pending);
        let original = writes.pending.get(attempt).unwrap();
        let result = write_frame(
            &original.slot,
            original.offset,
            STATUS_BYTES,
            &original.payload,
        );
        #[cfg(test)]
        let result = result.and_then(|()| {
            if self.lose_status_write_reply.swap(false, Ordering::AcqRel) {
                bail!("Controlled lost reply after actual original write/fsync/readback")
            }
            Ok(())
        });
        if result.is_err() {
            self.failed.store(true, Ordering::Release);
        } else {
            writes.pending.remove(attempt);
        }
        result
    }

    fn validate_accepted(
        &self,
        receipt: &Signed<ManagedResultDeliveryReceipt>,
        pending: &Signed<ManagedResultRetirementReceipt>,
    ) -> Result<()> {
        receipt.verify(
            "managed_result_delivery_receipt",
            &self.binding.runner_public_key,
        )?;
        receipt
            .payload
            .validate(&receipt.payload.request, &self.binding)?;
        pending.verify(
            "managed_result_retirement_receipt",
            &self.binding.runner_public_key,
        )?;
        pending.payload.validate(
            receipt,
            &self.binding,
            &pending.payload.physical_completion_sha256,
        )?;
        ensure!(
            pending.payload.state == ManagedResultRetirementState::Pending
                && pending.payload.revision == 1
                && pending.payload.recorded_at == receipt.payload.accepted_at,
            "Invalid initial retirement status"
        );
        Ok(())
    }
    fn validate_record(&self, record: &Record) -> Result<()> {
        record.source().validate(&self.binding)?;
        if let Record::Accepted {
            source,
            receipt,
            initial_status,
        } = record
        {
            self.validate_accepted(receipt, initial_status)?;
            ensure!(
                source.matches(&receipt.payload.request.payload),
                "Accepted delivery source changed"
            );
        }
        Ok(())
    }
    fn validate_status(
        &self,
        value: &StatusRecord,
        receipt: &Signed<ManagedResultDeliveryReceipt>,
        physical: &str,
    ) -> Result<()> {
        value.status.verify(
            "managed_result_retirement_receipt",
            &self.binding.runner_public_key,
        )?;
        value
            .status
            .payload
            .validate(receipt, &self.binding, physical)?;
        ensure!(
            value.local_evidence.as_ref().map(canonical_hash)
                == value.status.payload.retirement_evidence_sha256,
            "Native retirement evidence differs from signed status"
        );
        if let Some(evidence) = &value.local_evidence {
            ensure!(
                serde_json::to_vec(evidence)?.len() <= 4096,
                "Native retirement evidence limit"
            );
        }
        Ok(())
    }
    fn status_at(&self, n: usize) -> Result<Option<Signed<ManagedResultRetirementReceipt>>> {
        let Some(record) = self.record(n)? else {
            return Ok(None);
        };
        self.validate_record(&record)?;
        let Record::Accepted {
            receipt,
            initial_status,
            ..
        } = record
        else {
            return Ok(None);
        };
        let file = self.slot(n)?;
        let mut choices = vec![*initial_status];
        let mut invalid = false;
        for i in 0..2 {
            let loaded = (|| -> Result<Option<Signed<ManagedResultRetirementReceipt>>> {
                let Some(bytes) = read_frame(
                    &file,
                    (ACCEPT_BYTES + i * STATUS_BYTES) as u64,
                    STATUS_BYTES,
                )?
                else {
                    return Ok(None);
                };
                let value: StatusRecord = serde_json::from_slice(&bytes)?;
                self.validate_status(
                    &value,
                    &receipt,
                    &choices[0].payload.physical_completion_sha256,
                )?;
                Ok(Some(value.status))
            })();
            match loaded {
                Ok(Some(value)) => choices.push(value),
                Ok(None) => {}
                Err(_) => invalid = true,
            }
        }
        if invalid {
            self.failed.store(true, Ordering::Release);
        }
        ensure!(
            !invalid || choices.len() > 1,
            "Original retirement status framing uncertain"
        );
        choices.sort_by_key(|value| value.payload.revision);
        let last = choices.pop().unwrap();
        ensure!(
            !choices
                .iter()
                .any(|value| value.payload.revision == last.payload.revision && *value != last),
            "Conflicting original retirement revisions"
        );
        Ok(Some(last))
    }
    fn record(&self, n: usize) -> Result<Option<Record>> {
        let result: Result<Option<Record>> = (|| {
            let file = self.slot(n)?;
            let record: Option<Record> = read_frame(&file, 0, ACCEPT_BYTES)?
                .map(|bytes| serde_json::from_slice(&bytes).map_err(anyhow::Error::from))
                .transpose()?;
            if !matches!(record, Some(Record::Accepted { .. })) {
                for i in 0..2 {
                    ensure!(
                        read_frame(
                            &file,
                            (ACCEPT_BYTES + i * STATUS_BYTES) as u64,
                            STATUS_BYTES
                        )?
                        .is_none(),
                        "Retirement frames without original accepted delivery"
                    );
                }
            }
            Ok(record)
        })();
        if result.is_err() {
            self.failed.store(true, Ordering::Release);
        }
        result
    }
    fn require_writable(&self) -> Result<()> {
        ensure!(
            !self.failed.load(Ordering::Acquire),
            "Native journal retains an unconfirmed write or record"
        );
        Ok(())
    }
    fn persist(&self, file: &File, offset: u64, bytes: usize, payload: &[u8]) -> Result<()> {
        self.require_writable()?;
        let result = write_frame(file, offset, bytes, payload);
        if result.is_err() {
            self.failed.store(true, Ordering::Release);
        }
        result
    }
    fn slot(&self, n: usize) -> Result<File> {
        let expected = self
            .metadata
            .slots
            .get(n)
            .context("Invalid native journal slot")?;
        let file = open_file(&self.directory, &slot_name(n), SLOT_BYTES)?;
        ensure!(
            LocalId::of(&file)? == *expected,
            "Original native journal slot replaced"
        );
        Ok(file)
    }
    fn verify(&self) -> Result<()> {
        require_directory(&self.directory)?;
        let named = self.parent.open_dir_nofollow(DIRECTORY)?;
        require_directory(&named)?;
        ensure!(
            self.metadata.format_version == 1
                && (1..=MAX_SLOTS).contains(&self.metadata.slots.len())
                && self.metadata.binding_sha256 == canonical_hash(&self.binding)
                && self.metadata.directory == LocalId::directory(&self.directory)?
                && self.metadata.directory == LocalId::directory(&named)?
                && self.metadata.header == LocalId::of(&self.header)?
                && self.metadata.lock == LocalId::of(&self.owner)?
                && self.metadata.header
                    == LocalId::of(&open_file(&self.directory, HEADER, HEADER_BYTES)?)?
                && self.metadata.lock == LocalId::of(&open_file(&self.directory, LOCK, 0)?)?,
            "Original native journal identity changed"
        );
        let loaded: Header = serde_json::from_slice(
            &read_frame(&self.header, 0, HEADER_BYTES)?
                .context("Original native journal header missing")?,
        )?;
        ensure!(
            serde_json::to_vec(&loaded)? == serde_json::to_vec(&self.metadata)?,
            "Original native journal header changed"
        );
        let expected = (0..self.metadata.slots.len())
            .map(slot_name)
            .chain([HEADER.into(), LOCK.into()])
            .collect::<BTreeSet<_>>();
        let actual = self
            .directory
            .entries()?
            .take(MAX_SLOTS + 3)
            .map(|entry| {
                Ok(entry?
                    .file_name()
                    .to_str()
                    .context("Invalid native journal entry")?
                    .to_owned())
            })
            .collect::<Result<BTreeSet<_>>>()?;
        ensure!(
            expected == actual,
            "Original native journal entries changed"
        );
        Ok(())
    }
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}
fn slot_name(n: usize) -> String {
    format!("slot-{n:04}.bin")
}
fn require_directory(dir: &Dir) -> Result<()> {
    let m = dir.open(".")?.into_std().metadata()?;
    ensure!(
        m.is_dir() && m.uid() == 0 && m.mode() & 0o777 == 0o700,
        "Invalid private native journal directory"
    );
    Ok(())
}
fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .mode(0o600)
        .follow(FollowSymlinks::No)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    options
}
fn create_file(dir: &Dir, name: &str, bytes: usize) -> Result<File> {
    let mut options = options();
    options.create_new(true);
    let file = dir.open_with(name, &options)?.into_std();
    if bytes != 0 {
        let status = unsafe { libc::posix_fallocate(file.as_raw_fd(), 0, bytes as i64) };
        ensure!(status == 0, "Native journal reservation unavailable");
    }
    file.sync_all()?;
    verify_file(&file, bytes)?;
    Ok(file)
}
fn open_file(dir: &Dir, name: &str, bytes: usize) -> Result<File> {
    let file = dir.open_with(name, &options())?.into_std();
    verify_file(&file, bytes)?;
    Ok(file)
}
fn verify_file(file: &File, bytes: usize) -> Result<()> {
    let m = file.metadata()?;
    ensure!(
        m.is_file()
            && m.uid() == 0
            && m.nlink() == 1
            && m.mode() & 0o777 == 0o600
            && m.len() == bytes as u64
            && m.blocks()
                .checked_mul(512)
                .is_some_and(|n| n >= bytes as u64),
        "Native journal file changed or not reserved"
    );
    Ok(())
}
fn read_frame(file: &File, offset: u64, bytes: usize) -> Result<Option<Vec<u8>>> {
    ensure!(
        bytes > FRAME_HEADER
            && offset
                .checked_add(bytes as u64)
                .is_some_and(|end| end <= file.metadata().map(|m| m.len()).unwrap_or(0)),
        "Native journal frame boundary invalid"
    );
    let mut data = vec![0; bytes];
    file.read_exact_at(&mut data, offset)?;
    if data.iter().all(|b| *b == 0) {
        return Ok(None);
    }
    let len = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
    ensure!(
        len > 0 && len <= bytes - FRAME_HEADER,
        "Native journal frame length uncertain"
    );
    let payload = &data[FRAME_HEADER..FRAME_HEADER + len];
    ensure!(
        ring::digest::digest(&ring::digest::SHA256, payload).as_ref() == &data[4..FRAME_HEADER]
            && data[FRAME_HEADER + len..].iter().all(|b| *b == 0),
        "Native journal frame hash or padding uncertain"
    );
    Ok(Some(payload.to_vec()))
}
fn write_frame(file: &File, offset: u64, bytes: usize, payload: &[u8]) -> Result<()> {
    ensure!(
        bytes > FRAME_HEADER
            && !payload.is_empty()
            && payload.len() <= bytes - FRAME_HEADER
            && offset
                .checked_add(bytes as u64)
                .is_some_and(|end| end <= file.metadata().map(|m| m.len()).unwrap_or(0)),
        "Native journal record limit"
    );
    let mut frame = vec![0; bytes];
    frame[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    frame[4..FRAME_HEADER]
        .copy_from_slice(ring::digest::digest(&ring::digest::SHA256, payload).as_ref());
    frame[FRAME_HEADER..FRAME_HEADER + payload.len()].copy_from_slice(payload);
    file.write_all_at(&frame, offset)?;
    file.sync_all()?;
    ensure!(
        read_frame(file, offset, bytes)?.as_deref() == Some(payload),
        "Native journal durable record differs"
    );
    Ok(())
}

#[cfg(test)]
#[path = "managed_delivery_journal_tests.rs"]
mod tests;
