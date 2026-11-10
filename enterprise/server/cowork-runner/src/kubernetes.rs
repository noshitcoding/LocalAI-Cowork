//! Native Kubernetes execution. Original dispatch journals survive lost RPCs;
//! a name is never enough to adopt or delete a pod or persistent volume claim.
use crate::*;
use fs2::FileExt;
use sha2::Digest;
use std::path::{Path as FsPath, PathBuf};
use uuid::Uuid;
#[path = "kubernetes_desktop.rs"]
mod desktop;
pub(crate) use desktop::NativeProbeRequest;
pub(crate) use desktop::{CREATE_DOMAIN, PROBE_DOMAIN, STOP_DOMAIN};

pub(crate) struct Backend {
    runner_id: Uuid,
    namespace: String,
    namespace_uid: String,
    cluster_uid: String,
    instance: Uuid,
    organisation: Uuid,
    host: Uuid,
    node_selector: std::collections::BTreeMap<String, String>,
    node_names: Vec<String>,
    node_uids: std::collections::BTreeMap<String, String>,
    desktop_profile: Option<DesktopProfile>,
    generation: Uuid,
    runtime_class: String,
    storage_class: String,
    storage_gib: u64,
    directory: PathBuf,
    limits: admission::Limits,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dispatch {
    version: u8,
    run: Uuid,
    workspace: Option<Uuid>,
    attempt: Uuid,
    lease: Uuid,
    request_sha256: String,
    resources: Resources,
    pod: String,
    pod_uid: Option<String>,
    pvc: String,
    pvc_uid: Option<String>,
    phase: String,
    result: Option<SandboxRunResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesktopProfile {
    version: u8,
    runtime_class: String,
    node_selector: std::collections::BTreeMap<String, String>,
    node_names: Vec<String>,
    node_uids: std::collections::BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transfer {
    request_id: Uuid,
    original_uri: String,
    run_id: Uuid,
    workspace_id: Option<Uuid>,
    pvc: String,
    pvc_uid: String,
    pod: String,
    pod_uid: Option<String>,
    phase: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScopeRequest {
    pub version: u8,
    pub operation_id: Uuid,
    pub instance_id: Uuid,
    pub organisation_id: Uuid,
    pub namespace_uid: Uuid,
    pub cluster_uid: Uuid,
    pub generation: Uuid,
    pub expected_quiesce_operation_id: Option<Uuid>,
}

impl Backend {
    pub(crate) fn inspect_journal(directory: &FsPath) -> Result<serde_json::Value> {
        if !directory.is_absolute() || fs::symlink_metadata(directory)?.file_type().is_symlink() {
            bail!("unsafe original Kubernetes ledger directory");
        }
        let scope: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("kubernetes-scope"))?)?;
        let mut entries = Vec::new();
        let mut desktops = Vec::new();
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".json") {
                continue;
            }
            if fs::symlink_metadata(&path)?.file_type().is_symlink() {
                bail!("unsafe original native journal entry");
            }
            if name.starts_with("kube-") {
                let row: Dispatch = serde_json::from_slice(&fs::read(path)?)?;
                entries.push(json!({"kind":row.kind.as_deref().unwrap_or("job"),"run_id":row.run,"attempt_id":row.attempt,
                    "pod_uid":row.pod_uid,"pod_name":row.pod,"pvc_uid":row.pvc_uid,"resources":row.resources,"phase":row.phase}));
            } else if name.starts_with("transfer-") {
                let row: Transfer = serde_json::from_slice(&fs::read(path)?)?;
                entries.push(json!({"kind":"transfer","request_id":row.request_id,"run_id":row.run_id,"pod_uid":row.pod_uid,
                    "pod_name":row.pod,"pvc_uid":row.pvc_uid,"resources":{"cpu_nanos":100_000_000,"memory_bytes":64*1024*1024},"phase":row.phase}));
            } else if name.starts_with("desktop-") {
                let value: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
                if let Some(binding) = value.get("binding").filter(|v| !v.is_null()) {
                    desktops.push(json!({"pod_uid":binding["kubernetes"]["pod_uid"],"pod_pids_limit":binding["kubernetes"]["pod_pids_limit"],
                        "original_pids":value["spec"]["limits"]["pids"],"stopped":value["stopped"]}));
                }
            }
        }
        Ok(json!({"version":1,"scope":scope,"entries":entries,"desktop_pids":desktops}))
    }
    pub(crate) async fn configured() -> Result<Option<Arc<Self>>> {
        let kind = env::var("COWORK_RUNNER_BACKEND").unwrap_or_else(|_| "docker".into());
        if kind == "docker" {
            return Ok(None);
        }
        if kind != "kubernetes" {
            bail!("unsupported runner backend");
        }
        let required = |key: &str| env::var(key).with_context(|| format!("missing {key}"));
        let instance = Uuid::parse_str(&required("COWORK_RUNNER_INSTANCE_ID")?)?;
        let organisation = Uuid::parse_str(&required("COWORK_RUNNER_ORGANISATION_ID")?)?;
        let generation = Uuid::parse_str(&required("COWORK_RUNNER_STORAGE_GENERATION")?)?;
        if instance.is_nil() || organisation.is_nil() || generation.is_nil() {
            bail!("nil Kubernetes scope");
        }
        let namespace = format!("cowork-{}", instance.simple());
        if required("COWORK_KUBE_NAMESPACE")? != namespace {
            bail!("Kubernetes namespace scope differs");
        }
        let mut value = Self {
            runner_id: Uuid::nil(),
            namespace,
            namespace_uid: required("COWORK_KUBE_NAMESPACE_UID")?,
            cluster_uid: required("COWORK_KUBE_CLUSTER_UID")?,
            instance,
            organisation,
            host: Uuid::parse_str(&required("COWORK_KUBE_EXECUTOR_HOST_ID")?)?,
            node_selector: serde_json::from_str(&required("COWORK_KUBE_NODE_SELECTOR")?)?,
            node_names: serde_json::from_str(&required("COWORK_KUBE_NODE_NAMES")?)?,
            node_uids: serde_json::from_str(&required("COWORK_KUBE_NODE_UIDS")?)?,
            desktop_profile: env::var("COWORK_KUBE_DESKTOP_PROFILE")
                .ok()
                .map(|v| serde_json::from_str(&v))
                .transpose()?,
            generation,
            runtime_class: dns(&required("COWORK_KUBE_RUNTIME_CLASS")?)?,
            storage_class: dns(&required("COWORK_KUBE_STORAGE_CLASS")?)?,
            storage_gib: required("COWORK_KUBE_WORKSPACE_CLAIM_GIB")?.parse()?,
            directory: PathBuf::from(required("COWORK_RUNNER_ADMISSION_DIRECTORY")?),
            limits: admission::Limits::from_env()?,
        };
        Uuid::parse_str(&value.namespace_uid)?;
        Uuid::parse_str(&value.cluster_uid)?;
        if value.node_selector.get("cowork.role").map(String::as_str) != Some("executor")
            || value.node_selector.get("cowork.trust-domain")
                != Some(&value.organisation.to_string())
            || value.node_selector.get("cowork.host") != Some(&value.host.to_string())
        {
            bail!("Kubernetes runner node authority differs");
        }
        if value.storage_gib == 0 || value.storage_gib > 1_048_576 {
            bail!("invalid Kubernetes storage cap");
        }
        fs::create_dir_all(&value.directory)?;
        if value.directory.join(".kube").exists() {
            bail!("native runner journal must not contain an ambient kubectl configuration");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&value.directory, fs::Permissions::from_mode(0o700))?;
        }
        let _guard = value.lock().await?;
        let scope = json!({"namespace":value.namespace,"namespace_uid":value.namespace_uid,"cluster_uid":value.cluster_uid,"instance":instance,"organisation":organisation,"generation":generation,
            "runtime_class":value.runtime_class,"storage_class":value.storage_class,"node_selector":value.node_selector,"node_uids":value.node_uids,"desktop_profile":value.desktop_profile});
        let path = value.directory.join("kubernetes-scope");
        if path.exists() {
            if serde_json::from_slice::<serde_json::Value>(&fs::read(path)?)? != scope {
                bail!("Kubernetes journal scope changed");
            }
        } else {
            write(&path, &serde_json::to_vec(&scope)?)?;
        }
        let identity = value.directory.join("kubernetes-runner-identity");
        if identity.exists() {
            value.runner_id = Uuid::parse_str(fs::read_to_string(identity)?.trim())?;
            if value.runner_id.is_nil() {
                bail!("invalid original runner identity");
            }
        } else {
            if !value.records()?.is_empty() || !value.transfers()?.is_empty() {
                bail!("original runner identity missing; no replacement identity");
            }
            value.runner_id = Uuid::new_v4();
            write(&identity, value.runner_id.to_string().as_bytes())?;
        }
        value.current().await?;
        value.node_limits().await?;
        // The runtime class is operator-owned and must exist; no Docker fallback.
        value
            .rpc(
                &["get", "runtimeclass", &value.runtime_class, "-o", "json"],
                None,
            )
            .await?;
        drop(_guard);
        Ok(Some(Arc::new(value)))
    }

    async fn lock(&self) -> Result<fs::File> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.directory.join("kubernetes-lock"))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(file),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(25)).await
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn kubectl(&self) -> Command {
        let mut command = Command::new("kubectl");
        command
            .env("HOME", &self.directory)
            .env_remove("KUBECONFIG")
            .env_remove("KUBERNETES_MASTER")
            .arg("--cache-dir")
            .arg(self.directory.join("kubectl-cache"));
        command
    }
    async fn rpc(&self, args: &[&str], input: Option<Vec<u8>>) -> Result<std::process::Output> {
        let mut command = self.kubectl();
        command
            .args(["--namespace", &self.namespace, "--request-timeout=30s"])
            .args(args)
            .kill_on_drop(true)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context("Kubernetes client unavailable")?;
        if let Some(input) = input {
            let mut stdin = child.stdin.take().context("Kubernetes stdin unavailable")?;
            stdin.write_all(&input).await?;
            drop(stdin);
        }
        let output =
            tokio::time::timeout(Duration::from_secs(45), child.wait_with_output()).await??;
        if !output.status.success() {
            bail!("Kubernetes RPC failed; original dispatch may require reconciliation");
        }
        if output.stdout.len() > 64 * 1024 * 1024 {
            bail!("Kubernetes response exceeds bound");
        }
        Ok(output)
    }
    async fn object(&self, kind: &str, name: &str) -> Result<Option<serde_json::Value>> {
        let output = self
            .rpc(
                &["get", kind, name, "--ignore-not-found=true", "-o", "json"],
                None,
            )
            .await?;
        if output.stdout.is_empty() {
            Ok(None)
        } else {
            Ok(Some(serde_json::from_slice(&output.stdout)?))
        }
    }
    pub(crate) async fn current(&self) -> Result<()> {
        for (name, uid) in [
            (&self.namespace, &self.namespace_uid),
            (&"kube-system".to_owned(), &self.cluster_uid),
        ] {
            let observed = self
                .object("namespace", name)
                .await?
                .context("bound Kubernetes namespace is absent")?;
            if observed["metadata"]["uid"] != *uid
                || !observed["metadata"]["deletionTimestamp"].is_null()
            {
                bail!("Kubernetes namespace or cluster identity changed");
            }
            if name == &self.namespace {
                let labels = &observed["metadata"]["labels"];
                if labels["cowork.instance-id"] != self.instance.to_string()
                    || labels["cowork.organisation-id"] != self.organisation.to_string()
                    || labels["cowork.executor-host-id"] != self.host.to_string()
                {
                    bail!("Kubernetes namespace host assignment changed");
                }
            }
        }
        Ok(())
    }
    async fn node_limits(&self) -> Result<()> {
        if self.node_names.is_empty() || self.node_names.len() > 1000 {
            bail!("original executor node inventory absent");
        }
        for name in &self.node_names {
            if name.len() > 253
                || !name.bytes().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'.')
                })
            {
                bail!("invalid original executor node name");
            }
            let node = self
                .object("node", name)
                .await?
                .context("original executor node absent")?;
            if self
                .node_uids
                .get(name)
                .is_none_or(|uid| node["metadata"]["uid"] != *uid)
                || self
                    .node_selector
                    .iter()
                    .any(|(k, v)| node["metadata"]["labels"][k] != *v)
                || node["spec"]["unschedulable"] == true
                || !node["status"]["conditions"].as_array().is_some_and(|rows| {
                    rows.iter()
                        .any(|r| r["type"] == "Ready" && r["status"] == "True")
                })
            {
                bail!("original executor node unavailable or assignment changed");
            }
            let path = format!("/api/v1/nodes/{name}/proxy/configz");
            let observed: serde_json::Value =
                serde_json::from_slice(&self.rpc(&["get", "--raw", &path], None).await?.stdout)?;
            if !observed["kubeletconfig"]["podPidsLimit"]
                .as_u64()
                .is_some_and(|p| p > 0 && p <= 64)
            {
                bail!("measured kubelet Pod PID limit must be positive and at most 64");
            }
        }
        Ok(())
    }
    fn open(&self) -> Result<()> {
        let path = self.directory.join("kubernetes-quiesce");
        if path.exists() {
            let state: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
            if state["state"] != "active" {
                bail!("original Kubernetes execution scope is quiesced");
            }
        }
        Ok(())
    }
    fn scope_binding(&self, request: &ScopeRequest) -> Result<()> {
        if request.version != 1
            || request.operation_id.is_nil()
            || request.instance_id != self.instance
            || request.organisation_id != self.organisation
            || request.namespace_uid.to_string() != self.namespace_uid
            || request.cluster_uid.to_string() != self.cluster_uid
            || request.generation != self.generation
        {
            bail!("original Kubernetes operator scope differs");
        }
        Ok(())
    }
    pub(crate) async fn scope_status(&self) -> Result<serde_json::Value> {
        self.current().await?;
        let path = self.directory.join("kubernetes-quiesce");
        let barrier = if path.exists() {
            Some(serde_json::from_slice::<serde_json::Value>(&fs::read(
                path,
            )?)?)
        } else {
            None
        };
        Ok(
            json!({"version":1,"instance_id":self.instance,"organisation_id":self.organisation,"namespace_uid":self.namespace_uid,
            "cluster_uid":self.cluster_uid,"generation":self.generation,"barrier":barrier.as_ref().filter(|v|v["state"]!="active"),"activation":barrier.as_ref().filter(|v|v["state"]=="active")}),
        )
    }
    pub(crate) async fn quiesce(&self, request: &ScopeRequest) -> Result<serde_json::Value> {
        self.current().await?;
        self.scope_binding(request)?;
        let _guard = self.lock().await?;
        let path = self.directory.join("kubernetes-quiesce");
        let original = serde_json::to_value(request)?;
        let mut proof = json!({"original":original,"state":"quiescing","physical_pods":[]});
        if path.exists() {
            proof = serde_json::from_slice(&fs::read(&path)?)?;
            if proof["original"] == original && proof["state"] == "closed" {
                return Ok(proof);
            }
            if proof["original"] != original
                && proof["state"] != "active"
                && !(proof["state"] == "closed"
                    && proof["original"]["operation_id"]
                        == serde_json::to_value(request.expected_quiesce_operation_id)?)
            {
                bail!("another original quiesce operation is pending");
            }
            proof = json!({"original":original,"state":"quiescing","physical_pods":[]});
            write(&path, &serde_json::to_vec(&proof)?)?;
        } else {
            write(&path, &serde_json::to_vec(&proof)?)?;
        }
        let mut physical = Vec::new();
        for mut row in self
            .records()?
            .into_iter()
            .filter(|r| !matches!(r.phase.as_str(), "completed" | "stopped"))
        {
            let uid = row
                .pod_uid
                .as_deref()
                .context("original native create ACK unknown; scope remains quiescing")?;
            self.delete_pod(&row).await?;
            physical.push(serde_json::from_slice::<serde_json::Value>(&fs::read(
                self.directory.join(format!("physical-stop-{uid}")),
            )?)?);
            row.phase = "stopped".into();
            self.save(&row)?;
        }
        for mut row in self
            .transfers()?
            .into_iter()
            .filter(|r| r.phase != "closed")
        {
            let uid = row
                .pod_uid
                .as_deref()
                .context("original transfer create ACK unknown; scope remains quiescing")?;
            self.delete_original_pod(&row.pod, uid).await?;
            physical.push(serde_json::from_slice::<serde_json::Value>(&fs::read(
                self.directory.join(format!("physical-stop-{uid}")),
            )?)?);
            row.phase = "closed".into();
            write(
                &self
                    .directory
                    .join(format!("transfer-{}.json", row.request_id)),
                &serde_json::to_vec(&row)?,
            )?;
        }
        proof["state"] = json!("closed");
        proof["physical_pods"] = json!(physical);
        write(&path, &serde_json::to_vec(&proof)?)?;
        Ok(proof)
    }
    pub(crate) async fn activate(&self, request: &ScopeRequest) -> Result<serde_json::Value> {
        self.current().await?;
        self.node_limits().await?;
        self.scope_binding(request)?;
        let _guard = self.lock().await?;
        let receipt = self
            .directory
            .join(format!("scope-activation-{}", request.operation_id));
        let value = serde_json::to_value(request)?;
        if receipt.exists() {
            if serde_json::from_slice::<serde_json::Value>(&fs::read(&receipt)?)? != value {
                bail!("original scope activation changed");
            }
            let state = self.scope_status().await?;
            if state.get("barrier").is_none_or(|v| v.is_null()) {
                if state["activation"]["original"] != value {
                    bail!("original activation superseded; no replay");
                }
                return Ok(state);
            }
            // Complete only the same durable activation intent after a crash
            // between its receipt and atomic barrier update.
        }
        let path = self.directory.join("kubernetes-quiesce");
        if path.exists() {
            let prior: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
            if prior["state"] != "closed"
                || prior["original"]["operation_id"]
                    != serde_json::to_value(request.expected_quiesce_operation_id)?
            {
                bail!("original closed quiesce receipt required before activation");
            }
            write(
                &self.directory.join(format!(
                    "closed-quiesce-{}",
                    prior["original"]["operation_id"]
                        .as_str()
                        .context("original quiesce ID absent")?
                )),
                &serde_json::to_vec(&prior)?,
            )?;
        } else if request.expected_quiesce_operation_id.is_some() {
            bail!("original quiesce receipt disappeared");
        }
        write(&receipt, &serde_json::to_vec(&value)?)?;
        write(
            &path,
            &serde_json::to_vec(&json!({"original":value,"state":"active"}))?,
        )?;
        self.scope_status().await
    }
    pub(crate) async fn profile(&self) -> Result<serde_json::Value> {
        self.current().await?;
        Ok(
            json!({"backend":"kubernetes","instance_id":self.instance,"organisation_id":self.organisation,"cluster_uid":self.cluster_uid,"namespace_uid":self.namespace_uid}),
        )
    }

    pub(crate) async fn read_artifact(
        &self,
        config: &Config,
        run: Uuid,
        workspace: Option<Uuid>,
        path: &str,
        request: Uuid,
        uri: &str,
    ) -> Result<Vec<u8>, RunnerError> {
        self.read_artifact_inner(config, run, workspace, path, request, uri)
            .await
            .map_err(RunnerError::admission_unavailable)
    }
    async fn read_artifact_inner(
        &self,
        config: &Config,
        run: Uuid,
        workspace: Option<Uuid>,
        path: &str,
        request: Uuid,
        uri: &str,
    ) -> Result<Vec<u8>> {
        self.current().await?;
        let pvc = format!(
            "ws-{}",
            &hex::encode(Sha256::digest(
                format!("{}:{}", self.generation, workspace.unwrap_or(run)).as_bytes()
            ))[..48]
        );
        let receipt = self
            .object("configmap", &format!("receipt-{pvc}"))
            .await?
            .context("original workspace create receipt absent")?;
        if receipt["immutable"] != true {
            bail!("workspace original receipt is mutable");
        }
        self.owned(
            &receipt,
            receipt["metadata"]["uid"]
                .as_str()
                .context("workspace receipt UID absent")?,
        )?;
        let original: serde_json::Value = serde_json::from_str(
            receipt["data"]["receipt"]
                .as_str()
                .context("workspace original receipt missing")?,
        )?;
        let uid = original["uid"]
            .as_str()
            .context("workspace original UID absent")?;
        if original["name"] != pvc {
            bail!("workspace original name changed");
        }
        let actual = self
            .object("pvc", &pvc)
            .await?
            .context("original workspace absent")?;
        self.owned(&actual, uid)?;
        self.open()?;
        let journal = self.directory.join(format!("transfer-{request}.json"));
        let mut transfer = Transfer {
            request_id: request,
            original_uri: uri.into(),
            run_id: run,
            workspace_id: workspace,
            pvc: pvc.clone(),
            pvc_uid: uid.into(),
            pod: format!("transfer-{}", request.simple()),
            pod_uid: None,
            phase: "prepared".into(),
        };
        {
            let _guard = self.lock().await?;
            self.open()?;
            if journal.exists() || self.directory.join(format!("stopped-{run}")).exists() {
                bail!("original transfer already dispatched or run stopped");
            }
            let active = self
                .records()?
                .into_iter()
                .filter(|r| !matches!(r.phase.as_str(), "completed" | "stopped"))
                .collect::<Vec<_>>();
            let mut helpers = 0;
            for entry in fs::read_dir(&self.directory)? {
                let entry = entry?;
                if !entry.file_name().to_string_lossy().starts_with("transfer-") {
                    continue;
                }
                let prior: Transfer = serde_json::from_slice(&fs::read(entry.path())?)?;
                if prior.phase != "closed" {
                    helpers += 1;
                }
            }
            if active.len() + helpers >= self.limits.containers
                || active.iter().map(|r| r.resources.cpu_nanos).sum::<u64>()
                    + (helpers + 1) as u64 * 100_000_000
                    > self.limits.cpu_nanos
                || active.iter().map(|r| r.resources.memory_bytes).sum::<u64>()
                    + (helpers + 1) as u64 * 64 * 1024 * 1024
                    > self.limits.memory_bytes
            {
                bail!("native transfer capacity remains reserved");
            }
            write(&journal, &serde_json::to_vec(&transfer)?)?;
            let mut labels = self.labels();
            labels["cowork.network"] = json!("none");
            labels["cowork.run"] = json!(run);
            let manifest = json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":transfer.pod,"labels":labels,"finalizers":["cowork.opencowork.dev/physical-stop"]},"spec":{"runtimeClassName":self.runtime_class,"nodeSelector":self.node_selector,"restartPolicy":"Never","automountServiceAccountToken":false,"activeDeadlineSeconds":180,"securityContext":{"runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,"fsGroup":10001,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"sandbox","image":config.core_image,"command":["python3","-c","import time;time.sleep(180)"],"securityContext":{"readOnlyRootFilesystem":true,"allowPrivilegeEscalation":false,"capabilities":{"drop":["ALL"]}},"resources":{"requests":{"cpu":"100m","memory":"64Mi"},"limits":{"cpu":"100m","memory":"64Mi"}},"volumeMounts":[{"name":"workspace","mountPath":"/workspace","readOnly":true}]}],"volumes":[{"name":"workspace","persistentVolumeClaim":{"claimName":pvc,"readOnly":true}}]}});
            transfer.phase = "dispatched".into();
            write(&journal, &serde_json::to_vec(&transfer)?)?;
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
                .context("original transfer pod UID absent")?;
            self.owned(&created, uid)?;
            transfer.pod_uid = Some(uid.into());
            transfer.phase = "created".into();
            write(&journal, &serde_json::to_vec(&transfer)?)?;
        }
        let uid = transfer.pod_uid.as_deref().unwrap();
        for _ in 0..120 {
            self.current().await?;
            let actual = self
                .object("pod", &transfer.pod)
                .await?
                .context("original transfer pod absent")?;
            self.owned(&actual, uid)?;
            if actual["status"]["phase"] == "Running" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let actual = self
            .object("pod", &transfer.pod)
            .await?
            .context("original transfer pod absent")?;
        self.owned(&actual, uid)?;
        transfer.phase = "read_dispatched".into();
        write(&journal, &serde_json::to_vec(&transfer)?)?;
        let script="import pathlib,sys; r=pathlib.Path('/workspace').resolve();p=(r/sys.argv[1]).resolve(strict=True);assert r in p.parents and p.is_file();assert p.stat().st_size<=67108864;f=p.open('rb');n=0\nwhile True:\n b=f.read(65536)\n if not b:break\n n+=len(b);assert n<=67108864;sys.stdout.buffer.write(b)";
        let result = self
            .rpc(
                &[
                    "exec",
                    &transfer.pod,
                    "--container=sandbox",
                    "--",
                    "python3",
                    "-c",
                    script,
                    path,
                ],
                None,
            )
            .await;
        self.delete_original_pod(&transfer.pod, uid).await?;
        transfer.phase = "closed".into();
        write(&journal, &serde_json::to_vec(&transfer)?)?;
        result.map(|v| v.stdout)
    }
    fn path(&self, attempt: Uuid) -> PathBuf {
        self.directory.join(format!("kube-{attempt}.json"))
    }
    fn save(&self, value: &Dispatch) -> Result<()> {
        write(&self.path(value.attempt), &serde_json::to_vec(value)?)
    }
    fn records(&self) -> Result<Vec<Dispatch>> {
        let mut rows = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("kube-") || !name.ends_with(".json") {
                continue;
            }
            if fs::symlink_metadata(&path)?.file_type().is_symlink() {
                bail!("unsafe Kubernetes journal");
            }
            let row: Dispatch = serde_json::from_slice(&fs::read(&path)?)?;
            if row.version != 1 || self.path(row.attempt) != path {
                bail!("invalid Kubernetes journal identity");
            }
            rows.push(row);
        }
        Ok(rows)
    }
    fn transfers(&self) -> Result<Vec<Transfer>> {
        let mut rows = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
            if name.starts_with("transfer-") && name.ends_with(".json") {
                if fs::symlink_metadata(&path)?.file_type().is_symlink() {
                    bail!("unsafe original transfer journal");
                }
                let row: Transfer = serde_json::from_slice(&fs::read(&path)?)?;
                if path
                    != self
                        .directory
                        .join(format!("transfer-{}.json", row.request_id))
                {
                    bail!("original transfer journal identity changed");
                }
                rows.push(row);
            }
        }
        Ok(rows)
    }
    fn labels(&self) -> serde_json::Value {
        json!({"cowork.instance":self.instance.to_string(),"cowork.organisation":self.organisation.to_string(),"cowork.generation":self.generation.to_string(),"cowork.managed":"native-runner"})
    }
    fn owned(&self, object: &serde_json::Value, uid: &str) -> Result<()> {
        let labels = object["metadata"]["labels"]
            .as_object()
            .context("Kubernetes ownership labels absent")?;
        if object["metadata"]["uid"] != uid
            || self
                .labels()
                .as_object()
                .unwrap()
                .iter()
                .any(|(k, v)| labels.get(k) != Some(v))
        {
            bail!("original Kubernetes UID or ownership changed");
        }
        Ok(())
    }
    async fn ensure_workspace(&self, row: &mut Dispatch) -> Result<()> {
        if let Some(actual) = self.object("pvc", &row.pvc).await? {
            let uid = actual["metadata"]["uid"]
                .as_str()
                .context("PVC UID absent")?;
            if let Some(original) = &row.pvc_uid {
                self.owned(&actual, original)?;
            } else {
                // Previously registered workspace is authority; never adopt a matching caller name.
                let index = self
                    .directory
                    .join(format!("pvc-{}", row.workspace.unwrap_or(row.run)));
                let prior: serde_json::Value = if index.exists() {
                    serde_json::from_slice(&fs::read(&index)?)?
                } else {
                    let receipt = self
                        .object("configmap", &format!("receipt-{}", row.pvc))
                        .await?
                        .context("workspace has no original creation receipt")?;
                    if receipt["immutable"] != true {
                        bail!("workspace original receipt is mutable");
                    }
                    let receipt_uid = receipt["metadata"]["uid"]
                        .as_str()
                        .context("workspace receipt UID absent")?;
                    self.owned(&receipt, receipt_uid)?;
                    serde_json::from_str(
                        receipt["data"]["receipt"]
                            .as_str()
                            .context("workspace receipt absent")?,
                    )?
                };
                if prior["name"] != row.pvc || prior["uid"] != uid {
                    bail!("workspace original receipt differs");
                }
                self.owned(&actual, uid)?;
                row.pvc_uid = Some(uid.to_owned());
                self.save(row)?;
            }
            return Ok(());
        }
        if row.pvc_uid.is_some() {
            bail!("original PVC disappeared");
        }
        let index = self
            .directory
            .join(format!("pvc-{}", row.workspace.unwrap_or(row.run)));
        if index.exists() {
            bail!("workspace creation receipt exists but PVC is absent");
        }
        let mut labels = self.labels();
        labels["cowork.workspace"] = json!(row.workspace.unwrap_or(row.run));
        let manifest = json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{"name":row.pvc,"labels":labels},"spec":{"accessModes":["ReadWriteOnce"],"storageClassName":self.storage_class,"resources":{"requests":{"storage":format!("{}Gi",self.storage_gib)}}}});
        // Dispatch is already durable. Unknown create response is never retried/adopted.
        row.phase = "pvc_dispatched".into();
        self.save(row)?;
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
            .context("created PVC has no UID")?;
        self.owned(&actual, uid)?;
        row.pvc_uid = Some(uid.into());
        let receipt = json!({"name":row.pvc,"uid":uid});
        write(&index, &serde_json::to_vec(&receipt)?)?;
        self.save(row)?;
        let configmap = json!({"apiVersion":"v1","kind":"ConfigMap","immutable":true,"metadata":{"name":format!("receipt-{}",row.pvc),"labels":self.labels()},"data":{"receipt":serde_json::to_string(&receipt)?}});
        self.rpc(
            &["create", "-f", "-", "-o", "json"],
            Some(serde_json::to_vec(&configmap)?),
        )
        .await?;
        Ok(())
    }
    async fn delete_pod(&self, row: &Dispatch) -> Result<()> {
        let uid = row
            .pod_uid
            .as_deref()
            .context("original pod create receipt is unknown")?;
        self.delete_original_pod(&row.pod, uid).await
    }
    async fn delete_original_pod(&self, name: &str, uid: &str) -> Result<()> {
        let proof_path = self.directory.join(format!("physical-stop-{uid}"));
        let mut terminated = proof_path.exists();
        if let Some(actual) = self.object("pod", name).await? {
            self.owned(&actual, uid)?;
            let uri = format!("/api/v1/namespaces/{}/pods/{}", self.namespace, name);
            if actual["metadata"]["finalizers"] != json!(["cowork.opencowork.dev/physical-stop"])
                && !terminated
            {
                bail!("original pod physical-stop finalizer changed");
            }
            let options = json!({"apiVersion":"v1","kind":"DeleteOptions","gracePeriodSeconds":30,"preconditions":{"uid":uid}});
            self.rpc(
                &["delete", "--raw", &uri, "-f", "-"],
                Some(serde_json::to_vec(&options)?),
            )
            .await?;
        }
        for _ in 0..360 {
            match self.object("pod", name).await? {
                None if terminated => return Ok(()),
                None => bail!("original pod disappeared without physical termination evidence"),
                Some(v) => {
                    self.owned(&v, uid)?;
                    if !terminated
                        && matches!(v["status"]["phase"].as_str(), Some("Succeeded" | "Failed"))
                        && v["status"]["containerStatuses"]
                            .as_array()
                            .is_some_and(|rows| {
                                !rows.is_empty()
                                    && rows
                                        .iter()
                                        .all(|r| r["state"]["terminated"]["finishedAt"].is_string())
                            })
                    {
                        write(
                            &proof_path,
                            &serde_json::to_vec(
                                &json!({"pod_uid":uid,"namespace_uid":self.namespace_uid,"cluster_uid":self.cluster_uid,"container_statuses":v["status"]["containerStatuses"]}),
                            )?,
                        )?;
                        terminated = true;
                    }
                    if terminated
                        && v["metadata"]["finalizers"]
                            == json!(["cowork.opencowork.dev/physical-stop"])
                    {
                        let patch = json!([{"op":"test","path":"/metadata/uid","value":uid},{"op":"test","path":"/metadata/resourceVersion","value":v["metadata"]["resourceVersion"]},{"op":"test","path":"/metadata/finalizers","value":["cowork.opencowork.dev/physical-stop"]},{"op":"remove","path":"/metadata/finalizers"}]);
                        self.rpc(
                            &[
                                "patch",
                                "pod",
                                name,
                                "--type=json",
                                "-p",
                                &serde_json::to_string(&patch)?,
                            ],
                            None,
                        )
                        .await?;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        bail!("original pod deletion has no physical termination and absent readback");
    }
    pub(crate) async fn execute(
        &self,
        config: &Config,
        spec: &SandboxRunSpec,
        execution: Option<&admission::Execution>,
        body: &[u8],
    ) -> Result<SandboxRunResult, RunnerError> {
        self.execute_inner(config, spec, execution, body)
            .await
            .map_err(RunnerError::admission_unavailable)
    }
    async fn execute_inner(
        &self,
        config: &Config,
        spec: &SandboxRunSpec,
        execution: Option<&admission::Execution>,
        body: &[u8],
    ) -> Result<SandboxRunResult> {
        let execution =
            execution.context("native Kubernetes jobs require original attempt and lease")?;
        if execution.version != 1 || execution.attempt_id.is_nil() || execution.lease_token.is_nil()
        {
            bail!("invalid original execution authority");
        }
        if spec.limits.pids < 64 {
            bail!("native core profile requires an original PID allowance of at least 64");
        }
        if spec.image == SandboxImage::Gui {
            return self.execute_desktop_job(spec, Some(execution), body).await;
        }
        self.current().await?;
        self.node_limits().await?;
        self.open()?;
        let sha = hex::encode(Sha256::digest(body));
        let mut row;
        {
            let _guard = self.lock().await?;
            self.open()?;
            let rows = self.records()?;
            if self
                .directory
                .join(format!("stopped-{}", spec.run_id))
                .exists()
            {
                bail!("execution was stopped");
            }
            if let Some(prior) = rows.iter().find(|v| v.attempt == execution.attempt_id) {
                if prior.run != spec.run_id
                    || prior.workspace != spec.workspace_id
                    || prior.lease != execution.lease_token
                    || prior.request_sha256 != sha
                {
                    bail!("original execution body changed");
                }
                return prior
                    .result
                    .clone()
                    .context("original Kubernetes launch is unsettled; no replay");
            }
            let active: Vec<_> = rows
                .iter()
                .filter(|r| !matches!(r.phase.as_str(), "completed" | "stopped"))
                .collect();
            let helpers = self
                .transfers()?
                .into_iter()
                .filter(|r| r.phase != "closed")
                .count();
            if active.len() + helpers >= self.limits.containers
                || active
                    .iter()
                    .map(|r| r.run)
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    >= self.limits.concurrent_runs
                || active.iter().map(|r| r.resources.cpu_nanos).sum::<u64>()
                    + spec.limits.cpu_nanos
                    + helpers as u64 * 100_000_000
                    > self.limits.cpu_nanos
                || active.iter().map(|r| r.resources.memory_bytes).sum::<u64>()
                    + spec.limits.memory_bytes
                    + helpers as u64 * 64 * 1024 * 1024
                    > self.limits.memory_bytes
            {
                bail!("native Kubernetes capacity is reserved");
            }
            row = Dispatch {
                version: 1,
                run: spec.run_id,
                workspace: spec.workspace_id,
                attempt: execution.attempt_id,
                lease: execution.lease_token,
                request_sha256: sha,
                resources: Resources {
                    cpu_nanos: spec.limits.cpu_nanos,
                    memory_bytes: spec.limits.memory_bytes,
                },
                pod: format!("job-{}", execution.attempt_id.simple()),
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
                phase: "prepared".into(),
                result: None,
                kind: None,
            };
            self.save(&row)?;
            self.ensure_workspace(&mut row).await?;
            let image = if spec.image == SandboxImage::Crew {
                &config.crew_image
            } else {
                &config.core_image
            };
            if !image.contains("@sha256:")
                || image.rsplit("@sha256:").next().is_none_or(|v| {
                    v.len() != 64
                        || !v
                            .bytes()
                            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                })
            {
                bail!("Kubernetes image must be digest pinned");
            }
            let mut labels = self.labels();
            labels["cowork.network"] = json!(if spec.network == SandboxNetwork::None {
                "none"
            } else {
                "filtered"
            });
            labels["cowork.run"] = json!(row.run);
            labels["cowork.attempt"] = json!(row.attempt);
            let manifest = json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":row.pod,"labels":labels,"finalizers":["cowork.opencowork.dev/physical-stop"]},"spec":{"runtimeClassName":self.runtime_class,"nodeSelector":self.node_selector,"restartPolicy":"Never","automountServiceAccountToken":false,"activeDeadlineSeconds":spec.limits.timeout_seconds+90,"securityContext":{"runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,"fsGroup":10001,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"sandbox","image":image,"command":["python3","-c","import time; time.sleep(90000)"],"securityContext":{"readOnlyRootFilesystem":true,"allowPrivilegeEscalation":false,"capabilities":{"drop":["ALL"]}},"resources":{"requests":{"cpu":format!("{}m",spec.limits.cpu_nanos/1_000_000),"memory":spec.limits.memory_bytes.to_string()},"limits":{"cpu":format!("{}m",spec.limits.cpu_nanos/1_000_000),"memory":spec.limits.memory_bytes.to_string()}},"volumeMounts":[{"name":"workspace","mountPath":"/workspace"},{"name":"tmp","mountPath":"/tmp"}]}],"volumes":[{"name":"workspace","persistentVolumeClaim":{"claimName":row.pvc}},{"name":"tmp","emptyDir":{"medium":"Memory","sizeLimit":spec.limits.tmpfs_bytes.max(1048576).to_string()}}]}});
            row.phase = "pod_dispatched".into();
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
                .context("original pod UID absent")?;
            self.owned(&created, uid)?;
            row.pod_uid = Some(uid.into());
            row.phase = "pod_created".into();
            self.save(&row)?;
        }
        for _ in 0..120 {
            self.current().await?;
            if self.directory.join(format!("stopped-{}", row.run)).exists() {
                bail!("execution stopped before native exec");
            }
            let actual = self
                .object("pod", &row.pod)
                .await?
                .context("original pod absent")?;
            self.owned(&actual, row.pod_uid.as_deref().unwrap())?;
            if actual["status"]["phase"] == "Running" {
                break;
            }
            if matches!(
                actual["status"]["phase"].as_str(),
                Some("Failed" | "Succeeded")
            ) {
                bail!("sandbox terminated before original exec");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        {
            let _guard = self.lock().await?;
            if self.directory.join(format!("stopped-{}", row.run)).exists() {
                bail!("execution stopped before native exec");
            }
            self.open()?;
            row.phase = "exec_dispatched".into();
            self.save(&row)?;
        }
        // One original exec, never retried. RLIMIT_NPROC and bounded pipes apply
        // to the workload and descendants inside the sandbox runtime class.
        let script="import sys,json,subprocess,base64,resource,os,selectors,time,tempfile; p=json.loads(sys.argv[1]); resource.setrlimit(resource.RLIMIT_NPROC,(p['pids'],p['pids'])); os.chdir('/workspace'); e={**os.environ,**p['env']}; f=tempfile.TemporaryFile(); f.write(base64.b64decode(p['stdin'])); f.seek(0); c=subprocess.Popen(p['argv'],stdin=f,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=e,start_new_session=True); s=selectors.DefaultSelector(); s.register(c.stdout,selectors.EVENT_READ,'out'); s.register(c.stderr,selectors.EVENT_READ,'err'); b={'out':bytearray(),'err':bytearray()}; trunc=False; deadline=time.monotonic()+p['timeout']; timed=False\nwhile s.get_map():\n if time.monotonic()>deadline and not timed:\n  timed=True\n  try: os.killpg(c.pid,9)\n  except ProcessLookupError: pass\n for k,_ in s.select(.1):\n  x=os.read(k.fileobj.fileno(),65536)\n  if not x: s.unregister(k.fileobj); continue\n  room=max(0,p['max']-len(b[k.data])); b[k.data].extend(x[:room]); trunc=trunc or len(x)>room\nc.wait();\ntry: os.killpg(c.pid,9)\nexcept ProcessLookupError: pass\nprint(json.dumps({'exit':c.returncode,'timed_out':timed,'stdout':b['out'].decode('utf8','replace'),'stderr':b['err'].decode('utf8','replace'),'truncated':trunc}))";
        let mut environment = spec.environment.clone();
        if spec.network == SandboxNetwork::FilteredEgress {
            let proxy = config
                .filtered_egress_proxy
                .as_ref()
                .context("filtered proxy missing")?;
            for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
                environment.insert(key.into(), proxy.clone());
            }
            environment.insert("NO_PROXY".into(), "localhost,127.0.0.1,::1".into());
        }
        let payload = serde_json::to_string(
            &json!({"argv":spec.argv,"env":environment,"stdin":spec.stdin_base64.as_deref().unwrap_or(""),"pids":spec.limits.pids,"timeout":spec.limits.timeout_seconds,"max":spec.limits.output_bytes}),
        )?;
        // argv is capped by the existing spec validator; large stdin is sent in
        // the stream, not argv or Kubernetes objects (which have a 1 MiB bound).
        let stream_script =
            script.replacen("p=json.loads(sys.argv[1])", "p=json.load(sys.stdin)", 1);
        let output = self
            .rpc_exec(
                &row,
                &stream_script,
                payload.into_bytes(),
                spec.limits.timeout_seconds + 15,
            )
            .await?;
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let result = SandboxRunResult {
            schema_version: SCHEMA_VERSION,
            run_id: row.run,
            container_name: row.pod.clone(),
            workspace_volume: row.pvc.clone(),
            exit_code: value["exit"].as_i64().map(|v| v as i32),
            timed_out: value["timed_out"]
                .as_bool()
                .context("native timeout receipt absent")?,
            stdout: value["stdout"]
                .as_str()
                .context("native stdout receipt absent")?
                .into(),
            stderr: value["stderr"]
                .as_str()
                .context("native stderr receipt absent")?
                .into(),
            output_truncated: value["truncated"]
                .as_bool()
                .context("native truncation receipt absent")?,
        };
        let _guard = self.lock().await?;
        self.delete_pod(&row).await?;
        row.result = Some(result.clone());
        row.phase = "completed".into();
        self.save(&row)?;
        Ok(result)
    }
    async fn rpc_exec(
        &self,
        row: &Dispatch,
        script: &str,
        input: Vec<u8>,
        seconds: u64,
    ) -> Result<std::process::Output> {
        self.current().await?;
        self.open()?;
        let actual = self
            .object("pod", &row.pod)
            .await?
            .context("original pod absent")?;
        self.owned(&actual, row.pod_uid.as_deref().context("pod UID absent")?)?;
        let mut child = self
            .kubectl()
            .args([
                "--namespace",
                &self.namespace,
                "exec",
                "-i",
                &row.pod,
                "--container=sandbox",
                "--",
                "python3",
                "-c",
                script,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut pipe = child
            .stdin
            .take()
            .context("native exec input unavailable")?;
        let writer = tokio::spawn(async move { pipe.write_all(&input).await });
        let output =
            tokio::time::timeout(Duration::from_secs(seconds), child.wait_with_output()).await;
        writer.abort();
        let output =
            output.context("original native exec timed out; stop/reconcile required")??;
        if !output.status.success() {
            bail!("original native exec failed; no replay");
        }
        Ok(output)
    }
    pub(crate) async fn stop(
        &self,
        request: &admission::StopRequest,
    ) -> Result<serde_json::Value, RunnerError> {
        self.stop_inner(request)
            .await
            .map_err(RunnerError::admission_unavailable)
    }
    async fn stop_inner(&self, request: &admission::StopRequest) -> Result<serde_json::Value> {
        self.current().await?;
        let _guard = self.lock().await?;
        if request.version != 1 || request.run_id.is_nil() {
            bail!("invalid original stop request");
        }
        let marker = self.directory.join(format!("stopped-{}", request.run_id));
        let stop = serde_json::to_value(request)?;
        if marker.exists() {
            if serde_json::from_slice::<serde_json::Value>(&fs::read(&marker)?)? != stop {
                bail!("original stop authority changed");
            }
        } else {
            write(&marker, &serde_json::to_vec(&stop)?)?;
        }
        let mut stopped = Vec::new();
        let mut physical = Vec::new();
        let mut physical_desktops = Vec::new();
        let mut physical_transfers = Vec::new();
        for mut transfer in self
            .transfers()?
            .into_iter()
            .filter(|r| r.run_id == request.run_id)
        {
            let uid = transfer
                .pod_uid
                .as_deref()
                .context("original transfer create ACK unknown; no stop receipt")?;
            if transfer.phase != "closed" {
                self.delete_original_pod(&transfer.pod, uid).await?;
                transfer.phase = "closed".into();
                write(
                    &self
                        .directory
                        .join(format!("transfer-{}.json", transfer.request_id)),
                    &serde_json::to_vec(&transfer)?,
                )?;
            }
            let proof: serde_json::Value = serde_json::from_slice(&fs::read(
                self.directory.join(format!("physical-stop-{uid}")),
            )?)?;
            physical_transfers
                .push(json!({"request_id":transfer.request_id,"pod_uid":uid,"proof":proof}));
        }
        for mut row in self
            .records()?
            .into_iter()
            .filter(|r| r.run == request.run_id)
        {
            if row.kind.as_deref() == Some("desktop") {
                if row.phase != "stopped" {
                    if request.lease_token != Some(row.lease) && request.cancel_lease.is_none() {
                        bail!("original native desktop stop authority missing");
                    }
                    self.delete_pod(&row).await?;
                    row.phase = "stopped".into();
                    self.save(&row)?;
                }
                let uid = row
                    .pod_uid
                    .as_deref()
                    .context("original desktop create ACK unknown")?;
                let proof: serde_json::Value = serde_json::from_slice(&fs::read(
                    self.directory.join(format!("physical-stop-{uid}")),
                )?)?;
                physical_desktops.push(proof);
                continue;
            }
            let original = request
                .attempts
                .iter()
                .find(|a| a.attempt_id == row.attempt)
                .context("stop omitted an original admitted attempt")?;
            if original.lease_token != row.lease
                || original.request_sha256 != row.request_sha256
                || original.workspace_id != row.workspace
            {
                bail!("stop original execution differs");
            }
            if !matches!(row.phase.as_str(), "completed" | "stopped") {
                if row.pod_uid.is_none() {
                    bail!("original native create ACK unknown; stop cannot claim absence");
                }
                self.delete_pod(&row).await?;
                row.phase = "stopped".into();
                self.save(&row)?;
            }
            stopped.push(row.attempt);
            if let Some(uid) = &row.pod_uid {
                let proof: serde_json::Value = serde_json::from_slice(
                    &fs::read(self.directory.join(format!("physical-stop-{uid}")))
                        .context("original pod termination receipt absent")?,
                )?;
                if proof["pod_uid"] != *uid
                    || proof["namespace_uid"] != self.namespace_uid
                    || proof["cluster_uid"] != self.cluster_uid
                {
                    bail!("original physical termination receipt changed");
                }
                if !physical
                    .iter()
                    .any(|v: &serde_json::Value| v["pod_uid"] == *uid)
                {
                    physical.push(json!({"attempt_id":row.attempt,"pod_uid":uid,"proof":proof}));
                }
            }
        }
        Ok(
            json!({"version":1,"run_id":request.run_id,"lease_token":request.lease_token,"cancel_lease":request.cancel_lease,"attempts":request.attempts,"execution_stopped":true,"containers":[],"backend":"kubernetes","namespace_uid":self.namespace_uid,"cluster_uid":self.cluster_uid,"physical_pods":physical,"physical_transfers":physical_transfers,"physical_desktops":physical_desktops,"stopped_attempt_ids":stopped}),
        )
    }
}
fn dns(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 63
        || !value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        || value.starts_with('-')
        || value.ends_with('-')
    {
        bail!("invalid Kubernetes configured name");
    }
    Ok(value.into())
}
fn write(path: &FsPath, data: &[u8]) -> Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!("unsafe Kubernetes journal path");
    }
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    use std::io::Write;
    file.write_all(data)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    #[cfg(unix)]
    fs::File::open(path.parent().context("journal parent absent")?)?.sync_all()?;
    Ok(())
}
