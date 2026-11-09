//! Native desktop enrollment and continuous original Pod/CRI identities.
use super::*;
use cowork_contracts::KubernetesJobIdentity;

pub(crate) const CREATE_DOMAIN: &[u8] = b"open-cowork/runner-kubernetes-desktop-create/v2\0";
pub(crate) const PROBE_DOMAIN: &[u8] = b"open-cowork/runner-kubernetes-desktop-probe/v2\0";
pub(crate) const STOP_DOMAIN: &[u8] = b"open-cowork/runner-kubernetes-desktop-stop/v2\0";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeJobBinding {
    pub version: u8,
    pub runner_id: Uuid,
    pub engine_id: String,
    pub scope_prefix: String,
    pub reservation_id: Uuid,
    pub attempt_id: Uuid,
    pub container_id: String,
    pub container_name: String,
    pub created_at: String,
    pub started_at: String,
    pub run_id: Uuid,
    pub session_id: Uuid,
    pub lease_token: Uuid,
    pub workspace_id: Option<Uuid>,
    pub workspace_volume: String,
    pub kubernetes: KubernetesJobIdentity,
}
impl NativeJobBinding {
    pub(crate) fn digest(&self) -> Result<String> {
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(self)?)))
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeLiveJob {
    pub binding: NativeJobBinding,
    pub running: bool,
    pub paused: bool,
    pub restarting: bool,
    pub observed_resource_version: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeProbeRequest {
    pub version: u8,
    pub challenge: Uuid,
    pub binding: NativeJobBinding,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u8,
    spec: SandboxDesktopSessionSpec,
    original_uri: String,
    body_sha256: String,
    lease: Uuid,
    attempt: Uuid,
    reservation: Uuid,
    create_resource_version: Option<String>,
    binding: Option<NativeJobBinding>,
    stopped: bool,
}

impl Backend {
    fn desktop_path(&self, session: Uuid) -> PathBuf {
        self.directory.join(format!("desktop-{session}.json"))
    }
    fn intent(&self, session: Uuid) -> Result<Intent> {
        let path = self.desktop_path(session);
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            bail!("unsafe native desktop intent");
        }
        let value: Intent = serde_json::from_slice(&fs::read(path)?)?;
        if value.version != 2 || value.spec.session_id != session {
            bail!("original native desktop intent differs");
        }
        Ok(value)
    }
    fn save_intent(&self, value: &Intent) -> Result<()> {
        write(
            &self.desktop_path(value.spec.session_id),
            &serde_json::to_vec(value)?,
        )
    }
    pub(crate) fn desktop_enabled(&self) -> bool {
        self.desktop_profile.is_some()
    }
    async fn desktop_nodes(&self, pids: u32) -> Result<DesktopProfile> {
        let profile = self
            .desktop_profile
            .clone()
            .context("native desktop node profile is not configured")?;
        if profile.version != 1
            || profile.node_names.is_empty()
            || profile.node_names.len() > 1000
            || pids < 64
            || pids > 4096
            || profile.node_selector.get("cowork.role").map(String::as_str) != Some("executor")
            || profile.node_selector.get("cowork.trust-domain")
                != Some(&self.organisation.to_string())
            || profile.node_selector.get("cowork.host") != Some(&self.host.to_string())
        {
            bail!("native desktop original node profile differs");
        }
        dns(&profile.runtime_class)?;
        self.rpc(
            &["get", "runtimeclass", &profile.runtime_class, "-o", "json"],
            None,
        )
        .await?;
        for name in &profile.node_names {
            if name.len() > 253
                || !name.bytes().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'.')
                })
            {
                bail!("invalid original GUI node");
            }
            let node = self
                .object("node", name)
                .await?
                .context("original GUI node absent")?;
            if profile
                .node_uids
                .get(name)
                .is_none_or(|uid| node["metadata"]["uid"] != *uid)
                || profile
                    .node_selector
                    .iter()
                    .any(|(key, value)| node["metadata"]["labels"][key] != *value)
                || node["spec"]["unschedulable"] == true
                || !node["status"]["conditions"].as_array().is_some_and(|rows| {
                    rows.iter()
                        .any(|r| r["type"] == "Ready" && r["status"] == "True")
                })
            {
                bail!("original GUI node changed or is unavailable");
            }
            let uri = format!("/api/v1/nodes/{name}/proxy/configz");
            let actual: serde_json::Value =
                serde_json::from_slice(&self.rpc(&["get", "--raw", &uri], None).await?.stdout)?;
            if !actual["kubeletconfig"]["podPidsLimit"]
                .as_u64()
                .is_some_and(|value| value > 0 && value <= pids as u64)
            {
                bail!("measured GUI pod PID limit exceeds the original desktop limit");
            }
        }
        Ok(profile)
    }
    pub(crate) async fn create_desktop(
        &self,
        config: &Config,
        spec: &SandboxDesktopSessionSpec,
        lease: Uuid,
        attempt: Uuid,
        body: &[u8],
        uri: &str,
    ) -> Result<(SandboxDesktopSessionResult, NativeLiveJob)> {
        self.current().await?;
        self.open()?;
        if lease.is_nil() || attempt.is_nil() || spec.session_id.is_nil() || spec.run_id.is_nil() {
            bail!("original native desktop creation authority missing");
        }
        if spec.network != SandboxNetwork::FilteredEgress {
            bail!("native desktop requires the scoped filtered proxy");
        }
        if spec.limits.pids < 64
            || spec.limits.pids > 4096
            || spec.limits.cpu_nanos < 100_000_000
            || spec.limits.memory_bytes < 512 * 1024 * 1024
        {
            bail!("native desktop limits are below the supported bounded GUI profile");
        }
        let profile = self.desktop_nodes(spec.limits.pids.clamp(64, 4096)).await?;
        if !config.gui_image.contains("@sha256:") {
            bail!("native GUI image must be digest pinned");
        }
        let mut intent;
        let mut row;
        {
            let _guard = self.lock().await?;
            self.open()?;
            if self
                .directory
                .join(format!("stopped-{}", spec.run_id))
                .exists()
            {
                bail!("original run was stopped");
            }
            if self.desktop_path(spec.session_id).exists() {
                let existing = self.intent(spec.session_id)?;
                if existing.lease != lease
                    || existing.attempt != attempt
                    || existing.original_uri != uri
                    || existing.body_sha256 != hex::encode(Sha256::digest(body))
                    || existing.stopped
                {
                    bail!("original native desktop creation changed");
                }
                let binding = existing
                    .binding
                    .context("original native desktop create ACK is unsettled; no replay")?;
                return Ok((
                    SandboxDesktopSessionResult {
                        schema_version: SCHEMA_VERSION,
                        session_id: spec.session_id,
                        run_id: spec.run_id,
                        container_name: binding.container_name.clone(),
                        workspace_volume: binding.workspace_volume.clone(),
                        dimensions: spec.dimensions.clone(),
                    },
                    self.desktop_live(&binding).await?,
                ));
            }
            let resources = Resources {
                cpu_nanos: spec.limits.cpu_nanos.clamp(100_000_000, 16_000_000_000),
                memory_bytes: spec
                    .limits
                    .memory_bytes
                    .clamp(512 * 1024 * 1024, 32 * 1024 * 1024 * 1024),
            };
            let active: Vec<_> = self
                .records()?
                .into_iter()
                .filter(|r| !matches!(r.phase.as_str(), "completed" | "stopped"))
                .collect();
            let transfers = self
                .transfers()?
                .into_iter()
                .filter(|r| r.phase != "closed")
                .count() as u64;
            if active.len() + transfers as usize >= self.limits.containers
                || active
                    .iter()
                    .map(|r| r.run)
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    >= self.limits.concurrent_runs
                || active.iter().map(|r| r.resources.cpu_nanos).sum::<u64>()
                    + resources.cpu_nanos
                    + transfers * 100_000_000
                    > self.limits.cpu_nanos
                || active.iter().map(|r| r.resources.memory_bytes).sum::<u64>()
                    + resources.memory_bytes
                    + transfers * 64 * 1024 * 1024
                    > self.limits.memory_bytes
            {
                bail!("native desktop resource pool is reserved");
            }
            intent = Intent {
                version: 2,
                spec: spec.clone(),
                original_uri: uri.into(),
                body_sha256: hex::encode(Sha256::digest(body)),
                lease,
                attempt,
                reservation: Uuid::new_v4(),
                create_resource_version: None,
                binding: None,
                stopped: false,
            };
            self.save_intent(&intent)?;
            row = Dispatch {
                version: 1,
                run: spec.run_id,
                workspace: spec.workspace_id,
                attempt,
                lease,
                request_sha256: intent.body_sha256.clone(),
                resources,
                pod: format!("desktop-{}", spec.session_id.simple()),
                pod_uid: None,
                pvc: format!(
                    "ws-{}",
                    &hex::encode(Sha256::digest(
                        format!(
                            "{}:{}",
                            self.generation,
                            spec.workspace_id.unwrap_or(spec.run_id)
                        )
                        .as_bytes()
                    ))[..48]
                ),
                pvc_uid: None,
                phase: "desktop_prepared".into(),
                result: None,
                kind: Some("desktop".into()),
            };
            self.save(&row)?;
            self.ensure_workspace(&mut row).await?;
            let mut labels = self.labels();
            labels["cowork.network"] = json!("filtered");
            labels["cowork.run"] = json!(spec.run_id);
            labels["cowork.desktop"] = json!(spec.session_id);
            labels["cowork.attempt"] = json!(attempt);
            let proxy = config
                .filtered_egress_proxy
                .as_ref()
                .context("scoped desktop proxy absent")?;
            let manifest = json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":row.pod,"labels":labels,"finalizers":["cowork.opencowork.dev/physical-stop"]},"spec":{
                "runtimeClassName":profile.runtime_class,"nodeSelector":profile.node_selector,"restartPolicy":"Never","automountServiceAccountToken":false,
                "securityContext":{"runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,"fsGroup":10001,"seccompProfile":{"type":"RuntimeDefault"}},
                "containers":[{"name":"sandbox","image":config.gui_image,"args":["/bin/bash","-lc","while true; do sleep 3600; done"],
                    "env":[{"name":"COWORK_DESKTOP_SIZE","value":format!("{}x{}x24",spec.dimensions.width,spec.dimensions.height)},
                        {"name":"HTTP_PROXY","value":proxy},{"name":"HTTPS_PROXY","value":proxy},{"name":"http_proxy","value":proxy},{"name":"https_proxy","value":proxy},
                        {"name":"NO_PROXY","value":"localhost,127.0.0.1,::1"},{"name":"no_proxy","value":"localhost,127.0.0.1,::1"}],
                    "securityContext":{"readOnlyRootFilesystem":true,"allowPrivilegeEscalation":false,"capabilities":{"drop":["ALL"]}},
                    "resources":{"requests":{"cpu":format!("{}m",resources.cpu_nanos/1_000_000),"memory":resources.memory_bytes.to_string()},
                        "limits":{"cpu":format!("{}m",resources.cpu_nanos/1_000_000),"memory":resources.memory_bytes.to_string()}},
                    "volumeMounts":[{"name":"workspace","mountPath":"/workspace"},{"name":"tmp","mountPath":"/tmp"},{"name":"shm","mountPath":"/dev/shm"}]}],
                "volumes":[{"name":"workspace","persistentVolumeClaim":{"claimName":row.pvc}},{"name":"tmp","emptyDir":{"medium":"Memory","sizeLimit":spec.limits.tmpfs_bytes.clamp(64*1024*1024,4*1024*1024*1024).to_string()}},
                    {"name":"shm","emptyDir":{"medium":"Memory","sizeLimit":"1Gi"}}]}});
            row.phase = "desktop_dispatched".into();
            self.save(&row)?;
            let created: serde_json::Value = serde_json::from_slice(
                &self
                    .rpc(
                        &["create", "-f", "-", "-o", "json"],
                        Some(serde_json::to_vec(&manifest)?),
                    )
                    .await?
                    .stdout,
            )?;
            let uid = created["metadata"]["uid"]
                .as_str()
                .context("actual native desktop create UID absent")?;
            self.owned(&created, uid)?;
            row.pod_uid = Some(uid.into());
            row.phase = "desktop_created".into();
            self.save(&row)?;
            intent.create_resource_version = Some(
                created["metadata"]["resourceVersion"]
                    .as_str()
                    .context("actual native desktop create RV absent")?
                    .into(),
            );
            self.save_intent(&intent)?;
        }
        for _ in 0..300 {
            self.current().await?;
            self.open()?;
            let actual = self
                .object("pod", &row.pod)
                .await?
                .context("original native desktop pod absent")?;
            self.owned(&actual, row.pod_uid.as_deref().unwrap())?;
            if actual["status"]["phase"] == "Running" {
                let probe=self.rpc(&["exec",&row.pod,"--container=sandbox","--","python3","-c","import socket; s=socket.create_connection(('127.0.0.1',5900),2); assert s.recv(12).startswith(b'RFB '); s.close()"],None).await;
                if probe.is_ok() {
                    let binding = self.make_desktop_binding(&intent, &row, &actual).await?;
                    intent.binding = Some(binding.clone());
                    self.save_intent(&intent)?;
                    row.phase = "desktop_active".into();
                    self.save(&row)?;
                    let live = self.desktop_live(&binding).await?;
                    return Ok((
                        SandboxDesktopSessionResult {
                            schema_version: SCHEMA_VERSION,
                            session_id: spec.session_id,
                            run_id: spec.run_id,
                            container_name: row.pod,
                            workspace_volume: row.pvc,
                            dimensions: spec.dimensions.clone(),
                        },
                        live,
                    ));
                }
            }
            if matches!(
                actual["status"]["phase"].as_str(),
                Some("Succeeded" | "Failed")
            ) {
                bail!("original native desktop ended before VNC enrollment");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        bail!("original native desktop VNC enrollment remains unsettled")
    }
    async fn make_desktop_binding(
        &self,
        intent: &Intent,
        row: &Dispatch,
        pod: &serde_json::Value,
    ) -> Result<NativeJobBinding> {
        let node_name = pod["spec"]["nodeName"]
            .as_str()
            .context("actual native desktop node absent")?;
        let node = self
            .object("node", node_name)
            .await?
            .context("actual native desktop node UID absent")?;
        let profile = self
            .desktop_profile
            .as_ref()
            .context("original desktop node profile absent")?;
        if profile
            .node_uids
            .get(node_name)
            .is_none_or(|uid| node["metadata"]["uid"] != *uid)
        {
            bail!("actual desktop node UID differs");
        }
        let status = pod["status"]["containerStatuses"]
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["name"] == "sandbox"))
            .context("actual native desktop container status absent")?;
        if status["restartCount"] != 0 || !status["state"]["running"].is_object() {
            bail!("original native desktop container restarted");
        }
        let uri = format!("/api/v1/nodes/{node_name}/proxy/configz");
        let configuration: serde_json::Value =
            serde_json::from_slice(&self.rpc(&["get", "--raw", &uri], None).await?.stdout)?;
        let pids = configuration["kubeletconfig"]["podPidsLimit"]
            .as_u64()
            .context("actual desktop PID limit absent")?;
        if pids == 0 || pids > intent.spec.limits.pids.clamp(64, 4096) as u64 {
            bail!("actual desktop PID limit changed");
        }
        let native = KubernetesJobIdentity {
            version: 1,
            cluster_uid: Uuid::parse_str(&self.cluster_uid)?,
            namespace_uid: Uuid::parse_str(&self.namespace_uid)?,
            organisation_id: self.organisation,
            instance_id: self.instance,
            generation: self.generation,
            node_name: node_name.into(),
            node_uid: Uuid::parse_str(node["metadata"]["uid"].as_str().unwrap())?,
            pod_uid: Uuid::parse_str(
                row.pod_uid
                    .as_deref()
                    .context("original desktop pod UID absent")?,
            )?,
            pod_resource_version: intent
                .create_resource_version
                .clone()
                .context("original desktop create RV absent")?,
            pvc_uid: Uuid::parse_str(
                row.pvc_uid
                    .as_deref()
                    .context("original desktop PVC UID absent")?,
            )?,
            container_runtime_id: status["containerID"]
                .as_str()
                .context("actual desktop CRI container identity absent")?
                .into(),
            pod_pids_limit: pids as u32,
        };
        native.validate().map_err(|e| anyhow::anyhow!(e))?;
        Ok(NativeJobBinding {
            version: 2,
            runner_id: self.runner_id,
            engine_id: String::new(),
            scope_prefix: format!("kubernetes-{}", self.instance.simple()),
            reservation_id: intent.reservation,
            attempt_id: intent.attempt,
            container_id: String::new(),
            container_name: row.pod.clone(),
            created_at: pod["metadata"]["creationTimestamp"]
                .as_str()
                .context("actual desktop creation timestamp absent")?
                .into(),
            started_at: status["state"]["running"]["startedAt"]
                .as_str()
                .context("actual desktop start timestamp absent")?
                .into(),
            run_id: row.run,
            session_id: intent.spec.session_id,
            lease_token: intent.lease,
            workspace_id: row.workspace,
            workspace_volume: row.pvc.clone(),
            kubernetes: native,
        })
    }
    pub(crate) async fn desktop_live(&self, binding: &NativeJobBinding) -> Result<NativeLiveJob> {
        self.current().await?;
        self.open()?;
        let intent = self.intent(binding.session_id)?;
        if intent.stopped
            || intent.binding.as_ref() != Some(binding)
            || binding.runner_id != self.runner_id
        {
            bail!("original native desktop binding changed or stopped");
        }
        self.desktop_nodes(intent.spec.limits.pids.clamp(64, 4096))
            .await?;
        let row = self
            .records()?
            .into_iter()
            .find(|r| r.attempt == intent.attempt)
            .context("original desktop reservation absent")?;
        let actual = self
            .object("pod", &binding.container_name)
            .await?
            .context("original native desktop pod absent")?;
        self.owned(&actual, &binding.kubernetes.pod_uid.to_string())?;
        if actual["status"]["phase"] != "Running"
            || !actual["metadata"]["deletionTimestamp"].is_null()
            || self.make_desktop_binding(&intent, &row, &actual).await? != *binding
        {
            bail!("original native desktop physical identity changed");
        }
        let pvc = self
            .object("pvc", &binding.workspace_volume)
            .await?
            .context("original desktop PVC absent")?;
        self.owned(&pvc, &binding.kubernetes.pvc_uid.to_string())?;
        Ok(NativeLiveJob {
            binding: binding.clone(),
            running: true,
            paused: false,
            restarting: false,
            observed_resource_version: Some(
                actual["metadata"]["resourceVersion"]
                    .as_str()
                    .context("fresh desktop resourceVersion absent")?
                    .into(),
            ),
        })
    }
    pub(crate) async fn probe_desktop(
        &self,
        request: &NativeProbeRequest,
    ) -> Result<serde_json::Value> {
        if request.version != 2 || request.challenge.is_nil() {
            bail!("invalid original native desktop probe challenge");
        }
        Ok(
            json!({"version":2,"challenge":request.challenge,"job":self.desktop_live(&request.binding).await?}),
        )
    }
    pub(crate) async fn stop_desktop(
        &self,
        session: Uuid,
        digest: &str,
    ) -> Result<serde_json::Value> {
        self.current().await?;
        let _guard = self.lock().await?;
        let mut intent = self.intent(session)?;
        let binding = intent
            .binding
            .clone()
            .context("original native desktop enrollment ACK absent")?;
        if binding.digest()? != digest {
            bail!("original native desktop stop binding differs");
        }
        let mut row = self
            .records()?
            .into_iter()
            .find(|r| r.attempt == intent.attempt)
            .context("original native desktop reservation absent")?;
        if !intent.stopped {
            intent.stopped = true;
            self.save_intent(&intent)?;
        }
        if row.phase != "stopped" {
            self.delete_original_pod(
                &binding.container_name,
                &binding.kubernetes.pod_uid.to_string(),
            )
            .await?;
            row.phase = "stopped".into();
            self.save(&row)?;
        }
        let physical: serde_json::Value = serde_json::from_slice(&fs::read(
            self.directory
                .join(format!("physical-stop-{}", binding.kubernetes.pod_uid)),
        )?)?;
        if physical["pod_uid"] != binding.kubernetes.pod_uid.to_string() {
            bail!("original desktop physical termination receipt differs");
        }
        Ok(json!({"version":2,"binding":binding,"execution_stopped":true}))
    }
    pub(crate) async fn desktop_binding(
        &self,
        session: Uuid,
        digest: &str,
    ) -> Result<NativeJobBinding> {
        let intent = self.intent(session)?;
        let binding = intent.binding.context("original desktop binding absent")?;
        if binding.digest()? != digest {
            bail!("original native desktop stream binding differs");
        }
        self.desktop_live(&binding).await?;
        Ok(binding)
    }
    pub(crate) async fn execute_desktop_job(
        &self,
        spec: &SandboxRunSpec,
        execution: Option<&admission::Execution>,
        body: &[u8],
    ) -> Result<SandboxRunResult> {
        self.current().await?;
        self.open()?;
        let execution = execution.context("native GUI tool requires original execution attempt")?;
        let mut found = None;
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("desktop-") || !name.ends_with(".json") {
                continue;
            }
            let intent: Intent = serde_json::from_slice(&fs::read(path)?)?;
            if intent.spec.run_id == spec.run_id
                && intent.spec.workspace_id == spec.workspace_id
                && !intent.stopped
            {
                if found.is_some() {
                    bail!("multiple original desktop owners require reconciliation");
                }
                found = Some(intent);
            }
        }
        let intent = found.context("native GUI execution requires original desktop enrollment")?;
        let binding = intent
            .binding
            .context("original native desktop enrollment ACK absent")?;
        self.desktop_live(&binding).await?;
        if execution.version != 1
            || execution.lease_token != binding.lease_token
            || execution.attempt_id.is_nil()
        {
            bail!("native GUI original lease changed");
        }
        if spec.limits.cpu_nanos
            > intent
                .spec
                .limits
                .cpu_nanos
                .clamp(100_000_000, 16_000_000_000)
            || spec.limits.memory_bytes
                > intent
                    .spec
                    .limits
                    .memory_bytes
                    .clamp(512 * 1024 * 1024, 32 * 1024 * 1024 * 1024)
        {
            bail!("native GUI tool exceeds its original desktop pool");
        }
        let mut row;
        {
            let _guard = self.lock().await?;
            self.open()?;
            if self
                .directory
                .join(format!("stopped-{}", spec.run_id))
                .exists()
            {
                bail!("original run stopped");
            }
            if let Some(prior) = self
                .records()?
                .into_iter()
                .find(|r| r.attempt == execution.attempt_id)
            {
                if prior.run != spec.run_id
                    || prior.workspace != spec.workspace_id
                    || prior.lease != execution.lease_token
                    || prior.request_sha256 != hex::encode(Sha256::digest(body))
                    || prior.kind.as_deref() != Some("desktop_exec")
                {
                    bail!("original native GUI execution body changed");
                }
                return prior
                    .result
                    .context("original native GUI exec is unsettled; no replay");
            }
            row = Dispatch {
                version: 1,
                run: spec.run_id,
                workspace: spec.workspace_id,
                attempt: execution.attempt_id,
                lease: execution.lease_token,
                request_sha256: hex::encode(Sha256::digest(body)),
                resources: Resources {
                    cpu_nanos: 0,
                    memory_bytes: 0,
                },
                pod: binding.container_name.clone(),
                pod_uid: Some(binding.kubernetes.pod_uid.to_string()),
                pvc: binding.workspace_volume.clone(),
                pvc_uid: Some(binding.kubernetes.pvc_uid.to_string()),
                phase: "gui_exec_dispatched".into(),
                result: None,
                kind: Some("desktop_exec".into()),
            };
            self.save(&row)?;
        }
        self.desktop_live(&binding).await?;
        let script="import sys,json,subprocess,base64,resource,os,selectors,time,tempfile; p=json.load(sys.stdin); resource.setrlimit(resource.RLIMIT_NPROC,(p['pids'],p['pids'])); os.chdir('/workspace'); e={**os.environ,**p['env']}; f=tempfile.TemporaryFile(); f.write(base64.b64decode(p['stdin'])); f.seek(0); c=subprocess.Popen(p['argv'],stdin=f,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=e,start_new_session=True); s=selectors.DefaultSelector(); s.register(c.stdout,selectors.EVENT_READ,'out'); s.register(c.stderr,selectors.EVENT_READ,'err'); b={'out':bytearray(),'err':bytearray()}; trunc=False; deadline=time.monotonic()+p['timeout']; timed=False\nwhile s.get_map():\n if time.monotonic()>deadline and not timed:\n  timed=True\n  try: os.killpg(c.pid,9)\n  except ProcessLookupError: pass\n for k,_ in s.select(.1):\n  x=os.read(k.fileobj.fileno(),65536)\n  if not x: s.unregister(k.fileobj); continue\n  room=max(0,p['max']-len(b[k.data])); b[k.data].extend(x[:room]); trunc=trunc or len(x)>room\nc.wait()\ntry: os.killpg(c.pid,9)\nexcept ProcessLookupError: pass\nprint(json.dumps({'exit':c.returncode,'timed_out':timed,'stdout':b['out'].decode('utf8','replace'),'stderr':b['err'].decode('utf8','replace'),'truncated':trunc}))";
        let payload = serde_json::to_vec(
            &json!({"argv":spec.argv,"env":spec.environment,"stdin":spec.stdin_base64.as_deref().unwrap_or(""),
            "pids":spec.limits.pids,"timeout":spec.limits.timeout_seconds,"max":spec.limits.output_bytes}),
        )?;
        let output = self
            .rpc_exec(&row, script, payload, spec.limits.timeout_seconds + 15)
            .await?;
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let result = SandboxRunResult {
            schema_version: SCHEMA_VERSION,
            run_id: spec.run_id,
            container_name: binding.container_name,
            workspace_volume: binding.workspace_volume,
            exit_code: value["exit"].as_i64().map(|v| v as i32),
            timed_out: value["timed_out"]
                .as_bool()
                .context("actual native GUI timeout receipt absent")?,
            stdout: value["stdout"]
                .as_str()
                .context("actual native GUI stdout receipt absent")?
                .into(),
            stderr: value["stderr"]
                .as_str()
                .context("actual native GUI stderr receipt absent")?
                .into(),
            output_truncated: value["truncated"]
                .as_bool()
                .context("actual native GUI output bound receipt absent")?,
        };
        let _guard = self.lock().await?;
        row.result = Some(result.clone());
        row.phase = "completed".into();
        self.save(&row)?;
        Ok(result)
    }
    pub(crate) async fn terminal(
        self: Arc<Self>,
        config: Arc<Config>,
        mut socket: WebSocket,
        run: Uuid,
        workspace: Option<Uuid>,
        request: Uuid,
        uri: String,
        columns: u16,
        rows: u16,
    ) -> Result<()> {
        self.current().await?;
        self.node_limits().await?;
        self.open()?;
        let pvc = format!(
            "ws-{}",
            &hex::encode(Sha256::digest(
                format!("{}:{}", self.generation, workspace.unwrap_or(run)).as_bytes()
            ))[..48]
        );
        let receipt = self
            .object("configmap", &format!("receipt-{pvc}"))
            .await?
            .context("original terminal workspace receipt absent")?;
        if receipt["immutable"] != true {
            bail!("original terminal workspace receipt mutable");
        }
        self.owned(
            &receipt,
            receipt["metadata"]["uid"]
                .as_str()
                .context("original workspace receipt UID absent")?,
        )?;
        let original: serde_json::Value = serde_json::from_str(
            receipt["data"]["receipt"]
                .as_str()
                .context("original workspace receipt absent")?,
        )?;
        let pvc_uid = original["uid"]
            .as_str()
            .context("original terminal PVC UID absent")?;
        if original["name"] != pvc {
            bail!("original terminal workspace changed");
        }
        self.owned(
            &self
                .object("pvc", &pvc)
                .await?
                .context("original terminal workspace absent")?,
            pvc_uid,
        )?;
        let mut transfer = Transfer {
            request_id: request,
            original_uri: uri,
            run_id: run,
            workspace_id: workspace,
            pvc: pvc.clone(),
            pvc_uid: pvc_uid.into(),
            pod: format!("terminal-{}", request.simple()),
            pod_uid: None,
            phase: "prepared".into(),
        };
        let journal = self.directory.join(format!("transfer-{request}.json"));
        {
            let _guard = self.lock().await?;
            self.open()?;
            if journal.exists() || self.directory.join(format!("stopped-{run}")).exists() {
                bail!("original terminal already admitted or run stopped");
            }
            let active: Vec<_> = self
                .records()?
                .into_iter()
                .filter(|r| !matches!(r.phase.as_str(), "completed" | "stopped"))
                .collect();
            let helpers = self
                .transfers()?
                .into_iter()
                .filter(|r| r.phase != "closed")
                .count() as u64;
            if active.len() + helpers as usize >= self.limits.containers
                || active.iter().map(|r| r.resources.cpu_nanos).sum::<u64>()
                    + (helpers + 1) * 100_000_000
                    > self.limits.cpu_nanos
                || active.iter().map(|r| r.resources.memory_bytes).sum::<u64>()
                    + (helpers + 1) * 64 * 1024 * 1024
                    > self.limits.memory_bytes
            {
                bail!("native terminal resource pool reserved");
            }
            write(&journal, &serde_json::to_vec(&transfer)?)?;
            let mut labels = self.labels();
            labels["cowork.network"] = json!("none");
            labels["cowork.run"] = json!(run);
            let manifest = json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":transfer.pod,"labels":labels,"finalizers":["cowork.opencowork.dev/physical-stop"]},"spec":{
                "runtimeClassName":self.runtime_class,"nodeSelector":self.node_selector,"restartPolicy":"Never","automountServiceAccountToken":false,"activeDeadlineSeconds":3600,
                "securityContext":{"runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,"fsGroup":10001,"seccompProfile":{"type":"RuntimeDefault"}},
                "containers":[{"name":"sandbox","image":config.core_image,"command":["python3","-c","import time;time.sleep(3600)"],
                    "securityContext":{"readOnlyRootFilesystem":true,"allowPrivilegeEscalation":false,"capabilities":{"drop":["ALL"]}},
                    "resources":{"requests":{"cpu":"100m","memory":"64Mi"},"limits":{"cpu":"100m","memory":"64Mi"}},
                    "volumeMounts":[{"name":"workspace","mountPath":"/workspace"},{"name":"tmp","mountPath":"/tmp"}]}],
                "volumes":[{"name":"workspace","persistentVolumeClaim":{"claimName":pvc}},{"name":"tmp","emptyDir":{"medium":"Memory","sizeLimit":"32Mi"}}]}});
            transfer.phase = "dispatched".into();
            write(&journal, &serde_json::to_vec(&transfer)?)?;
            let actual: serde_json::Value = serde_json::from_slice(
                &self
                    .rpc(
                        &["create", "-f", "-", "-o", "json"],
                        Some(serde_json::to_vec(&manifest)?),
                    )
                    .await?
                    .stdout,
            )?;
            let uid = actual["metadata"]["uid"]
                .as_str()
                .context("original terminal pod create UID absent")?;
            self.owned(&actual, uid)?;
            transfer.pod_uid = Some(uid.into());
            transfer.phase = "created".into();
            write(&journal, &serde_json::to_vec(&transfer)?)?;
        }
        let uid = transfer.pod_uid.as_deref().unwrap();
        for _ in 0..240 {
            self.current().await?;
            self.open()?;
            let actual = self
                .object("pod", &transfer.pod)
                .await?
                .context("original terminal pod absent")?;
            self.owned(&actual, uid)?;
            if actual["status"]["phase"] == "Running" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        transfer.phase = "exec_dispatched".into();
        write(&journal, &serde_json::to_vec(&transfer)?)?;
        let script=format!("import os,pty,fcntl,termios,struct,select,sys,signal;os.chdir('/workspace');pid,fd=pty.fork()\nif pid==0: os.execv('/bin/bash',['bash','-l'])\nfcntl.ioctl(fd,termios.TIOCSWINSZ,struct.pack('HHHH',{rows},{columns},0,0))\ntry:\n while True:\n  ready,_,_=select.select([fd,sys.stdin.buffer],[],[],1)\n  if fd in ready:\n   b=os.read(fd,65536)\n   if not b:break\n   sys.stdout.buffer.write(b);sys.stdout.buffer.flush()\n  if sys.stdin.buffer in ready:\n   b=os.read(sys.stdin.fileno(),65536)\n   if not b:break\n   os.write(fd,b)\nfinally:\n try:os.kill(pid,signal.SIGKILL)\n except ProcessLookupError:pass\n os.close(fd)");
        let mut child = self
            .kubectl()
            .args([
                "--namespace",
                &self.namespace,
                "exec",
                "-i",
                &transfer.pod,
                "--container=sandbox",
                "--",
                "python3",
                "-c",
                &script,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut input = child.stdin.take().context("native terminal stdin absent")?;
        let mut output = child
            .stdout
            .take()
            .context("native terminal stdout absent")?;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut pulse = tokio::time::interval(Duration::from_secs(1));
        let outcome:Result<()>=async{loop{tokio::select!{
            _=pulse.tick()=>{self.current().await?;self.open()?;let actual=self.object("pod",&transfer.pod).await?.context("original terminal pod absent")?;self.owned(&actual,uid)?;},
            frame=socket.recv()=>{match frame{Some(Ok(Message::Binary(b)))=>{if b.len()>1024*1024{bail!("native terminal frame exceeds bound");}input.write_all(&b).await?;},Some(Ok(Message::Text(t)))=>{if t.len()>1024*1024{bail!("native terminal input exceeds bound");}input.write_all(t.as_bytes()).await?;},Some(Ok(Message::Ping(b)))=>socket.send(Message::Pong(b)).await?,Some(Ok(Message::Close(_)))|None=>break,Some(Err(e))=>return Err(e.into()),_=>{}}},
            bytes=output.read(&mut buffer)=>{let size=bytes?;if size==0{break;}socket.send(Message::Binary(buffer[..size].to_vec().into())).await?;},
        }}Ok(())}.await;
        let _ = child.kill().await;
        let _ = child.wait().await;
        self.delete_original_pod(&transfer.pod, uid).await?;
        transfer.phase = "closed".into();
        write(&journal, &serde_json::to_vec(&transfer)?)?;
        outcome
    }
    pub(crate) async fn relay_desktop(
        self: Arc<Self>,
        mut socket: WebSocket,
        binding: NativeJobBinding,
        control: bool,
    ) -> Result<()> {
        self.desktop_live(&binding).await?;
        let target = format!("TCP:127.0.0.1:{}", if control { 5900 } else { 5901 });
        let mut child = self
            .kubectl()
            .args([
                "--namespace",
                &self.namespace,
                "exec",
                "-i",
                &binding.container_name,
                "--container=sandbox",
                "--",
                "socat",
                "STDIO",
                &target,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut input = child.stdin.take().context("native VNC input unavailable")?;
        let mut output = child
            .stdout
            .take()
            .context("native VNC output unavailable")?;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut pulse = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _=pulse.tick()=>{self.desktop_live(&binding).await?;},
                frame=socket.recv()=>{match frame{Some(Ok(Message::Binary(bytes)))=>{if bytes.len()>1024*1024{bail!("native VNC frame exceeds bound");}input.write_all(&bytes).await?;},Some(Ok(Message::Ping(bytes)))=>socket.send(Message::Pong(bytes)).await?,Some(Ok(Message::Close(_)))|None=>break,Some(Err(e))=>return Err(e.into()),_=>{}}},
                read=output.read(&mut buffer)=>{let size=read?;if size==0{break;}socket.send(Message::Binary(buffer[..size].to_vec().into())).await?;},
            }
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
        Ok(())
    }
}
