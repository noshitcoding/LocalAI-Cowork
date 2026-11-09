//! A distinct actor-private gVisor adapter. Guest stdin is the only configuration/secret channel.
use super::*;
use cowork_domain::canonical_hash;
use cowork_storage::android_private_runner::PrivateContainerAdmission;
use std::collections::BTreeMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use uuid::Uuid;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    format: u32,
    docker_executable: PathBuf,
    docker_socket: PathBuf,
    host_id: Uuid,
    engine_id: String,
    image: String,
    programs: BTreeMap<String, String>,
    #[serde(default)]
    office: bool,
    #[serde(default)]
    browser: bool,
    #[serde(default)]
    terminal_program: Option<String>,
    #[serde(default)]
    github: bool,
}
pub(super) struct Runner {
    config: Configuration,
    binding: Value,
    store: Store,
    keys: Arc<dyn KeyStore>,
}
pub(super) struct Container {
    pub nonce: Uuid,
    pub physical_id: String,
    child: Child,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    stop: CancellationToken,
    finished: Option<tokio::sync::oneshot::Receiver<Result<Value>>>,
    stderr: Option<tokio::task::JoinHandle<Result<Vec<u8>>>>,
}
impl Drop for Container {
    fn drop(&mut self) {
        self.stop.cancel();
        let _ = self.child.start_kill();
    }
}
fn unavailable() -> Error {
    Error::UncertainEffect
}
fn control_path(path: &Path, _socket: bool) -> Result<()> {
    for ancestor in path.ancestors() {
        let m = std::fs::symlink_metadata(ancestor).map_err(|_| invalid())?;
        if m.is_symlink() {
            return Err(invalid());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{FileTypeExt, MetadataExt};
            if m.uid() != 0 && m.uid() != unsafe { libc::geteuid() } {
                return Err(invalid());
            }
            if (!_socket || ancestor != path) && m.mode() & 0o022 != 0 {
                return Err(invalid());
            }
            if ancestor == path
                && if _socket {
                    !m.file_type().is_socket()
                } else {
                    !m.is_file()
                }
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
async fn bounded(mut read: impl tokio::io::AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = read.read(&mut buf).await.map_err(|_| unavailable())?;
        if n == 0 {
            return Ok(bytes);
        }
        if bytes.len().checked_add(n).is_none_or(|n| n > limit) {
            return Err(Error::BudgetExceeded);
        }
        bytes.extend_from_slice(&buf[..n]);
    }
}
impl Runner {
    pub(super) fn from_environment(
        data: &Path,
        web: Option<&Path>,
        store: Store,
        keys: Arc<dyn KeyStore>,
    ) -> Result<Option<Arc<Self>>> {
        let Some(path) =
            std::env::var_os("COWORK_ANDROID_PRIVATE_RUNNER_CONFIG").filter(|v| !v.is_empty())
        else {
            return Ok(None);
        };
        if !cfg!(target_os = "linux") {
            return Err(invalid());
        }
        let raw = private_account(data, web, &PathBuf::from(path))?;
        let config: Configuration = serde_json::from_str(&raw).map_err(|_| invalid())?;
        if config.format != 1
            || !config.docker_executable.is_absolute()
            || !config.docker_socket.is_absolute()
            || config.host_id.is_nil()
            || config.engine_id.is_empty()
            || config.engine_id.len() > 128
            || config.engine_id.chars().any(char::is_control)
            || config.programs.len() > 32
            || config
                .image
                .rsplit_once("@sha256:")
                .is_none_or(|(name, d)| {
                    name.is_empty()
                        || name.len() > 512
                        || !cowork_domain::processes::valid_sha256(d)
                })
            || config
                .image
                .bytes()
                .any(|b| !b.is_ascii_alphanumeric() && !b"_.-/:@".contains(&b))
        {
            return Err(invalid());
        }
        for (name, path) in &config.programs {
            if name.is_empty()
                || name.len() > 128
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                || !path.starts_with('/')
                || path.contains(['\0', '\\', ':'])
                || path
                    .split('/')
                    .skip(1)
                    .any(|s| s.is_empty() || s == "." || s == "..")
            {
                return Err(invalid());
            }
        }
        control_path(&config.docker_executable, false)?;
        let image = &config.image;
        let mut binding = json!({"format":2,"runtime":"runsc","network":"none","host_id":config.host_id,"engine_id":config.engine_id,"image":image,"programs":config.programs,"office":config.office,"browser":config.browser,"docker_socket":config.docker_socket,"docker_executable":config.docker_executable,"memory_bytes":1073741824u64,"scratch_bytes":268435456u64,"processes":128,"cpu_nanos":2000000000u64,"supervisor_uid":0,"process_uid":1000,"capabilities":["SETUID","SETGID","KILL","DAC_OVERRIDE","CHOWN"]});
        // Disabled extensions do not change any pre-existing binding bytes.
        if let Some(alias) = &config.terminal_program {
            if !config.programs.contains_key(alias) {
                return Err(invalid());
            }
            binding["terminal_program"] = json!(alias);
        }
        if config.github {
            binding["github"] = json!(true);
        }
        Ok(Some(Arc::new(Self {
            config,
            binding,
            store,
            keys,
        })))
    }
    pub(super) fn binding(&self) -> Value {
        self.binding.clone()
    }
    fn command(&self, args: &[String]) -> Command {
        let mut command = Command::new(&self.config.docker_executable);
        command
            .arg(format!(
                "--host=unix://{}",
                self.config.docker_socket.display()
            ))
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        command
    }
    async fn cli(&self, args: Vec<String>) -> Result<Vec<u8>> {
        let child = self.command(&args).spawn().map_err(|_| unavailable())?;
        self.cli_result(child).await
    }
    async fn create_cli(
        &self,
        args: Vec<String>,
        permit: cowork_storage::android_private_runner::PrivateCreateDispatch,
        stop: &CancellationToken,
    ) -> Result<Vec<u8>> {
        if stop.is_cancelled() {
            self.store
                .android_private_container_close_unconsumed(permit)
                .await?;
            return Err(unavailable());
        }
        let child = match self.command(&args).spawn() {
            Ok(child) => child,
            Err(_) => {
                self.store
                    .android_private_container_close_unconsumed(permit)
                    .await?;
                return Err(unavailable());
            }
        };
        // Successful OS spawn may already have sent the original create. The
        // no-send capability is consumed synchronously before any other await.
        permit.begin_io();
        self.cli_result(child).await
    }
    async fn cli_result(&self, mut child: Child) -> Result<Vec<u8>> {
        let out = child.stdout.take().ok_or_else(unavailable)?;
        let err = child.stderr.take().ok_or_else(unavailable)?;
        let (status, stdout, stderr) = tokio::time::timeout(Duration::from_secs(20), async {
            tokio::join!(child.wait(), bounded(out, 1024 * 1024), bounded(err, 65536))
        })
        .await
        .map_err(|_| unavailable())?;
        if !status.map_err(|_| unavailable())?.success() || !stderr?.is_empty() {
            return Err(unavailable());
        }
        stdout
    }
    async fn engine(&self) -> Result<()> {
        control_path(&self.config.docker_executable, false)?;
        control_path(&self.config.docker_socket, true)?;
        let bytes = self
            .cli(vec!["info".into(), "--format".into(), "{{json .}}".into()])
            .await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if value["ID"] != self.config.engine_id || value.pointer("/Runtimes/runsc").is_none() {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
    fn labels(&self, record: &Value) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "cowork.android-private.nonce".into(),
                record["id"].as_str().unwrap_or("").into(),
            ),
            (
                "cowork.android-private.run".into(),
                record["run_id"].as_str().unwrap_or("").into(),
            ),
            (
                "cowork.android-private.owner".into(),
                record["owner_id"].as_str().unwrap_or("").into(),
            ),
            (
                "cowork.android-private.project".into(),
                record["project_id"].as_str().unwrap_or("").into(),
            ),
            (
                "cowork.android-private.creation".into(),
                record["creation_sha256"].as_str().unwrap_or("").into(),
            ),
            (
                "cowork.android-private.binding".into(),
                canonical_hash(&record["binding"]),
            ),
        ])
    }
    async fn inspect(&self, record: &Value, target: &str) -> Result<Option<Value>> {
        self.engine().await?;
        let ids = self
            .cli(vec![
                "container".into(),
                "ls".into(),
                "--all".into(),
                "--no-trunc".into(),
                "--filter".into(),
                format!(
                    "name=^/cowork-android-private-{}$",
                    record["id"].as_str().ok_or_else(invalid)?
                ),
                "--format".into(),
                "{{.ID}}".into(),
            ])
            .await?;
        let found = std::str::from_utf8(&ids).map_err(|_| unavailable())?.trim();
        if found.is_empty() {
            return Ok(None);
        }
        if found.len() != 64
            || !found
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || (!record["container_id"].is_null() && record["container_id"] != found)
            || (target.len() == 64 && target != found)
        {
            return Err(Error::Forbidden);
        }
        let bytes = self.cli(vec!["inspect".into(), found.into()]).await?;
        let values: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        let value = values
            .as_array()
            .filter(|v| v.len() == 1)
            .and_then(|v| v.first())
            .ok_or_else(unavailable)?;
        if value["Id"] != found
            || value.pointer("/Config/Image") != Some(&self.binding["image"])
            || value.pointer("/HostConfig/Runtime") != Some(&json!("runsc"))
            || value.pointer("/HostConfig/NetworkMode") != Some(&json!("none"))
            || value.pointer("/HostConfig/ReadonlyRootfs") != Some(&json!(true))
            || value.pointer("/HostConfig/Memory") != Some(&self.binding["memory_bytes"])
            || value.pointer("/HostConfig/MemorySwap") != Some(&self.binding["memory_bytes"])
            || value.pointer("/HostConfig/PidsLimit") != Some(&self.binding["processes"])
            || value.pointer("/HostConfig/NanoCpus") != Some(&self.binding["cpu_nanos"])
            || value.pointer("/Config/User") != Some(&json!("0:0"))
            || value
                .pointer("/HostConfig/CapAdd")
                .and_then(Value::as_array)
                .is_none_or(|v| {
                    v.len() != 5
                        || !self.binding["capabilities"]
                            .as_array()
                            .is_some_and(|expected| {
                                expected.iter().all(|cap| {
                                    v.iter().any(|actual| {
                                        actual.as_str().zip(cap.as_str()).is_some_and(|(a, b)| {
                                            a.strip_prefix("CAP_")
                                                .unwrap_or(a)
                                                .eq_ignore_ascii_case(b)
                                        })
                                    })
                                })
                            })
                })
            || !value["Mounts"]
                .as_array()
                .is_some_and(|v| v.iter().all(|m| m["Type"] == "tmpfs"))
            || value
                .pointer("/HostConfig/Binds")
                .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
            || value
                .pointer("/HostConfig/CapDrop")
                .and_then(Value::as_array)
                .is_none_or(|v| {
                    !v.iter()
                        .any(|v| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("ALL")))
                })
            || value
                .pointer("/HostConfig/SecurityOpt")
                .and_then(Value::as_array)
                .is_none_or(|v| {
                    !v.iter()
                        .any(|v| v == "no-new-privileges" || v == "no-new-privileges:true")
                })
        {
            return Err(Error::Forbidden);
        }
        for (key, expected) in self.labels(record) {
            if value["Config"]["Labels"][key] != expected {
                return Err(Error::Forbidden);
            }
        }
        Ok(Some(value.clone()))
    }
    async fn cleanup(&self, record: &Value) -> Result<Value> {
        if record["binding"] != self.binding {
            return Err(Error::Forbidden);
        }
        let nonce =
            Uuid::parse_str(record["id"].as_str().ok_or_else(invalid)?).map_err(|_| invalid())?;
        if record["create_dispatch_admitted"] == 0 {
            // SQL proves that no dispatcher was ever admitted. This closes only
            // original ownership; no engine lookup, stop or removal is claimed.
            return self
                .store
                .android_private_container_close_unadmitted(nonce, &self.binding)
                .await;
        }
        let target = record["container_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("cowork-android-private-{nonce}"));
        let first = self.inspect(record, &target).await?;
        let mut physical = record["container_id"].as_str().map(str::to_owned);
        // A missing original create response leaves an outstanding engine request possible.
        // Current absence cannot turn that original unknown create into successful cleanup.
        if first.is_none() && physical.is_none() {
            return Err(unavailable());
        }
        let mut stop_witness = record["termination_witness"].clone();
        if let Some(first) = first {
            let id = first["Id"].as_str().ok_or_else(unavailable)?.to_owned();
            physical = Some(id.clone());
            if first["State"]["Running"] == true
                || first["State"]["Pid"] != 0
                || first["State"]["Paused"] == true
            {
                self.cli(vec!["kill".into(), "--signal=KILL".into(), id.clone()])
                    .await?;
            }
            let stopped = self.inspect(record, &id).await?.ok_or_else(unavailable)?;
            if stopped["State"]["Running"] != false
                || stopped["State"]["Paused"] != false
                || stopped["State"]["Restarting"] != false
                || stopped["State"]["Pid"] != 0
            {
                return Err(unavailable());
            }
            let observed = json!({"format":1,"engine_id":self.config.engine_id,"nonce":nonce,"original_id":id,"running":false,"paused":false,"restarting":false,"pid":0});
            self.store
                .android_private_container_terminated(nonce, &self.binding, &id, &observed)
                .await?;
            stop_witness = observed;
            self.cli(vec!["rm".into(), id.clone()]).await?;
            if self.inspect(record, &id).await?.is_some() {
                return Err(unavailable());
            }
        }
        if stop_witness["nonce"] != nonce.to_string()
            || stop_witness["engine_id"] != self.config.engine_id
            || stop_witness["original_id"] != json!(physical)
            || stop_witness["running"] != false
            || stop_witness["paused"] != false
            || stop_witness["restarting"] != false
            || stop_witness["pid"] != 0
        {
            return Err(unavailable());
        }
        let witness = json!({"format":1,"engine_id":self.config.engine_id,"nonce":nonce,"original_id":physical,"running":false,"paused":false,"restarting":false,"pid":0,"removed":true,"stop":stop_witness});
        self.store
            .android_private_container_removed(nonce, &self.binding, physical.as_deref(), &witness)
            .await?;
        Ok(witness)
    }
    pub(super) async fn recover(&self) -> Result<()> {
        for record in self
            .store
            .android_private_container_cleanup_records()
            .await?
        {
            if record["lease_active"] == true || record["binding"] != self.binding {
                continue;
            }
            if record["binding"] == self.binding {
                self.cleanup(&record).await?;
            }
        }
        self.store.android_private_browser_recover().await?;
        self.store
            .android_private_terminal_recover(self.keys.as_ref())
            .await?;
        self.store.android_private_github_recover().await?;
        Ok(())
    }
    pub(super) async fn start(
        self: &Arc<Self>,
        run: Uuid,
        lease: Uuid,
        invocation: &str,
        bootstrap: &Value,
        stop: &CancellationToken,
    ) -> Result<Container> {
        self.engine().await?;
        if stop.is_cancelled() {
            return Err(unavailable());
        }
        let admission: PrivateContainerAdmission = self
            .store
            .android_private_container_admit(run, lease, invocation, &self.binding)
            .await?;
        let mut record = match self
            .store
            .android_private_container_record(admission.id, &self.binding)
            .await
        {
            Ok(record) => record,
            Err(error) => {
                self.store
                    .android_private_container_close_unadmitted(admission.id, &self.binding)
                    .await?;
                return Err(error);
            }
        };
        let mut args = vec![
            "create".into(),
            "--pull=never".into(),
            "--name".into(),
            format!("cowork-android-private-{}", admission.id),
            "--runtime=runsc".into(),
            "--network=none".into(),
            "--read-only".into(),
            "--user=0:0".into(),
            "--cap-drop=ALL".into(),
            "--cap-add=SETUID".into(),
            "--cap-add=SETGID".into(),
            "--cap-add=KILL".into(),
            "--cap-add=DAC_OVERRIDE".into(),
            "--cap-add=CHOWN".into(),
            "--security-opt=no-new-privileges:true".into(),
            "--pids-limit=128".into(),
            "--memory=1g".into(),
            "--memory-swap=1g".into(),
            "--cpus=2".into(),
            "--tmpfs".into(),
            "/tmp:rw,nosuid,nodev,size=256m,mode=1777".into(),
            "--tmpfs".into(),
            "/workspace:rw,nosuid,nodev,size=256m,mode=0700,uid=1000,gid=1000".into(),
            "--workdir=/workspace".into(),
            "--interactive".into(),
            "--entrypoint=/opt/cowork/python/bin/python".into(),
        ];
        for (key, value) in self.labels(&record) {
            args.extend(["--label".into(), format!("{key}={value}")]);
        }
        args.extend([
            self.config.image.clone(),
            "-I".into(),
            "/opt/cowork-private-tools/guest.py".into(),
        ]);
        if stop.is_cancelled() {
            self.store
                .android_private_container_close_unadmitted(admission.id, &self.binding)
                .await?;
            return Err(unavailable());
        }
        let dispatch = match self
            .store
            .android_private_container_create_dispatch(
                run,
                lease,
                admission.id,
                &self.binding,
                self.keys.as_ref(),
                || !stop.is_cancelled(),
            )
            .await
        {
            Ok(permit) => permit,
            Err(error) => {
                self.store
                    .android_private_container_close_unadmitted(admission.id, &self.binding)
                    .await?;
                return Err(error);
            }
        };
        // Last SQL/crypto wait has ended. An unconsumed original permit can
        // prove no IO even when cancellation follows its committed admission.
        if stop.is_cancelled() {
            self.store
                .android_private_container_close_unconsumed(dispatch)
                .await?;
            return Err(unavailable());
        }
        record["create_dispatch_admitted"] = json!(1);
        record["original_dispatch_receipt"] = dispatch.original_receipt().clone();
        let bytes = self.create_cli(args, dispatch, stop).await?;
        let stdout = std::str::from_utf8(&bytes).map_err(|_| unavailable())?;
        let physical_id = stdout.trim().to_owned();
        self.store
            .android_private_container_created(run, lease, admission.id, stdout, &physical_id)
            .await?;
        record["container_id"] = json!(physical_id);
        self.inspect(&record, &physical_id)
            .await?
            .ok_or_else(unavailable)?;
        self.store
            .android_private_container_start_admit(
                run,
                lease,
                admission.id,
                &physical_id,
                &self.binding,
                self.keys.as_ref(),
            )
            .await?;
        if stop.is_cancelled() {
            self.cleanup(&record).await?;
            return Err(unavailable());
        }
        self.store
            .android_private_container_validate(
                run,
                lease,
                admission.id,
                &physical_id,
                &self.binding,
                self.keys.as_ref(),
            )
            .await?;
        if stop.is_cancelled() {
            self.cleanup(&record).await?;
            return Err(unavailable());
        }
        let mut child = self
            .command(&[
                "start".into(),
                "--attach".into(),
                "--interactive".into(),
                physical_id.clone(),
            ])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|_| unavailable())?;
        let stdin = child.stdin.take().ok_or_else(unavailable)?;
        let stdout = child.stdout.take().ok_or_else(unavailable)?;
        let stderr = child.stderr.take().ok_or_else(unavailable)?;
        let cancel = CancellationToken::new();
        let own_cancel = cancel.clone();
        let parent_stop = stop.clone();
        let runner = self.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut pulse = tokio::time::interval(Duration::from_secs(2));
            let deadline = tokio::time::sleep(Duration::from_secs(1800));
            tokio::pin!(deadline);
            loop {
                tokio::select! {_=own_cancel.cancelled()=>break,_=parent_stop.cancelled()=>break,_=&mut deadline=>break,_=pulse.tick()=>{if runner.store.android_private_container_live(run,lease,admission.id,runner.keys.as_ref()).await.is_err(){break}}}
            }
            let result = runner.cleanup(&record).await;
            let _ = tx.send(result);
        });
        let mut container = Container {
            nonce: admission.id,
            physical_id,
            child,
            stdin: Some(stdin),
            stdout: Some(stdout),
            stop: cancel,
            finished: Some(rx),
            stderr: Some(tokio::spawn(bounded(stderr, 65536))),
        };
        let mut payload = bootstrap.clone();
        payload["nonce"] = json!(admission.id);
        let raw = Zeroizing::new(serde_json::to_vec(&payload).map_err(|_| invalid())?);
        if raw.len() > 24 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        let write = container.stdin.as_mut().ok_or_else(unavailable)?;
        write.write_all(&raw).await.map_err(|_| unavailable())?;
        write.write_all(b"\n").await.map_err(|_| unavailable())?;
        write.flush().await.map_err(|_| unavailable())?;
        // Read the exact one-line launcher acknowledgement without consuming future MCP bytes.
        let stdout = container.stdout.as_mut().ok_or_else(unavailable)?;
        let mut ack = Vec::new();
        loop {
            let byte = tokio::time::timeout(Duration::from_secs(10), stdout.read_u8())
                .await
                .map_err(|_| unavailable())?
                .map_err(|_| unavailable())?;
            if byte == b'\n' {
                break;
            }
            if ack.len() > 1024 {
                return Err(unavailable());
            }
            ack.push(byte);
        }
        let value: Value = serde_json::from_slice(&ack).map_err(|_| unavailable())?;
        if value != json!({"format":1,"ready":true,"nonce":admission.id}) {
            return Err(unavailable());
        }
        self.store
            .android_private_container_started(
                run,
                lease,
                admission.id,
                &container.physical_id,
                &self.binding,
                &ack,
            )
            .await?;
        Ok(container)
    }
}
impl Container {
    pub(super) async fn finish(&mut self) -> Result<Value> {
        self.stop.cancel();
        let result = tokio::time::timeout(
            Duration::from_secs(90),
            self.finished.take().ok_or_else(unavailable)?,
        )
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())??;
        if tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
        }
        if let Some(task) = self.stderr.take() {
            let _ = task.await;
        }
        Ok(result)
    }
    pub(super) async fn program_reply(&mut self) -> Result<Value> {
        let stdout = self.stdout.take().ok_or_else(unavailable)?;
        self.stdin.take();
        let bytes = bounded(stdout, 32 * 1024 * 1024).await?;
        let status = self.child.wait().await.map_err(|_| unavailable())?;
        if !status.success() {
            return Err(unavailable());
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if value["format"] != 1 || value["nonce"] != self.nonce.to_string() {
            return Err(unavailable());
        }
        Ok(value)
    }
}
impl AndroidDeviceServices {
    pub(super) async fn private_program_tool(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        stop: &CancellationToken,
    ) -> Result<bool> {
        use cowork_storage::android_private_mcp_runtime::PrivateMcpGate;
        match self
            .store
            .android_private_program_gate(lease.run, lease.lease, self.keys.as_ref(), self.key_id)
            .await?
        {
            PrivateMcpGate::Waiting(_) => return Ok(false),
            PrivateMcpGate::Denied(_) => return Ok(true),
            PrivateMcpGate::Ready => {}
        }
        let runner = self.private_runner.as_ref().ok_or(Error::Forbidden)?;
        if lease.value["spec"]["input"]["frozen_private_runner"] != runner.binding() {
            return Err(Error::Forbidden);
        }
        let files = self
            .store
            .android_private_runner_files(lease.run, lease.lease, self.keys.as_ref())
            .await?;
        let permit = self
            .store
            .android_private_program_admit(lease.run, lease.lease, self.keys.as_ref(), self.key_id)
            .await?;
        let bootstrap = json!({"format":1,"mode":"program","files":files,"program":permit.program,"arguments":permit.arguments,"timeout_seconds":permit.timeout_seconds,"office_action":permit.office_action});
        let mut container = self
            .private_mcp_wait(
                lease.run,
                lease.lease,
                None,
                stop,
                runner.start(lease.run, lease.lease, &permit.invocation, &bootstrap, stop),
            )
            .await?;
        let received = {
            let observe = container.program_reply();
            tokio::pin!(observe);
            let mut pulse = tokio::time::interval(Duration::from_secs(3));
            let deadline = tokio::time::sleep(Duration::from_secs(permit.timeout_seconds + 30));
            tokio::pin!(deadline);
            loop {
                tokio::select! {biased;_=stop.cancelled()=>return Err(unavailable()),_=&mut deadline=>return Err(unavailable()),_=pulse.tick()=>{self.store.android_private_server_renew(lease.run,lease.lease).await?;self.store.android_private_loop_checkpoint(lease.run,lease.lease,self.keys.as_ref()).await?;},reply=&mut observe=>break reply?}
            }
        };
        self.store
            .android_private_program_retain_received(
                lease.run,
                lease.lease,
                permit.sequence,
                &permit.request_sha256,
                container.nonce,
                &received,
                self.keys.as_ref(),
                self.key_id,
            )
            .await?;
        let witness = container.finish().await?;
        self.store
            .android_private_program_received(
                lease.run,
                lease.lease,
                permit.sequence,
                &permit.request_sha256,
                container.nonce,
                &received,
                &witness,
                self.keys.as_ref(),
                self.key_id,
            )
            .await?;
        Ok(true)
    }
}
