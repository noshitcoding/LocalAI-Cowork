//! Actual fixed gVisor task adapter. Only a common one-use dispatch plus verified
//! recipient input can enter. Uncertain RPCs leave physical journals and frozen
//! resource reservations; there is no raw spec, host program or retry endpoint.
use super::*;
use super::{
    cgroups::{TaskBoundary, TaskCgroup},
    input::VerifiedTaskInput,
    output::BoundedOutput,
    profile::ManagedProfile,
    runsc_driver::WaitEvidence,
    task_journal::{Mutation, MutationResponse, PhysicalTaskRecord, TaskJournal},
};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::Dir;
use containerd_client::services::v1::{
    self as api, containers_client::ContainersClient, tasks_client::TasksClient,
};
use cowork_contracts::{ExecutionTargetRef, ProcessCapture, ProcessResult, ProcessTermination};
use cowork_runner_protocol::{Broker, DispatchedLaunch};
use prost::Message;
use serde_json::json;
use std::{
    collections::HashMap,
    io::Write,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

/// Wire-compatible with gVisor's pinned runtimeoptions.v1.Options. The runtime
/// config is private generated policy, never caller/model TOML.
#[derive(Clone, PartialEq, Message)]
struct ShimOptions {
    #[prost(string, tag = "1")]
    type_url: String,
    #[prost(string, tag = "2")]
    config_path: String,
}
pub struct ManagedTask {
    profile: Arc<ManagedProfile>,
    dispatch: DispatchedLaunch,
    input: VerifiedTaskInput,
    boundary: Option<TaskBoundary>,
    broker: Broker,
    cancel: Arc<AtomicBool>,
    task_id: String,
    sandbox_id: String,
    control: Dir,
    result_slot: std::fs::File,
    control_volume: crate::volumes::PublishedVolume,
    wait_evidence: WaitEvidence,
    output: Option<BoundedOutput>,
    spec: Vec<u8>,
    sandbox_spec: Vec<u8>,
    runtime: api::container::Runtime,
    started: Instant,
}
pub struct ManagedTaskOutcome {
    pub result: ProcessResult,
    /// Physical record fingerprint, not authority for another dispatch.
    pub physical_record_sha256: String,
    pub output_limit_exceeded: bool,
    pub control_exit: DaemonExit,
    input: VerifiedTaskInput,
    control_volume: crate::volumes::PublishedVolume,
    control: Dir,
    result_slot: std::fs::File,
}
/// Constructed only in this module after rechecking the original completed
/// physical outcome. Saved receipts/JSON cannot construct this capability.
pub(crate) struct CompletedVolumeRetirementSeal {
    work: (String, String),
    control: (String, String),
}
impl CompletedVolumeRetirementSeal {
    pub(crate) fn allows(&self, id: &str, request: &str) -> bool {
        (self.work.0 == id && self.work.1 == request)
            || (self.control.0 == id && self.control.1 == request)
    }
}
/// Independently reserved private control storage cannot be filled by guest
/// workspace writes. Both reservations stay booked on any uncertain effect.
pub struct TaskResources {
    pub group: TaskCgroup,
    pub control: crate::volumes::MountedVolume,
}
impl TaskResources {
    pub fn control_volume_request(request_sha256: &str) -> Result<crate::volumes::VolumeRequest> {
        if !crate::valid_hash(request_sha256) {
            return Err(Error::TaskAdmission);
        }
        Ok(crate::volumes::VolumeRequest {
            request_sha256: cowork_domain::canonical_hash(&(
                "managed_control_volume_v1",
                request_sha256,
            )),
            bytes: 16 * 1024 * 1024,
            inodes: 4096,
        })
    }
}
impl ManagedTaskOutcome {
    pub(super) fn retirement_seal(
        &self,
        receipt: &cowork_runner_protocol::Signed<cowork_runner_protocol::Receipt>,
    ) -> Result<CompletedVolumeRetirementSeal> {
        let grant = self.input.grant();
        receipt
            .verify("receipt", &grant.runner.runner_public_key)
            .map_err(|_| Error::TaskAdmission)?;
        if !matches!(&receipt.payload.state, cowork_runner_protocol::ExecutionState::Completed { result } if result == &self.result)
            || receipt.payload.request.process_attempt_id != grant.process_attempt_id
            || receipt.payload.request.run_id != grant.run_id
            || receipt.payload.request.request_sha256 != grant.request_sha256
            || !self.control_exit.daemon_process_reaped
        {
            return Err(Error::ManagedUncertain);
        }
        self.input.verify()?;
        self.control_volume
            .verify_published()
            .map_err(|_| Error::ManagedUncertain)?;
        super::task_journal::verify_completed(
            &self.control,
            &self.physical_record_sha256,
            &self.result,
            grant,
            self.input.volume().id(),
            self.control_volume.id(),
        )?;
        Ok(CompletedVolumeRetirementSeal {
            work: (
                self.input.volume().id().into(),
                grant.request_sha256.clone(),
            ),
            control: (
                self.control_volume.id().into(),
                TaskResources::control_volume_request(&grant.request_sha256)?.request_sha256,
            ),
        })
    }
    pub(super) fn into_retirement_parts(
        self,
        seal: CompletedVolumeRetirementSeal,
    ) -> super::retirement::RetiringTaskResources {
        let Self {
            input,
            control_volume,
            control,
            result_slot,
            ..
        } = self;
        drop(control);
        drop(result_slot);
        super::retirement::RetiringTaskResources::new(
            input.into_retirement_volume(),
            control_volume,
            seal,
        )
    }
    pub(super) fn input_grant(&self) -> &cowork_runner_protocol::ManagedInputGrant {
        self.input.grant()
    }
    pub(super) fn producer_instance_sha256(&self) -> String {
        self.input.producer_instance_sha256()
    }
    pub(super) fn persist_attested_snapshot(
        &self,
        manifest: &cowork_contracts::WorkspaceTransferManifest,
        stamps: &std::collections::BTreeMap<String, super::gvisor::Stamp>,
        receipt: &cowork_runner_protocol::Signed<cowork_runner_protocol::Receipt>,
        attestation: &cowork_runner_protocol::Signed<
            cowork_runner_protocol::ManagedOutputAttestation,
        >,
    ) -> Result<()> {
        use std::os::unix::fs::FileExt;
        let mut bytes = vec![0; RESULT_SLOT_BYTES];
        self.result_slot.read_exact_at(&mut bytes, 0)?;
        let size = u32::from_le_bytes(bytes[..4].try_into().map_err(|_| Error::Output)?) as usize;
        if size == 0
            || size > RESULT_SLOT_BYTES - 36
            || Sha256::digest(&bytes[36..36 + size]).as_slice() != &bytes[4..36]
            || bytes[36 + size..].iter().any(|b| *b != 0)
        {
            return Err(Error::Output);
        }
        let current: serde_json::Value =
            serde_json::from_slice(&bytes[36..36 + size]).map_err(|_| Error::Output)?;
        if current["format_version"] != 2
            || current["output_state"] != "ready"
            || current["execution_receipt_sha256"] != cowork_domain::canonical_hash(receipt)
            || current["manifest"] != serde_json::to_value(manifest).map_err(|_| Error::Output)?
        {
            return Err(Error::Output);
        }
        let signed = serde_json::to_value(attestation).map_err(|_| Error::Output)?;
        if let Some(previous) = current.get("attestation") {
            if *previous != signed {
                return Err(Error::ResultExport {
                    reason: "result_attestation_conflict",
                });
            }
            return Ok(());
        }
        let mut record = self.result_record("ready", receipt)?;
        record["manifest"] = serde_json::to_value(manifest).map_err(|_| Error::Output)?;
        record["local_native_stamps"] = serde_json::to_value(stamps).map_err(|_| Error::Output)?;
        record["attestation"] = signed;
        self.persist_result_record(record).map(|_| ())
    }
    /// Root-private recovery bytes, never a portable grant. Storage was fully
    /// reserved before any native task/fence creator was reached.
    pub(super) fn persist_result_snapshot(
        &self,
        manifest: &cowork_contracts::WorkspaceTransferManifest,
        stamps: &std::collections::BTreeMap<String, super::gvisor::Stamp>,
        receipt: &cowork_runner_protocol::Signed<cowork_runner_protocol::Receipt>,
    ) -> Result<()> {
        let mut record = self.result_record("ready", receipt)?;
        record["manifest"] = serde_json::to_value(manifest).map_err(|_| Error::Output)?;
        record["local_native_stamps"] = serde_json::to_value(stamps).map_err(|_| Error::Output)?;
        self.persist_result_record(record).map(|_| ())
    }
    pub(super) fn persist_result_rejection(
        &self,
        reason: &str,
        receipt: &cowork_runner_protocol::Signed<cowork_runner_protocol::Receipt>,
    ) -> Result<String> {
        if reason.len() > 128 {
            return Err(Error::Output);
        }
        let mut record = self.result_record("rejected", receipt)?;
        record["reason"] = reason.into();
        self.persist_result_record(record)
    }
    fn result_record(
        &self,
        state: &str,
        receipt: &cowork_runner_protocol::Signed<cowork_runner_protocol::Receipt>,
    ) -> Result<serde_json::Value> {
        receipt
            .verify("receipt", &self.input.grant().runner.runner_public_key)
            .map_err(|_| Error::Output)?;
        if receipt.payload.request.process_attempt_id != self.input.grant().process_attempt_id
            || receipt.payload.request.request_sha256 != self.input.grant().request_sha256
            || !matches!(&receipt.payload.state,cowork_runner_protocol::ExecutionState::Completed{result} if result==&self.result)
        {
            return Err(Error::Output);
        }
        Ok(serde_json::json!({"format_version":2,"output_state":state,
            "grant":self.input.grant(),"execution_receipt_sha256":cowork_domain::canonical_hash(receipt),
            "physical_record_sha256":self.physical_record_sha256,
            "process_result_sha256":cowork_domain::canonical_hash(&self.result)}))
    }
    fn persist_result_record(&self, record: serde_json::Value) -> Result<String> {
        use std::os::unix::fs::FileExt;
        self.workspace()?;
        let metadata = self.result_slot.metadata()?;
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let current = self
            .control
            .open_with("result-snapshot.bin", &options)?
            .into_std()
            .metadata()?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.nlink() != 1
            || metadata.mode() & 0o777 != 0o600
            || metadata.len() != RESULT_SLOT_BYTES as u64
            || metadata.blocks() * 512 < RESULT_SLOT_BYTES as u64
            || current.dev() != metadata.dev()
            || current.ino() != metadata.ino()
        {
            return Err(Error::Output);
        }
        let payload = serde_json::to_vec(&record).map_err(|_| Error::Output)?;
        if payload.len() > RESULT_SLOT_BYTES - 36 {
            return Err(Error::Output);
        }
        let mut bytes = vec![0; RESULT_SLOT_BYTES];
        bytes[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes[4..36].copy_from_slice(&Sha256::digest(&payload));
        bytes[36..36 + payload.len()].copy_from_slice(&payload);
        self.result_slot.write_all_at(&bytes, 0)?;
        self.result_slot.sync_all()?;
        Ok(cowork_domain::hash(&payload))
    }
    /// Called only after actual task/delete/EOF/native-boundary completion.
    /// Result publication still needs the app's separate manifest/version path.
    pub fn workspace(&self) -> Result<Dir> {
        self.input.verify()?;
        self.control_volume
            .verify_published()
            .map_err(|_| Error::TaskAdmission)?;
        Ok(self.input.workspace().try_clone()?)
    }
    pub fn reserved_volume_id(&self) -> &str {
        self.input.volume().id()
    }
    pub fn reserved_control_volume_id(&self) -> &str {
        self.control_volume.id()
    }
}
impl ManagedTask {
    pub fn prepare(
        profile: Arc<ManagedProfile>,
        dispatch: DispatchedLaunch,
        input: VerifiedTaskInput,
        resources: TaskResources,
        broker: Broker,
        cancel: Arc<AtomicBool>,
        current_target: &ExecutionTargetRef,
    ) -> Result<Self> {
        let TaskResources {
            group,
            control: control_storage,
        } = resources;
        profile.verify()?;
        if !broker.is_bound_to(&profile.binding) {
            return Err(Error::TaskAdmission);
        }
        broker
            .validate_owned_dispatch(&dispatch)
            .map_err(|_| Error::TaskAdmission)?;
        group.verify()?;
        let request = dispatch.request();
        let wall = now()?;
        input.validate_dispatch(&dispatch, &broker, &profile.binding, current_target, wall)?;
        if profile.target != *current_target
            || profile.reference != request.environment
            || profile.binding.runner_id != request.runner_id
            || profile.binding.generation != request.runner_generation
            || profile.binding.adapter_build_sha256 != request.adapter_build_sha256
            || group.limits() != profile.limits()
            || request.issued_at > wall
            || wall >= request.expires_at
            || wall >= request.execution_deadline
            || cancel.load(Ordering::Acquire)
        {
            return Err(Error::TaskAdmission);
        }
        input
            .volume()
            .require_limits(
                profile.definition.limits.scratch_bytes,
                profile.maximum_files,
            )
            .map_err(|_| Error::TaskAdmission)?;
        let program = profile
            .definition
            .programs
            .get(&request.invocation.program)
            .ok_or(Error::TaskAdmission)?;
        let program = profile.rootfs.executable(program)?;
        let task_id = format!("cw-{}", uuid::Uuid::new_v4().simple());
        // gVisor accepts abbreviated IDs; neither ID may prefix the other.
        let sandbox_id = format!("sb-{}", uuid::Uuid::new_v4().simple());
        let control_volume = control_storage
            .publish_for_dispatch()
            .map_err(|_| Error::TaskAdmission)?;
        if control_volume.request_sha256()
            != TaskResources::control_volume_request(&request.request_sha256)?.request_sha256
        {
            return Err(Error::TaskAdmission);
        }
        control_volume
            .require_limits(16 * 1024 * 1024, 4096)
            .map_err(|_| Error::TaskAdmission)?;
        let root = control_volume.directory();
        root.create_dir(&task_id)?;
        root.set_permissions(
            &task_id,
            cap_std::fs::Permissions::from_std(std::fs::Permissions::from_mode(0o700)),
        )?;
        let control = root.open_dir_nofollow(&task_id)?;
        let result_slot = reserve_result_slot(&control)?;
        let control_path = native_path(&control)?;
        let wall = now()?;
        if wall < request.issued_at
            || wall >= request.expires_at
            || wall >= request.execution_deadline
            || wall >= input.grant().initial_execution_deadline(request)
        {
            return Err(Error::TaskAdmission);
        }
        let lifetime = Duration::from_secs(request.invocation.timeout_seconds)
            .min(Duration::from_millis(
                (input.grant().initial_execution_deadline(request) - wall) as u64,
            ))
            .min(Duration::from_millis((request.expires_at - wall) as u64));
        if lifetime.is_zero() {
            return Err(Error::TaskAdmission);
        }
        let started = Instant::now();
        let boundary = TaskBoundary::arm(
            group,
            (started + lifetime).min(input.initial_deadline()),
            cancel.clone(),
            &profile.fence,
        )?;
        let wait_evidence = WaitEvidence::prepare(
            &control,
            &profile.driver,
            &profile.tools,
            &request.request_sha256,
            &task_id,
            &sandbox_id,
        )?;
        let config = profile.tools.configuration(
            &boundary.shim_path(),
            &control_path.join("runsc"),
            profile.driver.path(),
        )?;
        let mut options = cap_std::fs::OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
        let mut config_file = control.open_with("runsc.toml", &options)?.into_std();
        config_file.write_all(config.as_bytes())?;
        config_file.sync_all()?;
        control.open(".")?.into_std().sync_all()?;
        let runtime = api::container::Runtime {
            name: profile.tools.shim_path()?.to_string_lossy().into_owned(),
            options: Some(prost_types::Any {
                type_url: "runtimeoptions.v1.Options".into(),
                value: ShimOptions {
                    type_url: "io.containerd.runsc.v1.options".into(),
                    config_path: control_path
                        .join("runsc.toml")
                        .to_string_lossy()
                        .into_owned(),
                }
                .encode_to_vec(),
            }),
        };
        let mut arguments = vec![program];
        arguments.extend(request.invocation.arguments.iter().cloned());
        let limits = profile.limits();
        let spec=serde_json::to_vec(&json!({"ociVersion":"1.2.0","hostname":"cowork","annotations":{"io.kubernetes.cri.container-type":"container","io.kubernetes.cri.sandbox-id":sandbox_id},"root":{"path":profile.rootfs.path(),"readonly":true},
            "process":{"terminal":false,"user":{"uid":1000,"gid":1000,"additionalGids":[]},"args":arguments,"cwd":"/workspace",
                "env":["PATH=/usr/local/bin:/usr/bin:/bin","HOME=/tmp","TMPDIR=/tmp","LANG=C.UTF-8"],"noNewPrivileges":true,
                "capabilities":{"bounding":[],"effective":[],"inheritable":[],"permitted":[],"ambient":[]},
                "rlimits":[{"type":"RLIMIT_NOFILE","hard":256,"soft":256},{"type":"RLIMIT_CORE","hard":0,"soft":0}]},
            "mounts":[{"destination":"/proc","type":"proc","source":"proc","options":["nosuid","noexec","nodev"]},
                {"destination":"/dev","type":"tmpfs","source":"tmpfs","options":["nosuid","noexec","mode=0755","size=1m"]},
                {"destination":"/sys","type":"sysfs","source":"sysfs","options":["nosuid","noexec","nodev","ro"]},
                {"destination":"/workspace","type":"bind","source":native_path(input.workspace())?,"options":["rbind","rw","nodev","nosuid","noexec","rprivate"]},
                {"destination":"/tmp","type":"bind","source":native_path(input.temporary())?,"options":["rbind","rw","nodev","nosuid","noexec","rprivate"]}],
            "linux":{"cgroupsPath":boundary.sandbox_path(),"namespaces":[{"type":"pid"},{"type":"network"},{"type":"ipc"},{"type":"uts"},{"type":"mount"}],
                "resources":{"memory":{"limit":limits.memory_bytes,"swap":limits.memory_bytes},"cpu":{"quota":u64::from(limits.cpu_millis)*100,"period":100000},"pids":{"limit":limits.processes}},
                "maskedPaths":["/proc/acpi","/proc/kcore","/proc/keys","/proc/timer_list","/sys/firmware"],
                "readonlyPaths":["/proc/asound","/proc/bus","/proc/fs","/proc/irq","/proc/sys","/proc/sysrq-trigger"]}})).map_err(|_|Error::TaskAdmission)?;
        if spec.len() > 65536 {
            return Err(Error::TaskAdmission);
        }
        let mut sandbox_value: serde_json::Value =
            serde_json::from_slice(&spec).map_err(|_| Error::TaskAdmission)?;
        sandbox_value["annotations"]["io.kubernetes.cri.container-type"] = json!("sandbox");
        sandbox_value["process"]["args"] = json!(["/bin/cowork-pause"]);
        let sandbox_spec = serde_json::to_vec(&sandbox_value).map_err(|_| Error::TaskAdmission)?;
        let keep = (profile.output_bytes as usize).min(65536);
        let output = BoundedOutput::create(&control_path, keep, profile.output_bytes)?;
        Ok(Self {
            profile,
            dispatch,
            input,
            boundary: Some(boundary),
            broker,
            cancel,
            task_id,
            sandbox_id,
            control,
            result_slot,
            control_volume,
            output: Some(output),
            spec,
            sandbox_spec,
            runtime,
            wait_evidence,
            started,
        })
    }
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    pub async fn start_daemon(&self, executable: DaemonExecutable) -> Result<PrivateDaemon> {
        self.profile.verify()?;
        self.input.validate_dispatch(
            &self.dispatch,
            &self.broker,
            &self.profile.binding,
            &self.profile.target,
            now()?,
        )?;
        if !self
            .broker
            .execution_allowed(self.dispatch.request())
            .await
            .map_err(|_| Error::TaskAdmission)?
        {
            return Err(Error::TaskAdmission);
        }
        let boundary = self.boundary.as_ref().ok_or(Error::Cgroup)?;
        boundary.dispatch_ready()?;
        self.control.create_dir("daemon")?;
        self.control.set_permissions(
            "daemon",
            cap_std::fs::Permissions::from_std(std::fs::Permissions::from_mode(0o700)),
        )?;
        let directory = native_path(&self.control.open_dir_nofollow("daemon")?)?;
        PrivateDaemon::start_scoped(
            executable,
            &directory,
            &cowork_domain::canonical_hash(&self.profile.binding),
            Duration::from_secs(
                self.dispatch
                    .request()
                    .invocation
                    .timeout_seconds
                    .saturating_add(30)
                    .min(86400),
            ),
            boundary.shim_placement()?,
            self.wait_evidence.environment(),
        )
        .await
    }
    async fn allowed(&self, connection: &ContainerdConnection) -> Result<()> {
        self.profile.verify()?;
        self.input.verify()?;
        self.control_volume
            .verify_published()
            .map_err(|_| Error::TaskAdmission)?;
        connection.binding.verify()?;
        let daemon_group =
            std::fs::read_to_string(format!("/proc/{}/cgroup", connection.binding.peer.pid))?;
        let shim = self.boundary.as_ref().ok_or(Error::Cgroup)?.shim_path();
        if !daemon_group
            .lines()
            .any(|s| s.strip_prefix("0::") == Some(shim.as_str()))
        {
            return Err(Error::Cgroup);
        }
        // A private daemon namespace does not automatically see volumes mounted
        // after its creation. Native local identities must agree before any
        // creator RPC; equal path strings are never sufficient.
        for directory in [
            &self.control,
            self.input.workspace(),
            self.input.temporary(),
        ] {
            let path = native_path(directory)?;
            let through_daemon =
                PathBuf::from(format!("/proc/{}/root", connection.binding.peer.pid))
                    .join(path.strip_prefix("/").map_err(|_| Error::TaskAdmission)?);
            let host = directory.open(".")?.into_std().metadata()?;
            let daemon = std::fs::metadata(through_daemon)?;
            if host.dev() != daemon.dev() || host.ino() != daemon.ino() {
                return Err(Error::TaskAdmission);
            }
        }
        self.boundary
            .as_ref()
            .ok_or(Error::Cgroup)?
            .dispatch_ready()?;
        self.output.as_ref().ok_or(Error::Output)?.verify()?;
        let wall = now()?;
        let request = self.dispatch.request();
        self.input.validate_dispatch(
            &self.dispatch,
            &self.broker,
            &self.profile.binding,
            &self.profile.target,
            wall,
        )?;
        if wall < request.issued_at
            || wall >= request.expires_at
            || wall >= request.execution_deadline
            || self.cancel.load(Ordering::Acquire)
            || self
                .boundary
                .as_ref()
                .ok_or(Error::Cgroup)?
                .observation()
                .triggered
            || !self
                .broker
                .execution_allowed(request)
                .await
                .map_err(|_| Error::TaskAdmission)?
        {
            return Err(Error::TaskAdmission);
        }
        Ok(())
    }
    /// Caller must drive this in the existing independent worker, not in an
    /// HTTP request future. Drop still freezes/kills the exact workload boundary
    /// and retains journals/reservations; it cannot turn uncertainty into replay.
    pub async fn run(mut self, daemon: PrivateDaemon) -> Result<ManagedTaskOutcome> {
        let connection = daemon.connection();
        if connection.namespace
            != format!(
                "cowork-{}",
                &cowork_domain::canonical_hash(&self.profile.binding)[..32]
            )
        {
            return Err(Error::TaskAdmission);
        }
        self.allowed(connection).await?;
        let peer = &connection.binding.peer;
        let mut journal = TaskJournal::create(
            self.control.try_clone()?,
            PhysicalTaskRecord {
                format_version: 2,
                sequence: 0,
                task_id: self.task_id.clone(),
                sandbox_id: self.sandbox_id.clone(),
                sandbox_spec_sha256: cowork_domain::hash(&self.sandbox_spec),
                namespace: connection.namespace.clone(),
                request_sha256: self.dispatch.request().request_sha256.clone(),
                plan_sha256: self.input.grant().execution_plan_sha256.clone(),
                input_manifest_sha256: self.input.grant().input_manifest_sha256.clone(),
                volume_id: self.input.volume().id().into(),
                control_volume_id: self.control_volume.id().into(),
                spec_sha256: cowork_domain::hash(&self.spec),
                definition_sha256: self.profile.reference.definition_sha256.clone(),
                daemon_pid: peer.pid,
                daemon_start_ticks: peer.start_ticks,
                daemon_executable_sha256: peer.executable_sha256.clone(),
                task_pid: None,
                sandbox_pid: None,
                reported_exit_status: None,
                sandbox_reported_exit_status: None,
                guest_exit_confirmed: false,
                guest_wait_evidence_sha256: None,
                sandbox_live_at_guest_wait: false,
                control_process_reaped: false,
                cgroup_path: self.boundary.as_ref().ok_or(Error::Cgroup)?.sandbox_path(),
                mutations: vec![],
                physical_stop_confirmed: false,
                output_complete: false,
                outcome_sha256: None,
            },
        )?;
        let done = Arc::new(AtomicBool::new(false));
        let watcher_done = done.clone();
        let watcher_cancel = self.cancel.clone();
        let broker = self.broker.clone();
        let request = self.dispatch.request().clone();
        // Bounded private diagnosis survives the failure that stopped a job.
        // It is never a receipt, authority source or reason to retry dispatch.
        let mut diagnostic_options = cap_std::fs::OpenOptions::new();
        use cap_std::fs::OpenOptionsExt;
        diagnostic_options
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .follow(FollowSymlinks::No);
        let diagnostic = self
            .control
            .open_with("authority-observation.bin", &diagnostic_options)?
            .into_std();
        if unsafe { libc::posix_fallocate(diagnostic.as_raw_fd(), 0, 4096) } != 0 {
            return Err(Error::TaskJournal);
        }
        diagnostic.sync_all()?;
        let watcher = tokio::spawn(async move {
            while !watcher_done.load(Ordering::Acquire) {
                let observed = broker.execution_allowed(&request).await;
                if !matches!(observed, Ok(true)) {
                    let detail = match observed {
                        Ok(false) => "authority_denied".to_owned(),
                        Err(error) => format!("{error:?}").chars().take(1024).collect(),
                        _ => unreachable!(),
                    };
                    let data = serde_json::to_vec(
                        &json!({"request_sha256":request.request_sha256,"observed_at":now().ok(),"detail":detail,"clock_rollback":broker.last_clock_rollback_observation()}),
                    );
                    if let Ok(data) = data
                        && data.len() <= 4096 - 36
                    {
                        let mut slot = [0u8; 4096];
                        slot[..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
                        slot[4..36].copy_from_slice(&Sha256::digest(&data));
                        slot[36..36 + data.len()].copy_from_slice(&data);
                        use std::os::unix::fs::FileExt;
                        let _ = diagnostic.write_all_at(&slot, 0);
                        let _ = diagnostic.sync_all();
                    }
                    watcher_cancel.store(true, Ordering::Release);
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        struct Watcher {
            done: Arc<AtomicBool>,
            handle: Option<tokio::task::JoinHandle<()>>,
        }
        impl Watcher {
            async fn finish(mut self) -> Result<()> {
                self.done.store(true, Ordering::Release);
                if let Some(handle) = self.handle.take() {
                    tokio::time::timeout(Duration::from_secs(5), handle)
                        .await
                        .map_err(|_| Error::Lifecycle)?
                        .map_err(|_| Error::Lifecycle)?;
                }
                Ok(())
            }
        }
        impl Drop for Watcher {
            fn drop(&mut self) {
                self.done.store(true, Ordering::Release);
                // Do not abort a database transaction mid-flight. This task has
                // read/admission-observation authority only and exits after its
                // current bounded operation, without creating another effect.
            }
        }
        let _watcher = Watcher {
            done,
            handle: Some(watcher),
        };
        let mut labels = HashMap::new();
        labels.insert(
            "localai.cowork.request".into(),
            self.dispatch.request().request_sha256.clone(),
        );
        labels.insert(
            "localai.cowork.plan".into(),
            self.input.grant().execution_plan_sha256.clone(),
        );
        labels.insert(
            "localai.cowork.volume".into(),
            self.input.volume().id().into(),
        );
        let expected = api::Container {
            id: self.task_id.clone(),
            labels,
            runtime: Some(self.runtime.clone()),
            spec: Some(prost_types::Any {
                type_url: "types.containerd.io/opencontainers/runtime-spec/1/Spec".into(),
                value: self.spec.clone(),
            }),
            ..Default::default()
        };
        let mut containers = ContainersClient::new(connection.channel.clone());
        let mut tasks = TasksClient::new(connection.channel.clone());
        let mut sandbox = expected.clone();
        sandbox.id = self.sandbox_id.clone();
        sandbox.spec.as_mut().ok_or(Error::TaskAdmission)?.value = self.sandbox_spec.clone();
        journal.begin(Mutation::ContainerCreate, &self.sandbox_id)?;
        let created = containers
            .create(rpc(
                api::CreateContainerRequest {
                    container: Some(sandbox.clone()),
                },
                &connection.namespace,
                Duration::from_secs(10),
            ))
            .await
            .map_err(task_rpc_error("sandbox_container_create"))?
            .into_inner()
            .container
            .ok_or(Error::Rpc)?;
        same_container(&sandbox, &created)?;
        journal.acknowledge()?;
        self.allowed(connection).await?;
        journal.begin(Mutation::TaskCreate, &self.sandbox_id)?;
        let created = tasks
            .create(rpc(
                api::CreateTaskRequest {
                    container_id: self.sandbox_id.clone(),
                    ..Default::default()
                },
                &connection.namespace,
                Duration::from_secs(15),
            ))
            .await
            .map_err(task_rpc_error("sandbox_task_create"))?
            .into_inner();
        if created.container_id != self.sandbox_id || created.pid == 0 {
            return Err(Error::Rpc);
        }
        let sandbox_pid = created.pid;
        journal.record_sandbox_pid(sandbox_pid)?;
        self.require_workload_pid(sandbox_pid)?;
        journal.acknowledge()?;
        self.allowed(connection).await?;
        journal.begin(Mutation::TaskStart, &self.sandbox_id)?;
        let started = tasks
            .start(rpc(
                api::StartRequest {
                    container_id: self.sandbox_id.clone(),
                    ..Default::default()
                },
                &connection.namespace,
                Duration::from_secs(10),
            ))
            .await
            .map_err(task_rpc_error("sandbox_task_start"))?
            .into_inner();
        if started.pid != sandbox_pid {
            return Err(Error::Rpc);
        }
        journal.acknowledge()?;
        self.allowed(connection).await?;
        journal.begin(Mutation::ContainerCreate, &self.task_id)?;
        let created = containers
            .create(rpc(
                api::CreateContainerRequest {
                    container: Some(expected.clone()),
                },
                &connection.namespace,
                Duration::from_secs(10),
            ))
            .await
            .map_err(task_rpc_error("container_create"))?
            .into_inner()
            .container
            .ok_or(Error::Rpc)?;
        same_container(&expected, &created)?;
        journal.acknowledge()?;
        self.allowed(connection).await?;
        check_container(&mut containers, &expected, &connection.namespace).await?;
        journal.begin(Mutation::TaskCreate, &self.task_id)?;
        let output = self.output.as_ref().ok_or(Error::Output)?;
        let created = tasks
            .create(rpc(
                api::CreateTaskRequest {
                    container_id: self.task_id.clone(),
                    stdout: output.stdout_path().to_string_lossy().into_owned(),
                    stderr: output.stderr_path().to_string_lossy().into_owned(),
                    ..Default::default()
                },
                &connection.namespace,
                Duration::from_secs(15),
            ))
            .await
            .map_err(task_rpc_error("task_create"))?
            .into_inner();
        if created.container_id != self.task_id || created.pid == 0 {
            return Err(Error::Rpc);
        }
        let pid = created.pid;
        journal.record_created_pid(pid)?;
        if pid != sandbox_pid {
            return Err(Error::Rpc);
        }
        self.require_workload_pid(pid)?;
        journal.acknowledge()?;
        self.allowed(connection).await?;
        check_container(&mut containers, &expected, &connection.namespace).await?;
        journal.begin(Mutation::TaskStart, &self.task_id)?;
        let start = tasks
            .start(rpc(
                api::StartRequest {
                    container_id: self.task_id.clone(),
                    ..Default::default()
                },
                &connection.namespace,
                Duration::from_secs(10),
            ))
            .await
            .map_err(task_rpc_error("task_start"))?
            .into_inner();
        if start.pid != pid {
            return Err(Error::Rpc);
        }
        journal.acknowledge()?;
        let mut termination = ProcessTermination::Exited;
        let mut output_limit = false;
        let wait = tasks.wait(rpc(
            api::WaitRequest {
                container_id: self.task_id.clone(),
                ..Default::default()
            },
            &connection.namespace,
            Duration::from_secs(86400),
        ));
        tokio::pin!(wait);
        let exit = loop {
            tokio::select! {
                result=&mut wait=>break result.map_err(|_|Error::Rpc)?.into_inner(),
                _=tokio::time::sleep(Duration::from_millis(20))=>{
                    connection.binding.verify()?;
                    let output=self.output.as_ref().ok_or(Error::Output)?;let boundary=self.boundary.as_ref().ok_or(Error::Cgroup)?;
                    output_limit|=output.limit_exceeded();
                    if output_limit||output.observer_failed(){self.cancel.store(true,Ordering::Release);boundary.request_stop();}
                    if boundary.observation().triggered||self.cancel.load(Ordering::Acquire){
                        termination=if output_limit||self.cancel.load(Ordering::Acquire){ProcessTermination::Canceled}else{ProcessTermination::TimedOut};
                        boundary.request_stop();
                    }
                }
            }
        };
        if exit.exited_at.is_none() {
            return Err(Error::Rpc);
        }
        output_limit |= self.output.as_ref().ok_or(Error::Output)?.limit_exceeded();
        if output_limit {
            self.cancel.store(true, Ordering::Release);
            self.boundary.as_ref().ok_or(Error::Cgroup)?.request_stop();
        }
        journal.record_exit(exit.exit_status)?;
        if self
            .boundary
            .as_ref()
            .ok_or(Error::Cgroup)?
            .observation()
            .triggered
            || self.cancel.load(Ordering::Acquire)
        {
            termination = if self.cancel.load(Ordering::Acquire) {
                ProcessTermination::Canceled
            } else {
                ProcessTermination::TimedOut
            };
        }
        if termination == ProcessTermination::Exited {
            // Root liveness cannot distinguish a synthetic internal-error
            // status from the actual guest's status. Require the pinned wait.
            let state = TasksClient::new(connection.channel.clone())
                .get(rpc(
                    api::GetRequest {
                        container_id: self.sandbox_id.clone(),
                        ..Default::default()
                    },
                    &connection.namespace,
                    Duration::from_secs(5),
                ))
                .await
                .map_err(task_rpc_error("sandbox_state"))?
                .into_inner()
                .process
                .ok_or(Error::Rpc)?;
            if state.pid != sandbox_pid || state.status != 2 {
                return Err(Error::Lifecycle);
            }
            journal.record_live_sandbox_at_guest_wait()?;
            let guest_exit = self.wait_evidence.verified_workload_exit()?;
            if guest_exit.status() != exit.exit_status {
                return Err(Error::GuestExit);
            }
            journal.confirm_guest_exit(&guest_exit)?;
        }
        // Waiting for the init process does not kill its surviving descendants.
        // Kill the exact task before deletion even on a normal root exit.
        check_container(&mut containers, &expected, &connection.namespace).await?;
        journal.begin(Mutation::TaskKill, &self.task_id)?;
        let kill = TasksClient::new(connection.channel.clone())
            .kill(rpc(
                api::KillRequest {
                    container_id: self.task_id.clone(),
                    exec_id: String::new(),
                    signal: 9,
                    all: true,
                },
                &connection.namespace,
                Duration::from_secs(5),
            ))
            .await;
        let response = match kill {
            Ok(_) => MutationResponse::Succeeded,
            Err(status) => match status.code() {
                tonic::Code::NotFound => MutationResponse::RejectedNotFound,
                tonic::Code::FailedPrecondition => MutationResponse::RejectedFailedPrecondition,
                _ => return Err(Error::Rpc),
            },
        };
        journal.acknowledge_response(response)?;
        let boundary = self.boundary.as_ref().ok_or(Error::Cgroup)?;
        // Workload exit is already observed while the held root is alive. Now
        // terminate the complete job branch, including its fixed pause root.
        boundary.request_stop();
        let until = Instant::now() + Duration::from_secs(3);
        while !boundary.workload_empty()? {
            boundary.request_stop();
            if Instant::now() >= until {
                return Err(Error::Cgroup);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.output
            .as_ref()
            .ok_or(Error::Output)?
            .drain_stopped_workload()?;
        journal.begin(Mutation::TaskDelete, &self.task_id)?;
        let deleted = TasksClient::new(connection.channel.clone())
            .delete(rpc(
                api::DeleteTaskRequest {
                    container_id: self.task_id.clone(),
                },
                &connection.namespace,
                Duration::from_secs(10),
            ))
            .await
            .map_err(task_rpc_error("task_delete"))?
            .into_inner();
        // DeleteTask returns an empty process/exec id for the init task. The
        // request/container binding and exact PID identify this deletion.
        if !deleted.id.is_empty()
            || deleted.pid != pid
            || deleted.exit_status != exit.exit_status
            || deleted.exited_at.is_none()
        {
            return Err(Error::Rpc);
        }
        journal.acknowledge()?;
        check_container(&mut containers, &expected, &connection.namespace).await?;
        journal.begin(Mutation::ContainerDelete, &self.task_id)?;
        containers
            .delete(rpc(
                api::DeleteContainerRequest {
                    id: self.task_id.clone(),
                },
                &connection.namespace,
                Duration::from_secs(5),
            ))
            .await
            .map_err(|_| Error::Rpc)?;
        journal.acknowledge()?;
        check_container(&mut containers, &sandbox, &connection.namespace).await?;
        journal.begin(Mutation::TaskKill, &self.sandbox_id)?;
        let response = match TasksClient::new(connection.channel.clone())
            .kill(rpc(
                api::KillRequest {
                    container_id: self.sandbox_id.clone(),
                    signal: 9,
                    all: true,
                    ..Default::default()
                },
                &connection.namespace,
                Duration::from_secs(5),
            ))
            .await
        {
            Ok(_) => MutationResponse::Succeeded,
            Err(status) => match status.code() {
                tonic::Code::NotFound => MutationResponse::RejectedNotFound,
                tonic::Code::FailedPrecondition => MutationResponse::RejectedFailedPrecondition,
                _ => return Err(Error::Rpc),
            },
        };
        journal.acknowledge_response(response)?;
        let root_exit = TasksClient::new(connection.channel.clone())
            .wait(rpc(
                api::WaitRequest {
                    container_id: self.sandbox_id.clone(),
                    ..Default::default()
                },
                &connection.namespace,
                Duration::from_secs(5),
            ))
            .await
            .map_err(task_rpc_error("sandbox_wait"))?
            .into_inner();
        if root_exit.exited_at.is_none() {
            return Err(Error::Rpc);
        }
        journal.begin(Mutation::TaskDelete, &self.sandbox_id)?;
        let deleted = TasksClient::new(connection.channel.clone())
            .delete(rpc(
                api::DeleteTaskRequest {
                    container_id: self.sandbox_id.clone(),
                },
                &connection.namespace,
                Duration::from_secs(10),
            ))
            .await
            .map_err(task_rpc_error("sandbox_task_delete"))?
            .into_inner();
        if !deleted.id.is_empty()
            || deleted.pid != sandbox_pid
            || deleted.exited_at.is_none()
            || deleted.exit_status != root_exit.exit_status
        {
            return Err(Error::Rpc);
        }
        journal.record_sandbox_exit(deleted.exit_status)?;
        journal.acknowledge()?;
        journal.begin(Mutation::ContainerDelete, &self.sandbox_id)?;
        containers
            .delete(rpc(
                api::DeleteContainerRequest {
                    id: self.sandbox_id.clone(),
                },
                &connection.namespace,
                Duration::from_secs(5),
            ))
            .await
            .map_err(task_rpc_error("sandbox_container_delete"))?;
        journal.acknowledge()?;
        connection.binding.verify()?;
        let control_exit = daemon.stop().await?;
        if !control_exit.daemon_process_reaped {
            return Err(Error::Lifecycle);
        }
        journal.record_control_reaped(control_exit.outcome.process_id)?;
        let boundary = self.boundary.take().ok_or(Error::Cgroup)?;
        // All creator responses were acknowledged; only now may the native
        // owned empty boundary retire. Unknown create/start never reaches here.
        boundary.retire_deleted()?;
        let output = self.output.take().ok_or(Error::Output)?.finish()?;
        if !output.complete {
            return Err(Error::Output);
        }
        self.profile.verify()?;
        self.input.verify()?;
        let capture = |c: cowork_executor::Capture| ProcessCapture {
            sha256: cowork_domain::hash(&c.bytes),
            bytes: c.bytes,
            total_bytes: c.total_bytes,
            truncated: c.truncated,
        };
        let exit_code = if termination == ProcessTermination::Exited {
            Some(i64::from(exit.exit_status))
        } else {
            None
        };
        let result = ProcessResult {
            termination,
            process_id: Some(pid),
            // A forced host sandbox kill may make the shim report its internal
            // sentinel 128, not a measured guest exit. Preserve the raw report
            // in the physical journal and do not invent a guest exit code.
            exit_code,
            signal: None,
            stdout: capture(output.stdout),
            stderr: capture(output.stderr),
            duration_ms: self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            output_complete: true,
        };
        let physical_record_sha256 = journal.confirm_stopped(&result)?;
        _watcher.finish().await?;
        Ok(ManagedTaskOutcome {
            result,
            physical_record_sha256,
            output_limit_exceeded: output_limit || output.limit_exceeded,
            control_exit,
            input: self.input,
            control_volume: self.control_volume,
            control: self.control,
            result_slot: self.result_slot,
        })
    }
    fn require_workload_pid(&self, pid: u32) -> Result<()> {
        let expected = self.boundary.as_ref().ok_or(Error::Cgroup)?.sandbox_path();
        if !std::fs::read_to_string(format!("/proc/{pid}/cgroup"))?
            .lines()
            .any(|s| s.strip_prefix("0::") == Some(expected.as_str()))
        {
            return Err(Error::Cgroup);
        }
        Ok(())
    }
}
// Portable manifest bytes also bound the sum of escaped native-index paths.
// A serialized native Stamp is <384 bytes even at numeric maxima. The extra
// 64KiB covers the strictly bounded grant and fixed hashes/record framing.
const RESULT_SLOT_BYTES: usize = 8 * 1024 * 1024;
const _: () = assert!(
    2 * cowork_domain::workspace_transfer::MAX_MANIFEST_BYTES
        + (cowork_domain::workspace_transfer::MAX_ENTRIES + 1) * 384
        + 64 * 1024
        + 36
        <= RESULT_SLOT_BYTES
);
fn reserve_result_slot(directory: &Dir) -> Result<std::fs::File> {
    use cap_std::fs::OpenOptionsExt;
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .follow(FollowSymlinks::No);
    let file = directory
        .open_with("result-snapshot.bin", &options)?
        .into_std();
    let code = unsafe { libc::posix_fallocate(file.as_raw_fd(), 0, RESULT_SLOT_BYTES as i64) };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code).into());
    }
    let metadata = file.metadata()?;
    if metadata.len() != RESULT_SLOT_BYTES as u64
        || metadata.blocks() * 512 < RESULT_SLOT_BYTES as u64
    {
        return Err(Error::Output);
    }
    file.sync_all()?;
    directory.open(".")?.into_std().sync_all()?;
    Ok(file)
}
pub(super) fn native_path(dir: &Dir) -> Result<PathBuf> {
    let path = std::fs::read_link(format!("/proc/self/fd/{}", dir.as_raw_fd()))?;
    if !path.is_absolute() || path.as_os_str().as_encoded_bytes().ends_with(b" (deleted)") {
        return Err(Error::Input);
    }
    Ok(path)
}
fn task_rpc_error(operation: &'static str) -> impl FnOnce(tonic::Status) -> Error {
    move |status| Error::TaskRpc {
        operation,
        code: status.code().to_string(),
        detail: status.message().chars().take(2048).collect(),
    }
}
fn now() -> Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::TaskAdmission)?
        .as_millis()
        .try_into()
        .map_err(|_| Error::TaskAdmission)
}
fn rpc<T>(value: T, namespace: &str, timeout: Duration) -> Request<T> {
    let mut request = Request::new(value);
    request.metadata_mut().insert(
        "containerd-namespace",
        namespace.parse().expect("derived private namespace"),
    );
    request.set_timeout(timeout);
    request
}
fn same_container(expected: &api::Container, actual: &api::Container) -> Result<()> {
    if expected.id != actual.id
        || expected.labels != actual.labels
        || expected.runtime != actual.runtime
        || expected.spec != actual.spec
        || !actual.image.is_empty()
        || !actual.snapshot_key.is_empty()
    {
        return Err(Error::TaskAdmission);
    }
    Ok(())
}
async fn check_container(
    client: &mut ContainersClient<Channel>,
    expected: &api::Container,
    namespace: &str,
) -> Result<()> {
    let actual = client
        .get(rpc(
            api::GetContainerRequest {
                id: expected.id.clone(),
            },
            namespace,
            Duration::from_secs(5),
        ))
        .await
        .map_err(|_| Error::Rpc)?
        .into_inner()
        .container
        .ok_or(Error::Rpc)?;
    same_container(expected, &actual)
}

#[cfg(test)]
mod result_slot_tests {
    use super::*;
    #[test]
    fn accepted_large_portable_manifest_and_native_index_fit_reserved_result_slot() {
        let maximum: super::super::gvisor::Stamp = serde_json::from_value(serde_json::json!({
            "dev":u64::MAX,"ino":u64::MAX,"bytes":u64::MAX,"mode":u32::MAX,
            "uid":u32::MAX,"links":u64::MAX,"mtime":[i64::MIN,i64::MIN],"ctime":[i64::MIN,i64::MIN]
        }))
        .unwrap();
        assert!(serde_json::to_vec(&maximum).unwrap().len() <= 384);
        let directory = "a".repeat(140);
        let files = (0..8000)
            .map(|n| cowork_contracts::WorkspaceTransferFile {
                path: format!("{directory}/f{n:04}"),
                size: 0,
                sha256: cowork_domain::hash(b""),
                chunks: vec![],
            })
            .collect();
        let manifest =
            cowork_domain::workspace_transfer::build(vec![directory.clone()], files).unwrap();
        let mut stamps =
            std::collections::BTreeMap::from([(String::new(), maximum), (directory, maximum)]);
        for file in &manifest.files {
            stamps.insert(file.path.clone(), maximum);
        }
        let record =
            serde_json::to_vec(&serde_json::json!({"format_version":1,"manifest":manifest,
            "local_native_stamps":stamps,"grant":"g".repeat(32*1024),
            "physical_record_sha256":"a".repeat(64),"process_result_sha256":"b".repeat(64)}))
            .unwrap();
        assert!(
            record.len() > 4 * 1024 * 1024,
            "Regression must exceed the former four-MiB reservation"
        );
        assert!(
            record.len() + 36 <= RESULT_SLOT_BYTES,
            "Every accepted manifest needs actual preallocated recovery space"
        );
    }
}
