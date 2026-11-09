//! Physical ownership only, not a second run/admission ledger. Preallocated
//! slots allow acknowledgements at ENOSPC. A saved record never mints dispatch.
use super::*;
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::{
    io::{Seek, SeekFrom, Write},
    os::fd::AsRawFd,
};

const SLOT_BYTES: usize = 16384;
const OUTCOME_BYTES: usize = 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mutation {
    ContainerCreate,
    TaskCreate,
    TaskStart,
    TaskKill,
    TaskDelete,
    ContainerDelete,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MutationReceipt {
    operation: Mutation,
    container_id: String,
    acknowledged: bool,
    response: Option<MutationResponse>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MutationResponse {
    Succeeded,
    RejectedNotFound,
    RejectedFailedPrecondition,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PhysicalTaskRecord {
    pub format_version: u32,
    pub sequence: u64,
    pub task_id: String,
    pub sandbox_id: String,
    pub sandbox_spec_sha256: String,
    pub namespace: String,
    pub request_sha256: String,
    pub plan_sha256: String,
    pub input_manifest_sha256: String,
    pub volume_id: String,
    pub control_volume_id: String,
    pub spec_sha256: String,
    pub definition_sha256: String,
    pub daemon_pid: u32,
    pub daemon_start_ticks: u64,
    pub daemon_executable_sha256: String,
    pub task_pid: Option<u32>,
    pub sandbox_pid: Option<u32>,
    pub reported_exit_status: Option<u32>,
    pub sandbox_reported_exit_status: Option<u32>,
    pub guest_exit_confirmed: bool,
    pub guest_wait_evidence_sha256: Option<String>,
    pub sandbox_live_at_guest_wait: bool,
    pub control_process_reaped: bool,
    pub cgroup_path: String,
    pub mutations: Vec<MutationReceipt>,
    pub physical_stop_confirmed: bool,
    pub output_complete: bool,
    pub outcome_sha256: Option<String>,
}
pub(super) struct TaskJournal {
    directory: Dir,
    files: [File; 2],
    outcome: File,
    record: PhysicalTaskRecord,
}
impl TaskJournal {
    pub fn create(directory: Dir, record: PhysicalTaskRecord) -> Result<Self> {
        if record.format_version != 2 {
            return Err(Error::TaskJournal);
        }
        let metadata = directory.open(".")?.into_std().metadata()?;
        if metadata.uid() != 0 || metadata.mode() & 0o077 != 0 {
            return Err(Error::TaskJournal);
        }
        let create = |name: &str, bytes: usize| -> Result<File> {
            let mut options = cap_std::fs::OpenOptions::new();
            options
                .read(true)
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
            let file = directory.open_with(name, &options)?.into_std();
            let code = unsafe { libc::posix_fallocate(file.as_raw_fd(), 0, bytes as libc::off_t) };
            if code != 0 {
                return Err(io::Error::from_raw_os_error(code).into());
            }
            file.sync_all()?;
            Ok(file)
        };
        let files = [create("task.a", SLOT_BYTES)?, create("task.b", SLOT_BYTES)?];
        let outcome = create("outcome.bin", OUTCOME_BYTES)?;
        let mut journal = Self {
            directory,
            files,
            outcome,
            record,
        };
        journal.persist()?;
        journal.directory.open(".")?.into_std().sync_all()?;
        Ok(journal)
    }
    pub fn begin(&mut self, operation: Mutation, container_id: &str) -> Result<()> {
        let order = [
            (Mutation::ContainerCreate, true),
            (Mutation::TaskCreate, true),
            (Mutation::TaskStart, true),
            (Mutation::ContainerCreate, false),
            (Mutation::TaskCreate, false),
            (Mutation::TaskStart, false),
            (Mutation::TaskKill, false),
            (Mutation::TaskDelete, false),
            (Mutation::ContainerDelete, false),
            (Mutation::TaskKill, true),
            (Mutation::TaskDelete, true),
            (Mutation::ContainerDelete, true),
        ];
        let Some((expected, sandbox)) = order.get(self.record.mutations.len()) else {
            return Err(Error::TaskJournal);
        };
        if operation != *expected
            || container_id
                != if *sandbox {
                    &self.record.sandbox_id
                } else {
                    &self.record.task_id
                }
        {
            return Err(Error::TaskJournal);
        }
        if (container_id != self.record.task_id && container_id != self.record.sandbox_id)
            || self.record.mutations.len() >= 16
            || self
                .record
                .mutations
                .last()
                .is_some_and(|m| !m.acknowledged)
        {
            return Err(Error::TaskJournal);
        }
        self.record.mutations.push(MutationReceipt {
            operation,
            container_id: container_id.into(),
            acknowledged: false,
            response: None,
        });
        self.persist()
    }
    pub fn acknowledge(&mut self) -> Result<()> {
        self.acknowledge_response(MutationResponse::Succeeded)
    }
    pub fn acknowledge_response(&mut self, response: MutationResponse) -> Result<()> {
        let last = self.record.mutations.last_mut().ok_or(Error::TaskJournal)?;
        if last.acknowledged
            || (!matches!(response, MutationResponse::Succeeded)
                && !matches!(last.operation, Mutation::TaskKill))
        {
            return Err(Error::TaskJournal);
        }
        last.acknowledged = true;
        last.response = Some(response);
        self.persist()
    }
    pub fn record_created_pid(&mut self, pid: u32) -> Result<()> {
        if pid == 0 || self.record.task_pid.is_some() {
            return Err(Error::TaskJournal);
        }
        self.record.task_pid = Some(pid);
        self.persist()
    }
    pub fn record_sandbox_pid(&mut self, pid: u32) -> Result<()> {
        if pid == 0 || self.record.sandbox_pid.is_some() {
            return Err(Error::TaskJournal);
        }
        self.record.sandbox_pid = Some(pid);
        self.persist()
    }
    pub fn record_exit(&mut self, status: u32) -> Result<()> {
        if self.record.task_pid.is_none() || self.record.reported_exit_status.is_some() {
            return Err(Error::TaskJournal);
        }
        self.record.reported_exit_status = Some(status);
        self.persist()
    }
    pub fn confirm_guest_exit(
        &mut self,
        evidence: &super::runsc_driver::VerifiedGuestExit,
    ) -> Result<()> {
        if self.record.reported_exit_status != Some(evidence.status())
            || self.record.guest_exit_confirmed
        {
            return Err(Error::TaskJournal);
        }
        self.record.guest_exit_confirmed = true;
        self.record.guest_wait_evidence_sha256 = Some(evidence.sha256().into());
        self.persist()
    }
    pub fn record_live_sandbox_at_guest_wait(&mut self) -> Result<()> {
        if self.record.reported_exit_status.is_none() || self.record.sandbox_pid.is_none() {
            return Err(Error::TaskJournal);
        }
        self.record.sandbox_live_at_guest_wait = true;
        self.persist()
    }
    pub fn record_sandbox_exit(&mut self, status: u32) -> Result<()> {
        if self.record.sandbox_pid.is_none() || self.record.sandbox_reported_exit_status.is_some() {
            return Err(Error::TaskJournal);
        }
        self.record.sandbox_reported_exit_status = Some(status);
        self.persist()
    }
    pub fn record_control_reaped(&mut self, pid: Option<u32>) -> Result<()> {
        if pid != Some(self.record.daemon_pid) {
            return Err(Error::TaskJournal);
        }
        self.record.control_process_reaped = true;
        self.persist()
    }
    pub fn confirm_stopped(&mut self, result: &cowork_contracts::ProcessResult) -> Result<String> {
        if self.record.mutations.len() != 12
            || self.record.mutations.iter().any(|m| !m.acknowledged)
            || self.record.task_pid.is_none()
            || self.record.sandbox_pid.is_none()
            || self.record.reported_exit_status.is_none()
            || self.record.sandbox_reported_exit_status.is_none()
            || !self.record.control_process_reaped
            || (result.termination == cowork_contracts::ProcessTermination::Exited
                && !self.record.guest_exit_confirmed)
        {
            return Err(Error::TaskJournal);
        }
        // Persist actual captured bytes before publishing the stop record. A
        // crash before business-ledger completion cannot fabricate empty output.
        let data = serde_json::to_vec(result).map_err(|_| Error::TaskJournal)?;
        if data.len() > OUTCOME_BYTES - 36 {
            return Err(Error::TaskJournal);
        }
        let mut slot = vec![0u8; OUTCOME_BYTES];
        slot[..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
        slot[4..36].copy_from_slice(&Sha256::digest(&data));
        slot[36..36 + data.len()].copy_from_slice(&data);
        self.outcome.seek(SeekFrom::Start(0))?;
        self.outcome.write_all(&slot)?;
        self.outcome.sync_all()?;
        self.record.physical_stop_confirmed = true;
        self.record.output_complete = result.output_complete;
        self.record.outcome_sha256 = Some(cowork_domain::hash(&data));
        self.persist()?;
        Ok(cowork_domain::canonical_hash(&self.record))
    }
    fn persist(&mut self) -> Result<()> {
        self.record.sequence = self
            .record
            .sequence
            .checked_add(1)
            .ok_or(Error::TaskJournal)?;
        let data = serde_json::to_vec(&self.record).map_err(|_| Error::TaskJournal)?;
        if data.len() > SLOT_BYTES - 36 {
            return Err(Error::TaskJournal);
        }
        let mut slot = vec![0u8; SLOT_BYTES];
        slot[..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
        slot[4..36].copy_from_slice(&Sha256::digest(&data));
        slot[36..36 + data.len()].copy_from_slice(&data);
        let file = &mut self.files[(self.record.sequence % 2) as usize];
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&slot)?;
        file.sync_all()?;
        Ok(())
    }
}

/// Recheck the original completed record before consuming live resource
/// ownership. This function returns evidence only; it creates no capability.
pub(super) fn verify_completed(
    directory: &Dir,
    expected_sha256: &str,
    result: &cowork_contracts::ProcessResult,
    grant: &cowork_runner_protocol::ManagedInputGrant,
    work_id: &str,
    control_id: &str,
) -> Result<()> {
    use std::os::unix::fs::FileExt;
    let read = |name: &str, bytes: usize| -> Result<Vec<u8>> {
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
        let file = directory.open_with(name, &options)?.into_std();
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.nlink() != 1
            || metadata.mode() & 0o777 != 0o600
            || metadata.len() != bytes as u64
            || metadata
                .blocks()
                .checked_mul(512)
                .is_none_or(|n| n < bytes as u64)
        {
            return Err(Error::TaskJournal);
        }
        let mut frame = vec![0; bytes];
        file.read_exact_at(&mut frame, 0)?;
        let length =
            u32::from_le_bytes(frame[..4].try_into().map_err(|_| Error::TaskJournal)?) as usize;
        if length == 0
            || length > bytes - 36
            || Sha256::digest(&frame[36..36 + length]).as_slice() != &frame[4..36]
            || frame[36 + length..].iter().any(|b| *b != 0)
        {
            return Err(Error::TaskJournal);
        }
        Ok(frame[36..36 + length].to_vec())
    };
    let mut records = ["task.a", "task.b"]
        .into_iter()
        .filter_map(|name| {
            let bytes = read(name, SLOT_BYTES).ok()?;
            serde_json::from_slice::<PhysicalTaskRecord>(&bytes).ok()
        })
        .collect::<Vec<_>>();
    records.sort_by_key(|record| record.sequence);
    let record = records.pop().ok_or(Error::TaskJournal)?;
    if record.format_version != 2
        || record.sequence == 0
        || cowork_domain::canonical_hash(&record) != expected_sha256
        || record.request_sha256 != grant.request_sha256
        || record.plan_sha256 != grant.execution_plan_sha256
        || record.input_manifest_sha256 != grant.input_manifest_sha256
        || record.volume_id != work_id
        || record.control_volume_id != control_id
        || !record.physical_stop_confirmed
        || !record.control_process_reaped
        || !record.output_complete
        || !result.output_complete
        || record.mutations.len() != 12
        || record
            .mutations
            .iter()
            .any(|mutation| !mutation.acknowledged || mutation.response.is_none())
        || record.task_pid.is_none()
        || record.sandbox_pid.is_none()
        || record.reported_exit_status.is_none()
        || record.sandbox_reported_exit_status.is_none()
        || (!record.guest_exit_confirmed && result.exit_code.is_some())
    {
        return Err(Error::TaskJournal);
    }
    let outcome = read("outcome.bin", OUTCOME_BYTES)?;
    if record.outcome_sha256.as_deref() != Some(cowork_domain::hash(&outcome).as_str())
        || serde_json::from_slice::<cowork_contracts::ProcessResult>(&outcome)
            .map_err(|_| Error::TaskJournal)?
            != *result
    {
        return Err(Error::TaskJournal);
    }
    Ok(())
}
