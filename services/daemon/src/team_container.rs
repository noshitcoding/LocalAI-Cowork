//! Operator-configured team container adapter. The guest never receives host
//! data roots, daemon credentials, Docker authority or global GitHub credentials.
use cowork_domain::{Error, Result};
use cowork_toolbox::{Action, Request, Toolbox, WorkspaceLease};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;
#[path = "team_docker_host.rs"]
mod docker_host;
#[path = "team_env_file.rs"]
mod env_file;
use docker_host::HostBinding;
#[path = "team_lost_host_records.rs"]
mod lost_host;
#[path = "team_container_recovery.rs"]
mod recovery;
const DOCKER_HOST: &str = "--host=unix:///var/run/docker.sock";

#[derive(Clone)]
pub struct ContainerConfig {
    pub image: String,
    pub runtime: Option<String>,
    /// Actual Docker-host directory mapped to Toolbox's workspaces directory.
    pub host_workspace_root: PathBuf,
    /// Private daemon directory, outside the guest workspace mount.
    pub control_directory: PathBuf,
    /// "none" by default; other values require operator-controlled egress rules.
    pub network: String,
    pub host_id: Option<Uuid>,
    pub node_name: Option<String>,
    pub node_bindings_file: Option<PathBuf>,
}
#[derive(Clone)]
pub(super) struct Containers {
    toolbox: Arc<Toolbox>,
    config: ContainerConfig,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    format: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host: Option<HostBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trusted_host_admission: Option<cowork_storage::team_service::TeamHostAdmissionRef>,
    nonce: Uuid,
    name: String,
    image: String,
    team: Uuid,
    order: Uuid,
    run: Uuid,
    container_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github: Option<(i64, String)>,
}
#[derive(Deserialize)]
struct Inspection {
    id: String,
    name: String,
    image: String,
    labels: BTreeMap<String, String>,
    state: ContainerState,
}
#[derive(Deserialize)]
struct ContainerState {
    running: bool,
    paused: bool,
    restarting: bool,
    pid: i64,
}
struct CliOutcome {
    exit_code: Option<i64>,
    exited: bool,
    output_complete: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
fn invalid() -> Error {
    Error::Validation("Invalid team container configuration".into())
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn regular(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).map_err(cowork_domain::storage_error)?;
    if !meta.is_file() || meta.is_symlink() {
        return Err(invalid());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(invalid());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(invalid());
        }
    }
    Ok(())
}
impl Containers {
    pub(super) fn open(toolbox: Arc<Toolbox>, config: ContainerConfig) -> Result<Self> {
        let (name, digest) = if let Some(digest) = config.image.strip_prefix("sha256:") {
            ("local", digest)
        } else {
            config.image.rsplit_once("@sha256:").ok_or_else(invalid)?
        };
        if name.is_empty()
            || name.len() > 512
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-/:".contains(&b))
            || digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_hexdigit())
            || config
                .runtime
                .as_ref()
                .is_some_and(|runtime| !identifier(runtime))
            || !identifier(&config.network)
            || config.network == "host"
            || !config.host_workspace_root.is_absolute()
            || config.host_workspace_root.to_string_lossy().contains(',')
            || !config.control_directory.is_absolute()
        {
            return Err(invalid());
        }
        // Startup provides a private owner-controlled directory. No repair or
        // permission broadening is performed here.
        for parent in config.control_directory.ancestors() {
            let meta = fs::symlink_metadata(parent).map_err(cowork_domain::storage_error)?;
            if !meta.is_dir() || meta.is_symlink() {
                return Err(invalid());
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0 {
                    return Err(invalid());
                }
            }
        }
        Ok(Self { toolbox, config })
    }
    pub(super) fn network_enabled(&self) -> bool {
        self.config.network != "none"
    }
    fn file(&self, record: &Record) -> PathBuf {
        self.config
            .control_directory
            .join(format!("ct-{}.json", record.nonce))
    }
    pub(super) fn prepare(
        &self,
        team: Uuid,
        order: Uuid,
        run: Uuid,
        tool: &str,
        action: serde_json::Value,
        github: Option<(i64, String)>,
    ) -> Result<(Record, Request)> {
        let workspace = self
            .toolbox
            .workspace(&order.to_string())
            .map_err(|_| invalid())?;
        let workspaces = workspace.parent().ok_or_else(invalid)?;
        if self.config.control_directory.starts_with(workspaces) {
            return Err(invalid());
        }
        let nonce = Uuid::new_v4();
        let host_binding = self.host(nonce)?;
        let record = Record {
            format: 2,
            host: Some(host_binding),
            trusted_host_admission: None,
            nonce,
            name: format!("cowork-team-{nonce}"),
            image: self.config.image.clone(),
            team,
            order,
            run,
            container_id: None,
            github,
        };
        let host = self.config.host_workspace_root.join(order.to_string());
        // Creation does not start the guest. Its exact immutable ID is committed
        // and checked before a separate, fresh-authorized start command.
        let mut args = vec![
            DOCKER_HOST.into(),
            "create".into(),
            "--pull=never".into(),
            "--name".into(),
            record.name.clone(),
            "--user".into(),
            "1000:1000".into(),
            "--read-only".into(),
            "--cap-drop=ALL".into(),
            "--security-opt=no-new-privileges:true".into(),
            "--pids-limit=128".into(),
            "--memory=1g".into(),
            "--cpus=2".into(),
            "--tmpfs".into(),
            "/tmp:rw,nosuid,nodev,size=256m,mode=1777".into(),
            "--network".into(),
            self.config.network.clone(),
            "--workdir".into(),
            "/workspace".into(),
            "--mount".into(),
            format!("type=bind,src={},dst=/workspace", host.display()),
            "--env".into(),
            "LANG=C.UTF-8".into(),
            "--env".into(),
            "HOME=/tmp/home".into(),
            "--env".into(),
            "TMPDIR=/tmp".into(),
        ];
        if record.github.is_some() {
            if tool != "github" {
                return Err(Error::Forbidden);
            }
            args.extend([
                "--env-file".into(),
                env_file::path(&self.config.control_directory, nonce)
                    .to_str()
                    .ok_or_else(invalid)?
                    .to_owned(),
            ]);
        }
        for (key, value) in record.labels() {
            args.extend(["--label".into(), format!("{key}={value}")]);
        }
        if let Some(runtime) = &self.config.runtime {
            args.extend(["--runtime".into(), runtime.clone()]);
        }
        args.extend([
            "--entrypoint".into(),
            "/opt/cowork/python/bin/python".into(),
            self.config.image.clone(),
        ]);
        if tool == "browser_session" {
            args.extend([
                "-I".into(),
                "/opt/cowork-team-tools/browser_session.py".into(),
                "serve".into(),
                action["session_id"].as_str().ok_or_else(invalid)?.into(),
            ]);
        } else {
            args.extend([
                "/opt/cowork-team-tools/guest.py".into(),
                serde_json::to_string(&serde_json::json!({"tool":tool,"action":action}))
                    .map_err(cowork_domain::storage_error)?,
            ]);
        }
        Ok((
            record,
            Request {
                idempotency_key: nonce.to_string(),
                action: Action::Terminal {
                    program: "docker".into(),
                    arguments: args,
                    cwd: String::new(),
                },
                timeout_seconds: 120,
                output_bytes: 128 * 1024,
                approval_sha256: None,
            },
        ))
    }
    fn cli(&self, order: Uuid, mut args: Vec<String>) -> Result<CliOutcome> {
        args.insert(0, DOCKER_HOST.into());
        let order = order.to_string();
        let mut request = Request {
            idempotency_key: Uuid::new_v4().to_string(),
            action: Action::Terminal {
                program: "docker".into(),
                arguments: args,
                cwd: String::new(),
            },
            timeout_seconds: 60,
            output_bytes: 128 * 1024,
            approval_sha256: None,
        };
        request.approval_sha256 = Some(
            self.toolbox
                .preview(&order, &request)
                .map_err(|_| Error::UncertainEffect)?
                .request_sha256,
        );
        let job = self
            .toolbox
            .submit(&order, request)
            .map_err(|_| Error::UncertainEffect)?;
        let deadline = Instant::now() + Duration::from_secs(65);
        loop {
            let state = self
                .toolbox
                .status(&order, &job.id)
                .map_err(|_| Error::UncertainEffect)?;
            if state.state == "completed" {
                let output = state.outcome.ok_or(Error::UncertainEffect)?;
                return Ok(CliOutcome {
                    exit_code: output.exit_code,
                    exited: serde_json::to_value(output.termination)
                        .map_err(cowork_domain::storage_error)?
                        .as_str()
                        == Some("exited"),
                    output_complete: output.output_complete,
                    stdout: output.stdout.bytes,
                    stderr: output.stderr.bytes,
                });
            }
            if state.state != "running" || Instant::now() > deadline {
                let _ = self.toolbox.cancel(&order, &job.id);
                return Err(Error::UncertainEffect);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    fn inspect(&self, record: &Record, target: &str) -> Result<Option<Inspection>> {
        self.validate_host(record)?;
        self.inspect_original(record, target)
    }
    fn inspect_original(&self, record: &Record, target: &str) -> Result<Option<Inspection>> {
        // Control commands do not enter the guest workspace or contend with the
        // order lease held until actual container removal.
        let output=self.cli(record.nonce,vec!["inspect".into(),"--format".into(),
            "{\"id\":{{json .Id}},\"name\":{{json .Name}},\"image\":{{json .Config.Image}},\"labels\":{{json .Config.Labels}},\"state\":{\"running\":{{json .State.Running}},\"paused\":{{json .State.Paused}},\"restarting\":{{json .State.Restarting}},\"pid\":{{json .State.Pid}}}}".into(),target.into()])?;
        if !output.exited || !output.output_complete {
            return Err(Error::UncertainEffect);
        }
        if output.exit_code == Some(0) {
            let value: Inspection =
                serde_json::from_slice(&output.stdout).map_err(|_| Error::UncertainEffect)?;
            if value.id.len() != 64
                || (target.len() == 64 && value.id != target)
                || !value.id.bytes().all(|b| b.is_ascii_hexdigit())
                || value.name != format!("/{}", record.name)
                || value.image != record.image
                || record
                    .labels()
                    .iter()
                    .any(|(key, expected)| expected != value_in(&value.labels, key))
            {
                return Err(Error::Forbidden);
            }
            return Ok(Some(value));
        }
        let error = String::from_utf8_lossy(&output.stderr);
        if output.exit_code == Some(1)
            && (error.contains(&format!("No such object: {target}"))
                || error.contains(&format!("No such container: {target}")))
        {
            return Ok(None);
        }
        Err(Error::UncertainEffect)
    }
    fn cleanup(&self, record: &Record, env_file: &mut Option<env_file::EnvFile>) -> Result<()> {
        let id = record.container_id.as_ref().ok_or(Error::UncertainEffect)?;
        if let Some(first) = self.inspect(record, id)? {
            if first.state.running || first.state.pid != 0 {
                let stopped = self.cli(
                    record.nonce,
                    vec![
                        "stop".into(),
                        "--time".into(),
                        "45".into(),
                        first.id.clone(),
                    ],
                )?;
                if stopped.exit_code != Some(0) || !stopped.exited || !stopped.output_complete {
                    return Err(Error::UncertainEffect);
                }
            }
            // Positive process exit is required on the same bound Docker engine.
            if let Some(second) = self.inspect(record, &first.id)? {
                if second.id != first.id
                    || second.state.running
                    || second.state.paused
                    || second.state.restarting
                    || second.state.pid != 0
                {
                    return Err(Error::Forbidden);
                }
                let output = self.cli(record.nonce, vec!["rm".into(), first.id.clone()])?;
                if output.exit_code != Some(0) || !output.exited {
                    return Err(Error::UncertainEffect);
                }
                if self.inspect(record, &first.id)?.is_some() {
                    return Err(Error::UncertainEffect);
                }
            }
        }
        let path = self.file(record);
        regular(&path)?;
        // Unknown records and their env-files are retained. Confirmed absence
        // permits removal of the exact private credential file before receipt.
        if record.github.is_some() {
            if let Some(file) = env_file.take() {
                file.remove()?;
            } else {
                let secret = env_file::path(&self.config.control_directory, record.nonce);
                if secret.exists() {
                    regular(&secret)?;
                    fs::remove_file(secret).map_err(cowork_domain::storage_error)?;
                }
            }
        }
        fs::remove_file(path).map_err(cowork_domain::storage_error)
    }
    fn records(&self) -> Result<Vec<Record>> {
        let mut records = Vec::new();
        for entry in
            fs::read_dir(&self.config.control_directory).map_err(cowork_domain::storage_error)?
        {
            let path = entry.map_err(cowork_domain::storage_error)?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(invalid)?;
            if let Some(nonce) = name
                .strip_prefix("gh-")
                .and_then(|v| v.strip_suffix(".env"))
            {
                Uuid::parse_str(nonce).map_err(|_| invalid())?;
                regular(&path)?;
                continue;
            }
            if name == "hosts" {
                let metadata = fs::symlink_metadata(&path).map_err(cowork_domain::storage_error)?;
                if !metadata.is_dir() || metadata.is_symlink() {
                    return Err(invalid());
                }
                continue;
            }
            if name == "retired" {
                docker_host::private(&path, true)?;
                continue;
            }
            if !name.starts_with("ct-") || !name.ends_with(".json") {
                return Err(invalid());
            }
            regular(&path)?;
            let meta = fs::metadata(&path).map_err(cowork_domain::storage_error)?;
            if meta.len() > 4096 {
                return Err(invalid());
            }
            let record: Record =
                serde_json::from_slice(&fs::read(&path).map_err(cowork_domain::storage_error)?)
                    .map_err(|_| invalid())?;
            if !matches!(record.format, 1 | 2)
                || (record.format == 2 && record.host.is_none())
                || (record.format == 1 && record.host.is_some())
                || record.name != format!("cowork-team-{}", record.nonce)
                || self.file(&record) != path
            {
                return Err(invalid());
            }
            if records.len() >= 2048 {
                return Err(invalid());
            }
            records.push(record);
        }
        Ok(records)
    }
    pub(super) fn files_available(&self, order: Uuid) -> Result<()> {
        if self.records()?.iter().any(|record| record.order == order) {
            return Err(Error::UncertainEffect);
        }
        Ok(())
    }
    pub(super) fn recover(&self) -> Result<()> {
        for _record in self.records()? {
            // A shared directory does not prove that another worker is dead.
            // Without its expired SQL lease + exclusive worker witness we retain
            // this blocked/unknown record and perform no Docker IO or kills.
        }
        Ok(())
    }
    pub(super) fn created(
        &self,
        record: &mut Record,
        job: &cowork_toolbox::Job,
    ) -> Result<Request> {
        let outcome = job.outcome.as_ref().ok_or(Error::UncertainEffect)?;
        if job.state != "completed"
            || outcome.exit_code != Some(0)
            || !outcome.output_complete
            || serde_json::to_value(&outcome.termination)
                .map_err(cowork_domain::storage_error)?
                .as_str()
                != Some("exited")
        {
            return Err(Error::UncertainEffect);
        }
        let id = String::from_utf8(outcome.stdout.bytes.clone())
            .map_err(|_| Error::UncertainEffect)?
            .trim()
            .to_owned();
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::UncertainEffect);
        }
        record.container_id = Some(id.clone());
        if self.inspect(record, &id)?.is_none() {
            return Err(Error::UncertainEffect);
        }
        let path = self.file(record);
        regular(&path)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path)
            .map_err(cowork_domain::storage_error)?;
        file.write_all(&record.bytes()?)
            .map_err(cowork_domain::storage_error)?;
        file.sync_all().map_err(cowork_domain::storage_error)?;
        let order = record.order.to_string();
        let mut request = Request {
            idempotency_key: format!("start-{}", record.nonce),
            action: Action::Terminal {
                program: "docker".into(),
                arguments: vec![DOCKER_HOST.into(), "start".into(), "--attach".into(), id],
                cwd: String::new(),
            },
            timeout_seconds: 120,
            output_bytes: 128 * 1024,
            approval_sha256: None,
        };
        request.approval_sha256 = Some(
            self.toolbox
                .preview(&order, &request)
                .map_err(|_| Error::UncertainEffect)?
                .request_sha256,
        );
        Ok(request)
    }
    pub(super) fn guard(&self, record: Record, lease: Arc<WorkspaceLease>) -> Result<Guard> {
        self.validate_host(&record)?;
        // Called only while the order lease is held. No guest can race file IO,
        // and a previous unknown container blocks a new admission.
        self.files_available(record.order)?;
        let path = self.file(&record);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x0020_0000).share_mode(1);
        }
        let mut file = options.open(path).map_err(cowork_domain::storage_error)?;
        file.write_all(&record.bytes()?)
            .map_err(cowork_domain::storage_error)?;
        file.sync_all().map_err(cowork_domain::storage_error)?;
        Ok(Guard {
            containers: self.clone(),
            record: Arc::new(Mutex::new(record)),
            _lease: lease,
            job: None,
            finished: false,
            env_file: None,
        })
    }
    fn host(&self, nonce: Uuid) -> Result<HostBinding> {
        let output = self.cli(
            nonce,
            vec!["info".into(), "--format".into(), "{{json .ID}}".into()],
        )?;
        if output.exit_code != Some(0) || !output.exited || !output.output_complete {
            return Err(Error::UncertainEffect);
        }
        let engine: String =
            serde_json::from_slice(&output.stdout).map_err(|_| Error::UncertainEffect)?;
        docker_host::resolve(&self.config, engine)
    }
    fn validate_host(&self, record: &Record) -> Result<()> {
        let binding = record.host.as_ref().ok_or(Error::UncertainEffect)?;
        if record.format != 2 || self.host(record.nonce)? != *binding {
            return Err(Error::UncertainEffect);
        }
        Ok(())
    }
}
fn value_in<'a>(labels: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    labels.get(key).map(String::as_str).unwrap_or("")
}
impl Record {
    fn labels(&self) -> BTreeMap<String, String> {
        let mut labels = BTreeMap::from([
            ("cowork.team-tool.nonce".into(), self.nonce.to_string()),
            ("cowork.team-tool.team".into(), self.team.to_string()),
            ("cowork.team-tool.order".into(), self.order.to_string()),
            ("cowork.team-tool.run".into(), self.run.to_string()),
            ("cowork.team-tool.image".into(), self.image.clone()),
        ]);
        if let Some((revision, host)) = &self.github {
            labels.insert(
                "cowork.team-tool.github-revision".into(),
                revision.to_string(),
            );
            labels.insert("cowork.team-tool.github-host".into(), host.clone());
        }
        if let Some(host) = &self.host {
            labels.insert("cowork.team-tool.host-id".into(), host.host_id.to_string());
            labels.insert("cowork.team-tool.engine-id".into(), host.engine_id.clone());
            if let Some(name) = &host.node_name {
                labels.insert("cowork.team-tool.node-name".into(), name.clone());
            }
            if let Some(uid) = host.node_uid {
                labels.insert("cowork.team-tool.node-uid".into(), uid.to_string());
            }
        }
        labels
    }
    pub(super) fn bytes(&self) -> Result<Vec<u8>> {
        if self.trusted_host_admission.is_some() {
            serde_json::to_vec(&serde_json::to_value(self).map_err(cowork_domain::storage_error)?)
                .map_err(cowork_domain::storage_error)
        } else {
            serde_json::to_vec(self).map_err(cowork_domain::storage_error)
        }
    }
    pub(super) fn nonce(&self) -> Uuid {
        self.nonce
    }
    pub(super) async fn admit(
        &mut self,
        authority: &cowork_storage::team_service::TeamExecutionAuthority,
        approval: Uuid,
    ) -> Result<()> {
        self.trusted_host_admission = authority
            .admit_container_host(approval, &self.bytes()?)
            .await?;
        Ok(())
    }
    pub(super) fn public(&self) -> serde_json::Value {
        serde_json::json!({"container_name":self.name,"container_nonce":self.nonce,"image":self.image,"team_id":self.team,"order_id":self.order,"run_id":self.run,"github":self.github})
    }
}
pub(super) struct Guard {
    containers: Containers,
    record: Arc<Mutex<Record>>,
    _lease: Arc<WorkspaceLease>,
    job: Option<String>,
    finished: bool,
    env_file: Option<env_file::EnvFile>,
}
impl Guard {
    pub(super) fn original_bytes(&self) -> Result<Vec<u8>> {
        self.record.lock().map_err(|_| Error::Conflict)?.bytes()
    }
    pub(super) fn nonce(&self) -> Result<Uuid> {
        Ok(self.record.lock().map_err(|_| Error::Conflict)?.nonce)
    }
    pub(super) fn check_original_host(&self) -> Result<()> {
        let record = self.record.lock().map_err(|_| Error::Conflict)?;
        self.containers.validate_host(&record)
    }
    pub(super) fn created_background(&mut self, job: &cowork_toolbox::Job) -> Result<Request> {
        let mut request = self.created(job)?;
        if let Action::Terminal { arguments, .. } = &mut request.action {
            arguments.retain(|a| a != "--attach");
        }
        request.approval_sha256 = Some(
            self.containers
                .toolbox
                .preview(
                    &self
                        .record
                        .lock()
                        .map_err(|_| Error::Conflict)?
                        .order
                        .to_string(),
                    &request,
                )
                .map_err(|_| Error::UncertainEffect)?
                .request_sha256,
        );
        Ok(request)
    }
    pub(super) fn browser_request(&self, action: serde_json::Value) -> Result<Request> {
        let record = self.record.lock().map_err(|_| Error::Conflict)?;
        let id = record.container_id.as_ref().ok_or(Error::UncertainEffect)?;
        if self.containers.inspect(&record, id)?.is_none() {
            return Err(Error::UncertainEffect);
        }
        Ok(Request {
            idempotency_key: Uuid::new_v4().to_string(),
            action: Action::Terminal {
                program: "docker".into(),
                arguments: vec![
                    DOCKER_HOST.into(),
                    "exec".into(),
                    id.clone(),
                    "/opt/cowork/python/bin/python".into(),
                    "-I".into(),
                    "/opt/cowork-team-tools/browser_session.py".into(),
                    "call".into(),
                    serde_json::to_string(&action).map_err(cowork_domain::storage_error)?,
                ],
                cwd: String::new(),
            },
            timeout_seconds: 120,
            output_bytes: 128 * 1024,
            approval_sha256: None,
        })
    }
    pub(super) fn public(&self) -> Result<serde_json::Value> {
        Ok(self.record.lock().map_err(|_| Error::Conflict)?.public())
    }
    pub(super) fn github(
        &mut self,
        credential: cowork_storage::team_service::TeamGithubCredential,
    ) -> Result<()> {
        let record = self.record.lock().map_err(|_| Error::Conflict)?;
        if record.github.as_ref() != Some(&(credential.revision, credential.hostname.clone())) {
            return Err(Error::Forbidden);
        }
        self.env_file = Some(env_file::EnvFile::create(
            &self.containers.config.control_directory,
            record.nonce,
            &credential.hostname,
            &credential.token,
        )?);
        Ok(())
    }
    pub(super) fn created(&mut self, job: &cowork_toolbox::Job) -> Result<Request> {
        let mut record = self.record.lock().map_err(|_| Error::Conflict)?;
        self.containers.created(&mut record, job)
    }
    pub(super) fn started(&mut self, id: String) {
        self.job = Some(id);
    }
    fn cleanup(&mut self) -> Result<()> {
        let record = self.record.lock().map_err(|_| Error::Conflict)?;
        if let Some(job) = &self.job {
            let _ = self
                .containers
                .toolbox
                .cancel(&record.order.to_string(), job);
        }
        self.containers.cleanup(&record, &mut self.env_file)
    }
    pub(super) fn finish(mut self) -> Result<()> {
        let result = self.cleanup();
        self.finished = true;
        result
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.cleanup();
        }
    }
}
