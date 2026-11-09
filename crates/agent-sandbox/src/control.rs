use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tokio::time::{Instant, sleep};

pub const STOP_FINALIZER: &str = "cowork.local/physical-stop";
const MAX_RESPONSE: usize = 4 * 1024 * 1024;

/// Private operator configuration. The renderer cannot select a cluster, namespace,
/// runtime handler, identity, or endpoint. Projected service-account tokens rotate.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub api_url: String,
    pub namespace: String,
    pub namespace_uid: String,
    #[serde(default = "cluster_domain")]
    pub cluster_domain: String,
    /// Private operator allowlist. An owner-selected image cannot establish the
    /// trustworthiness of sandboxd or the immutable boundary/export helper.
    pub approved_images: Vec<String>,
    pub token_file: PathBuf,
    pub ca_file: PathBuf,
    pub runtime_class: String,
    pub runtime_handler: String,
    pub storage_class: String,
    pub pvc_access_mode: String,
    pub worker_namespace: String,
    pub worker_label_value: String,
    #[serde(default)]
    pub web_proxy_policies: BTreeMap<String, WebProxyPolicy>,
    /// Reachable control-plane canary for proving direct egress is denied in the
    /// workload. Required for NetworkDenied qualification; never a localhost probe.
    #[serde(default)]
    pub egress_probe_address: Option<String>,
    #[serde(default)]
    pub preview_base_domain: Option<String>,
    #[serde(default = "preview_ports")]
    pub preview_allowed_ports: Vec<u16>,
    #[serde(default)]
    pub preview_validator_token_file: Option<PathBuf>,
    #[serde(default)]
    pub preview_gateway: Option<PreviewGateway>,
    #[serde(default = "idle_default")]
    pub idle_suspend_seconds: u64,
    #[serde(default = "active_default")]
    pub max_active_sessions: usize,
    #[serde(default = "ready_timeout")]
    pub readiness_timeout_seconds: u64,
    #[serde(default = "stop_timeout")]
    pub stop_timeout_seconds: u64,
}
fn cluster_domain() -> String {
    "cluster.local".into()
}
fn preview_ports() -> Vec<u16> {
    vec![8081]
}
fn idle_default() -> u64 {
    900
}
fn active_default() -> usize {
    2
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewGateway {
    pub namespace: String,
    pub pod_labels: BTreeMap<String, String>,
    pub ports: Vec<u16>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebProxyPolicy {
    pub url: String,
    pub namespace: String,
    pub pod_labels: BTreeMap<String, String>,
    pub port: u16,
    pub probe_url: String,
    pub direct_probe_address: String,
}
impl WebProxyPolicy {
    fn validate(&self) -> Result<()> {
        dns_label(&self.namespace)?;
        let url = reqwest::Url::parse(&self.url)?;
        ensure!(
            url.scheme() == "http"
                && url.host_str().is_some()
                && url.port_or_known_default() == Some(self.port)
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none(),
            "Proxy must be an operator-selected HTTP origin without credentials"
        );
        ensure!(
            !self.pod_labels.is_empty()
                && self.pod_labels.len() <= 16
                && self.pod_labels.iter().all(|(k, v)| !k.is_empty()
                    && k.len() <= 253
                    && !v.is_empty()
                    && v.len() <= 63),
            "Proxy Pod selector required"
        );
        let target = reqwest::Url::parse(&self.probe_url)?;
        ensure!(
            target.scheme() == "http"
                && target.host_str().is_some()
                && target.username().is_empty()
                && target.password().is_none(),
            "HTTP probe URL required"
        );
        let _: std::net::SocketAddr = self.direct_probe_address.parse()?;
        Ok(())
    }
    pub fn environment(&self) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::from([
            ("HTTP_PROXY".into(), self.url.clone()),
            ("HTTPS_PROXY".into(), self.url.clone()),
            ("ALL_PROXY".into(), self.url.clone()),
            ("NO_PROXY".into(), String::new()),
        ])
    }
}
fn ready_timeout() -> u64 {
    180
}
fn stop_timeout() -> u64 {
    120
}
pub fn dns_label(s: &str) -> Result<()> {
    ensure!(
        !s.is_empty()
            && s.len() <= 63
            && s.as_bytes()[0].is_ascii_alphanumeric()
            && s.as_bytes()[s.len() - 1].is_ascii_alphanumeric()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        "Invalid Kubernetes name"
    );
    Ok(())
}
impl Configuration {
    pub fn validate(&self) -> Result<()> {
        for s in [&self.namespace, &self.runtime_class, &self.worker_namespace] {
            dns_label(s)?;
        }
        ensure!(
            self.cluster_domain.len() <= 190,
            "Invalid cluster DNS suffix"
        );
        for label in self.cluster_domain.split('.') {
            dns_label(label)?;
        }
        ensure!(
            !self.approved_images.is_empty() && self.approved_images.len() <= 64,
            "Operator-approved runtime images are required"
        );
        for image in &self.approved_images {
            let (repository, digest) = image
                .rsplit_once("@sha256:")
                .context("Approved runtime image must use an immutable repository digest")?;
            ensure!(
                !repository.is_empty()
                    && !repository.contains(char::is_whitespace)
                    && digest.len() == 64
                    && digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "Invalid operator-approved image digest"
            );
        }
        ensure!(
            !self.namespace_uid.is_empty() && self.namespace_uid.len() <= 128,
            "Namespace UID must be pinned"
        );
        ensure!(
            self.runtime_handler.starts_with("kata") && self.runtime_handler.len() <= 128,
            "A Kata runtime handler is required"
        );
        ensure!(
            !self.storage_class.is_empty() && self.storage_class.len() <= 253,
            "Explicit storage class required"
        );
        ensure!(
            matches!(
                self.pvc_access_mode.as_str(),
                "ReadWriteOncePod" | "ReadWriteOnce"
            ),
            "Single-writer PVC mode is required"
        );
        ensure!(
            self.worker_label_value.len() == 32
                && self
                    .worker_label_value
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit()),
            "Runner binding digest required"
        );
        ensure!(
            self.ca_file.is_absolute() && self.token_file.is_absolute(),
            "Absolute Kubernetes identity paths required"
        );
        ensure!(
            (1..=900).contains(&self.readiness_timeout_seconds)
                && (1..=900).contains(&self.stop_timeout_seconds),
            "Invalid observation deadline"
        );
        let url = reqwest::Url::parse(&self.api_url)?;
        ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/",
            "Kubernetes API must be an explicit HTTPS origin"
        );
        for (key, policy) in &self.web_proxy_policies {
            ensure!(
                !key.is_empty() && key.len() <= 128,
                "Invalid proxy policy identity"
            );
            policy.validate()?;
        }
        if let Some(address) = &self.egress_probe_address {
            let address: std::net::SocketAddr = address.parse()?;
            ensure!(
                !address.ip().is_loopback() && !address.ip().is_unspecified() && address.port() > 0,
                "Egress canary must leave sandbox loopback"
            );
        }
        ensure!(
            !self.preview_allowed_ports.is_empty()
                && self.preview_allowed_ports.len() <= 16
                && self
                    .preview_allowed_ports
                    .iter()
                    .all(|p| *p > 1024 && !matches!(*p, 8080 | 9090)),
            "Preview ports must exclude runtime control plane"
        );
        ensure!(
            (60..=86400).contains(&self.idle_suspend_seconds)
                && (1..=100).contains(&self.max_active_sessions),
            "Idle/session admission limit invalid"
        );
        if let Some(domain) = &self.preview_base_domain {
            ensure!(
                domain.len() <= 220 && domain.contains('.'),
                "Dedicated preview domain required"
            );
            for label in domain.split('.') {
                dns_label(label)?;
            }
        }
        if let Some(file) = &self.preview_validator_token_file {
            ensure!(
                file.is_absolute(),
                "Absolute preview validator credential required"
            );
        }
        if let Some(gateway) = &self.preview_gateway {
            dns_label(&gateway.namespace)?;
            ensure!(
                !gateway.pod_labels.is_empty()
                    && gateway
                        .ports
                        .iter()
                        .all(|p| self.preview_allowed_ports.contains(p)),
                "Preview gateway selectors/ports invalid"
            );
        }
        Ok(())
    }
    pub fn approve_image(&self, image: &str) -> Result<()> {
        ensure!(
            self.approved_images.iter().any(|allowed| allowed == image),
            "Runtime image is not approved by the operator"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PodIdentity {
    pub name: String,
    pub uid: String,
    pub node: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StopReceipt {
    pub sandbox_uid: String,
    pub pod: PodIdentity,
    pub terminated_container_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedSandbox {
    pub name: String,
    pub nonce: String,
    pub uid: Option<String>,
    pub pod: Option<PodIdentity>,
    pub stopped: Option<StopReceipt>,
}
#[derive(Clone, Debug)]
pub struct Resources {
    pub memory_bytes: u64,
    pub cpu_millis: u32,
    pub scratch_bytes: u64,
}
#[derive(Clone)]
pub struct ControlPlane {
    config: Configuration,
    client: Client,
}
pub fn field<'a>(v: &'a Value, path: &str) -> Result<&'a str> {
    v.pointer(path)
        .and_then(Value::as_str)
        .context("Kubernetes identity field missing")
}
impl ControlPlane {
    pub fn new(config: Configuration) -> Result<Self> {
        config.validate()?;
        // No ambient proxies, redirects, anonymous fallback, or system trust roots.
        let ca = std::fs::read(&config.ca_file).context("Kubernetes CA is unavailable")?;
        ensure!(ca.len() <= 1024 * 1024, "Kubernetes CA is oversized");
        let mut builder = Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .tls_built_in_root_certs(false)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30));
        for cert in reqwest::Certificate::from_pem_bundle(&ca)? {
            builder = builder.add_root_certificate(cert);
        }
        Ok(Self {
            config,
            client: builder.build()?,
        })
    }
    pub fn config(&self) -> &Configuration {
        &self.config
    }
    fn path(&self, kind: &str, name: Option<&str>) -> Result<String> {
        let base = match kind {
            "sandboxes" => format!(
                "/apis/agents.x-k8s.io/v1beta1/namespaces/{}/sandboxes",
                self.config.namespace
            ),
            "networkpolicies" => format!(
                "/apis/networking.k8s.io/v1/namespaces/{}/networkpolicies",
                self.config.namespace
            ),
            "pods" | "persistentvolumeclaims" => {
                format!("/api/v1/namespaces/{}/{kind}", self.config.namespace)
            }
            _ => bail!("Unsupported Kubernetes resource"),
        };
        Ok(if let Some(name) = name {
            dns_label(name)?;
            format!("{base}/{name}")
        } else {
            base
        })
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        patch: bool,
    ) -> Result<Option<Value>> {
        // Deliberately reread a projected token for every request; never log its body.
        let token = std::fs::read_to_string(&self.config.token_file)
            .context("Kubernetes projected identity unavailable")?;
        let token = token.trim();
        ensure!(
            (16..=16384).contains(&token.len()) && !token.chars().any(char::is_whitespace),
            "Invalid Kubernetes identity"
        );
        let mut request = self
            .client
            .request(
                method.clone(),
                format!("{}{path}", self.config.api_url.trim_end_matches('/')),
            )
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(body);
        }
        if patch {
            request = request.header("content-type", "application/json-patch+json");
        }
        let mut response = request
            .send()
            .await
            .context("Kubernetes operation uncertain; mutation must not be replayed")?;
        if response.status() == StatusCode::NOT_FOUND && method == Method::GET {
            return Ok(None);
        }
        ensure!(
            response.status().is_success(),
            "Kubernetes operation rejected ({})",
            response.status()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= MAX_RESPONSE,
                "Kubernetes response exceeded limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(Some(serde_json::from_slice(&bytes)?))
    }
    pub async fn get(&self, kind: &str, name: &str) -> Result<Option<Value>> {
        self.request(Method::GET, &self.path(kind, Some(name))?, None, false)
            .await
    }
    pub async fn inspect(&self) -> Result<()> {
        let ns = self
            .request(
                Method::GET,
                &format!("/api/v1/namespaces/{}", self.config.namespace),
                None,
                false,
            )
            .await?
            .context("Namespace absent")?;
        ensure!(
            field(&ns, "/metadata/uid")? == self.config.namespace_uid,
            "Namespace identity changed"
        );
        let runtime = self
            .request(
                Method::GET,
                &format!(
                    "/apis/node.k8s.io/v1/runtimeclasses/{}",
                    self.config.runtime_class
                ),
                None,
                false,
            )
            .await?
            .context("RuntimeClass absent")?;
        ensure!(
            field(&runtime, "/handler")? == self.config.runtime_handler,
            "Kata runtime handler changed"
        );
        Ok(())
    }
    fn labels(&self, nonce: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "cowork.local/runner".into(),
                self.config.worker_label_value.clone(),
            ),
            ("cowork.local/nonce".into(), nonce.into()),
            ("app.kubernetes.io/name".into(), "cowork-sandbox".into()),
        ])
    }
    pub fn verify_owned(&self, value: &Value, owned: &OwnedSandbox) -> Result<()> {
        ensure!(
            field(value, "/metadata/namespace")? == self.config.namespace
                && field(value, "/metadata/name")? == owned.name
                && field(value, "/metadata/labels/cowork.local~1runner")?
                    == self.config.worker_label_value
                && field(value, "/metadata/labels/cowork.local~1nonce")? == owned.nonce,
            "Foreign Kubernetes resource"
        );
        if let Some(uid) = &owned.uid {
            ensure!(
                field(value, "/metadata/uid")? == uid,
                "Sandbox physical identity changed"
            );
        }
        Ok(())
    }
    pub async fn create_pvc(&self, name: &str, nonce: &str, bytes: u64) -> Result<String> {
        dns_label(name)?;
        let value = json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{"name":name,"namespace":self.config.namespace,"labels":self.labels(nonce)},"spec":{"storageClassName":self.config.storage_class,"accessModes":[self.config.pvc_access_mode],"resources":{"requests":{"storage":bytes.to_string()}}}});
        let created = self
            .request(
                Method::POST,
                &self.path("persistentvolumeclaims", None)?,
                Some(&value),
                false,
            )
            .await?
            .context("PVC receipt missing")?;
        ensure!(
            created
                .pointer("/metadata/ownerReferences")
                .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty)),
            "Workspace PVC must be app-owned"
        );
        Ok(field(&created, "/metadata/uid")?.into())
    }
    pub async fn verify_pvc(&self, name: &str, nonce: &str, uid: &str) -> Result<()> {
        let value = self
            .get("persistentvolumeclaims", name)
            .await?
            .context("Workspace PVC disappeared")?;
        self.verify_owned(
            &value,
            &OwnedSandbox {
                name: name.into(),
                nonce: nonce.into(),
                uid: Some(uid.into()),
                pod: None,
                stopped: None,
            },
        )?;
        ensure!(
            value
                .pointer("/metadata/ownerReferences")
                .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty))
                && value
                    .pointer("/spec/storageClassName")
                    .and_then(Value::as_str)
                    == Some(self.config.storage_class.as_str()),
            "Workspace PVC ownership/storage class changed"
        );
        Ok(())
    }
    fn policy_value(&self, name: &str, nonce: &str, proxy: Option<&WebProxyPolicy>) -> Value {
        let egress = match proxy {
            None => json!([]),
            Some(proxy) => {
                json!([{"to":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":proxy.namespace}},"podSelector":{"matchLabels":proxy.pod_labels}}],"ports":[{"protocol":"TCP","port":proxy.port}]},{"to":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"kube-system"}},"podSelector":{"matchLabels":{"k8s-app":"kube-dns"}}}],"ports":[{"protocol":"UDP","port":53},{"protocol":"TCP","port":53}]}])
            }
        };
        let mut worker_ports = vec![8080u16, 9090];
        if self.config.preview_gateway.is_some() {
            worker_ports.extend(self.config.preview_allowed_ports.iter().copied());
        }
        let mut ingress = vec![
            json!({"from":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":self.config.worker_namespace}},"podSelector":{"matchLabels":{"cowork.local/runner":self.config.worker_label_value,"app.kubernetes.io/name":"cowork-process-runner"}}}],"ports":worker_ports.iter().map(|port|json!({"protocol":"TCP","port":port})).collect::<Vec<_>>()}),
        ];
        if let Some(gateway) = &self.config.preview_gateway {
            ingress.push(json!({"from":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":gateway.namespace}},"podSelector":{"matchLabels":gateway.pod_labels}}],"ports":gateway.ports.iter().map(|port|json!({"protocol":"TCP","port":port})).collect::<Vec<_>>()}));
        }
        json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":{"name":name,"namespace":self.config.namespace,"labels":self.labels(nonce)},"spec":{"podSelector":{"matchLabels":{"cowork.local/nonce":nonce,"cowork.local/runner":self.config.worker_label_value}},"policyTypes":["Ingress","Egress"],"egress":egress,"ingress":ingress}})
    }
    pub async fn create_policy(
        &self,
        name: &str,
        nonce: &str,
        proxy: Option<&WebProxyPolicy>,
    ) -> Result<String> {
        let value = self.policy_value(name, nonce, proxy);
        let created = self
            .request(
                Method::POST,
                &self.path("networkpolicies", None)?,
                Some(&value),
                false,
            )
            .await?
            .context("Policy receipt missing")?;
        ensure!(
            normalized_policy(&created["spec"]) == normalized_policy(&value["spec"]),
            "Admission changed network policy"
        );
        Ok(field(&created, "/metadata/uid")?.into())
    }
    pub async fn verify_policy(
        &self,
        name: &str,
        nonce: &str,
        uid: &str,
        proxy: Option<&WebProxyPolicy>,
    ) -> Result<()> {
        let policy = self
            .get("networkpolicies", name)
            .await?
            .context("Workload network policy disappeared")?;
        self.verify_owned(
            &policy,
            &OwnedSandbox {
                name: name.into(),
                nonce: nonce.into(),
                uid: Some(uid.into()),
                pod: None,
                stopped: None,
            },
        )?;
        ensure!(
            normalized_policy(&policy["spec"])
                == normalized_policy(&self.policy_value(name, nonce, proxy)["spec"]),
            "Network policy changed after creation"
        );
        let all = self
            .request(
                Method::GET,
                &self.path("networkpolicies", None)?,
                None,
                false,
            )
            .await?
            .context("Policy list missing")?;
        let labels = self.labels(nonce);
        for other in all["items"].as_array().context("Invalid policy list")? {
            if other.pointer("/metadata/name").and_then(Value::as_str) == Some(name) {
                continue;
            }
            if selector_matches(&other["spec"]["podSelector"], &labels) {
                // Kubernetes policies add permissions. A default-deny baseline is
                // fine; an overlapping allow policy invalidates our isolation.
                for direction in ["ingress", "egress"] {
                    ensure!(
                        other["spec"][direction]
                            .as_array()
                            .is_none_or(Vec::is_empty),
                        "Overlapping network policy grants additional access"
                    );
                }
            }
        }
        Ok(())
    }
    pub async fn create(
        &self,
        owned: &OwnedSandbox,
        pvc: &str,
        image: &str,
        limits: &Resources,
        auth_digest: &str,
        readonly: bool,
    ) -> Result<String> {
        dns_label(&owned.name)?;
        dns_label(pvc)?;
        self.config.approve_image(image)?;
        ensure!(
            auth_digest.len() == 64 && auth_digest.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid runtime token digest"
        );
        let labels = self.labels(&owned.nonce);
        let mut ports = vec![
            json!({"name":"http-runtime","containerPort":8080}),
            json!({"name":"grpc-runtime","containerPort":9090}),
        ];
        ports.extend(
            self.config
                .preview_allowed_ports
                .iter()
                .enumerate()
                .map(|(i, port)| json!({"name":format!("preview-{i}"),"containerPort":port})),
        );
        let value = json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","metadata":{"name":owned.name,"namespace":self.config.namespace,"labels":labels},"spec":{"operatingMode":"Running","service":true,"podTemplate":{"metadata":{"labels":labels},"spec":{"runtimeClassName":self.config.runtime_class,"serviceAccountName":"cowork-workload","automountServiceAccountToken":false,"enableServiceLinks":false,"restartPolicy":"Never","terminationGracePeriodSeconds":20,"securityContext":{"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,"fsGroup":1000,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"runtime","image":image,"imagePullPolicy":"IfNotPresent","command":["/usr/local/bin/sandboxd"],"args":["--root-dir=/workspace",format!("--auth-token-sha256={auth_digest}")],"env":[],"ports":ports,"securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"resources":{"limits":{"memory":limits.memory_bytes.to_string(),"cpu":format!("{}m",limits.cpu_millis)},"requests":{"memory":limits.memory_bytes.to_string(),"cpu":format!("{}m",limits.cpu_millis)}},"readinessProbe":{"httpGet":{"path":"/v1/health","port":8080}},"volumeMounts":[{"name":"workspace","mountPath":"/workspace","readOnly":readonly},{"name":"tmp","mountPath":"/tmp"}]}],"volumes":[{"name":"workspace","persistentVolumeClaim":{"claimName":pvc,"readOnly":readonly}},{"name":"tmp","emptyDir":{"medium":"Memory","sizeLimit":(limits.memory_bytes / 2).min(limits.scratch_bytes).to_string()}}]}}}});
        let created = self
            .request(
                Method::POST,
                &self.path("sandboxes", None)?,
                Some(&value),
                false,
            )
            .await?
            .context("Sandbox receipt missing")?;
        self.verify_owned(&created, owned)?;
        Ok(field(&created, "/metadata/uid")?.into())
    }
    pub async fn wait_ready(
        &self,
        owned: &OwnedSandbox,
        image: &str,
    ) -> Result<(PodIdentity, String)> {
        let deadline = Instant::now() + Duration::from_secs(self.config.readiness_timeout_seconds);
        loop {
            let sb = self
                .get("sandboxes", &owned.name)
                .await?
                .context("Owned Sandbox disappeared")?;
            self.verify_owned(&sb, owned)?;
            ensure!(
                sb.pointer("/metadata/deletionTimestamp").is_none(),
                "Sandbox is deleting"
            );
            let generation = sb
                .pointer("/metadata/generation")
                .and_then(Value::as_u64)
                .context("Sandbox generation absent")?;
            let ready = sb
                .pointer("/status/conditions")
                .and_then(Value::as_array)
                .is_some_and(|a| {
                    a.iter().any(|c| {
                        c["type"] == "Ready"
                            && c["status"] == "True"
                            && c["observedGeneration"].as_u64() == Some(generation)
                    })
                });
            if ready {
                let pod_name = sb
                    .pointer("/metadata/annotations/agents.x-k8s.io~1pod-name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&owned.name);
                let pod = self
                    .get("pods", pod_name)
                    .await?
                    .context("Ready Pod absent")?;
                self.verify_pod(&pod, owned)?;
                verify_pod_template(&pod, &sb)?;
                ensure!(
                    field(&pod, "/spec/runtimeClassName")? == self.config.runtime_class
                        && pod.pointer("/spec/automountServiceAccountToken")
                            == Some(&Value::Bool(false))
                        && pod
                            .pointer("/spec/containers/0/image")
                            .and_then(Value::as_str)
                            == Some(image),
                    "Sandbox runtime or image changed"
                );
                let actual = field(&pod, "/status/containerStatuses/0/imageID")?;
                let digest = image
                    .rsplit_once("@sha256:")
                    .context("Image digest required")?
                    .1;
                ensure!(
                    actual.ends_with(&format!("sha256:{digest}")),
                    "Runtime resolved a different image digest"
                );
                let pod = PodIdentity {
                    name: pod_name.into(),
                    uid: field(&pod, "/metadata/uid")?.into(),
                    node: field(&pod, "/spec/nodeName")?.into(),
                };
                match self.service_target(&sb, owned, &pod).await {
                    Ok(host) => return Ok((pod, host)),
                    Err(error) if Instant::now() >= deadline => return Err(error),
                    Err(_) => {} // Read-only readiness propagation; never retry mutation.
                }
            }
            ensure!(
                Instant::now() < deadline,
                "Sandbox readiness remains unconfirmed"
            );
            sleep(Duration::from_millis(250)).await;
        }
    }
    /// One fresh identity observation for a narrow preview grant. Never waits for,
    /// starts, or follows a replacement Pod.
    pub async fn ready_target(&self, owned: &OwnedSandbox) -> Result<String> {
        let sb = self
            .get("sandboxes", &owned.name)
            .await?
            .context("Preview Sandbox absent")?;
        self.verify_owned(&sb, owned)?;
        ensure!(
            sb.pointer("/metadata/deletionTimestamp").is_none()
                && sb.pointer("/spec/operatingMode").and_then(Value::as_str) == Some("Running"),
            "Preview Sandbox not running"
        );
        let generation = sb
            .pointer("/metadata/generation")
            .and_then(Value::as_u64)
            .context("Sandbox generation absent")?;
        ensure!(
            sb.pointer("/status/conditions")
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().any(|c| c["type"] == "Ready"
                    && c["status"] == "True"
                    && c["observedGeneration"].as_u64() == Some(generation))),
            "Preview Sandbox is not Ready"
        );
        let expected = owned.pod.as_ref().context("Preview Pod UID unavailable")?;
        let pod = self
            .get("pods", &expected.name)
            .await?
            .context("Preview Pod absent")?;
        self.verify_pod(&pod, owned)?;
        ensure!(
            pod.pointer("/metadata/deletionTimestamp").is_none()
                && pod.pointer("/status/phase").and_then(Value::as_str) == Some("Running")
                && pod
                    .pointer("/status/containerStatuses")
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty() && a.iter().all(|s| s["ready"] == true)),
            "Preview Pod is not Ready"
        );
        self.service_target(&sb, owned, expected).await
    }
    async fn service_target(
        &self,
        sandbox: &Value,
        owned: &OwnedSandbox,
        pod: &PodIdentity,
    ) -> Result<String> {
        let host = format!(
            "{}.{}.svc.{}",
            owned.name, self.config.namespace, self.config.cluster_domain
        );
        ensure!(
            field(sandbox, "/status/serviceFQDN")? == host,
            "Unexpected Sandbox Service DNS"
        );
        let service = self
            .request(
                Method::GET,
                &format!(
                    "/api/v1/namespaces/{}/services/{}",
                    self.config.namespace, owned.name
                ),
                None,
                false,
            )
            .await?
            .context("Sandbox Service absent")?;
        let current_pod = self
            .get("pods", &pod.name)
            .await?
            .context("Service Pod absent")?;
        self.verify_pod(&current_pod, owned)?;
        verify_pod_template(&current_pod, sandbox)?;
        ensure!(
            field(&current_pod, "/metadata/uid")? == pod.uid,
            "Service Pod UID changed"
        );
        let slices = self.request(Method::GET,
            &format!("/apis/discovery.k8s.io/v1/namespaces/{}/endpointslices?labelSelector=kubernetes.io%2Fservice-name%3D{}", self.config.namespace, owned.name), None, false)
            .await?.context("Sandbox EndpointSlices absent")?;
        verify_service_binding(&service, &slices, &current_pod, owned)?;
        Ok(host)
    }
    fn verify_pod(&self, pod: &Value, owned: &OwnedSandbox) -> Result<()> {
        ensure!(
            field(pod, "/metadata/namespace")? == self.config.namespace
                && field(pod, "/metadata/labels/cowork.local~1runner")?
                    == self.config.worker_label_value
                && field(pod, "/metadata/labels/cowork.local~1nonce")? == owned.nonce,
            "Foreign Pod labels"
        );
        let uid = owned.uid.as_deref().context("Sandbox UID required")?;
        ensure!(
            pod.pointer("/metadata/ownerReferences")
                .and_then(Value::as_array)
                .is_some_and(|a| a
                    .iter()
                    .any(|o| o["uid"] == uid && o["kind"] == "Sandbox" && o["controller"] == true)),
            "Foreign Pod controller"
        );
        if let Some(expected) = &owned.pod {
            ensure!(
                field(pod, "/metadata/uid")? == expected.uid
                    && field(pod, "/metadata/name")? == expected.name,
                "Owned Pod identity changed"
            );
        }
        ensure!(
            pod.pointer("/spec/containers")
                .and_then(Value::as_array)
                .is_some_and(|a| a.len() == 1)
                && pod
                    .pointer("/spec/initContainers")
                    .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty))
                && pod
                    .pointer("/spec/ephemeralContainers")
                    .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty)),
            "Injected workload container denied"
        );
        for flag in ["hostNetwork", "hostPID", "hostIPC"] {
            ensure!(
                !pod.pointer(&format!("/spec/{flag}"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                "Host namespace denied"
            );
        }
        ensure!(
            pod.pointer("/spec/automountServiceAccountToken") == Some(&Value::Bool(false))
                && pod.pointer("/spec/enableServiceLinks") == Some(&Value::Bool(false))
                && pod.pointer("/spec/containers/0/securityContext/allowPrivilegeEscalation")
                    == Some(&Value::Bool(false))
                && pod.pointer("/spec/containers/0/securityContext/readOnlyRootFilesystem")
                    == Some(&Value::Bool(true))
                && !pod
                    .pointer("/spec/containers/0/securityContext/privileged")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                && pod
                    .pointer("/spec/containers/0/securityContext/capabilities/drop")
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.iter().any(|v| v == "ALL")),
            "Pod security controls changed"
        );
        let volumes = pod
            .pointer("/spec/volumes")
            .and_then(Value::as_array)
            .context("Pod volumes missing")?;
        ensure!(
            volumes.len() == 2
                && volumes
                    .iter()
                    .all(|v| matches!(v["name"].as_str(), Some("workspace" | "tmp"))
                        && v.get("hostPath").is_none()
                        && v.get("secret").is_none()
                        && v.get("projected").is_none()),
            "Foreign host/credential volume denied"
        );
        Ok(())
    }
    pub async fn fence_pod(&self, owned: &OwnedSandbox) -> Result<()> {
        let expected = owned.pod.as_ref().context("Pod identity not journaled")?;
        let pod = self
            .get("pods", &expected.name)
            .await?
            .context("Pod absent before execution")?;
        self.verify_pod(&pod, owned)?;
        let sandbox = self
            .get("sandboxes", &owned.name)
            .await?
            .context("Sandbox absent before execution fence")?;
        self.verify_owned(&sandbox, owned)?;
        verify_pod_template(&pod, &sandbox)?;
        ensure!(
            pod.pointer("/metadata/deletionTimestamp").is_none(),
            "Pod is deleting"
        );
        let mut finalizers = pod
            .pointer("/metadata/finalizers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        ensure!(
            !finalizers.iter().any(|f| f == STOP_FINALIZER),
            "Pod already fenced; never replay execution"
        );
        finalizers.push(json!(STOP_FINALIZER));
        let patch = json!([{"op":"test","path":"/metadata/uid","value":expected.uid},{"op":"test","path":"/metadata/resourceVersion","value":field(&pod,"/metadata/resourceVersion")?},{"op":"add","path":"/metadata/finalizers","value":finalizers}]);
        self.request(
            Method::PATCH,
            &self.path("pods", Some(&expected.name))?,
            Some(&patch),
            true,
        )
        .await?;
        Ok(())
    }
    async fn mode(&self, owned: &OwnedSandbox, mode: &str) -> Result<()> {
        let sb = self
            .get("sandboxes", &owned.name)
            .await?
            .context("Owned Sandbox absent")?;
        self.verify_owned(&sb, owned)?;
        let patch = json!([{"op":"test","path":"/metadata/uid","value":field(&sb,"/metadata/uid")?},{"op":"test","path":"/metadata/resourceVersion","value":field(&sb,"/metadata/resourceVersion")?},{"op":"add","path":"/spec/operatingMode","value":mode}]);
        self.request(
            Method::PATCH,
            &self.path("sandboxes", Some(&owned.name))?,
            Some(&patch),
            true,
        )
        .await?;
        Ok(())
    }
    pub async fn resume(&self, owned: &OwnedSandbox) -> Result<()> {
        self.mode(owned, "Running").await
    }
    /// Positive kubelet termination, not API object disappearance. Persist the receipt
    /// before releasing our Pod finalizer. A partitioned node remains fenced.
    pub async fn suspend(
        &self,
        owned: &OwnedSandbox,
        mut persist: impl FnMut(&StopReceipt) -> Result<()>,
    ) -> Result<Option<StopReceipt>> {
        let Some(sb) = self.get("sandboxes", &owned.name).await? else {
            ensure!(
                owned.pod.is_none() || owned.stopped.is_some(),
                "Sandbox vanished without physical-stop receipt"
            );
            return Ok(owned.stopped.clone());
        };
        self.verify_owned(&sb, owned)?;
        self.mode(owned, "Suspended").await?;
        let deadline = Instant::now() + Duration::from_secs(self.config.stop_timeout_seconds);
        let mut receipt = owned.stopped.clone();
        let name = owned
            .pod
            .as_ref()
            .map(|p| p.name.as_str())
            .unwrap_or(&owned.name);
        loop {
            match self.get("pods", name).await? {
                None => {
                    ensure!(
                        owned.pod.is_none() || receipt.is_some(),
                        "Pod vanished without kubelet termination evidence"
                    );
                    let sb = self
                        .get("sandboxes", &owned.name)
                        .await?
                        .context("Sandbox disappeared while suspending")?;
                    self.verify_owned(&sb, owned)?;
                    let suspended = sb
                        .pointer("/status/conditions")
                        .and_then(Value::as_array)
                        .is_some_and(|a| {
                            a.iter()
                                .any(|c| c["type"] == "Suspended" && c["status"] == "True")
                        });
                    if suspended {
                        return Ok(receipt);
                    }
                }
                Some(pod) => {
                    self.verify_pod(&pod, owned)?;
                    if let Some(ids) = terminated_containers(&pod) {
                        let stopped = StopReceipt {
                            sandbox_uid: field(&sb, "/metadata/uid")?.into(),
                            pod: PodIdentity {
                                name: name.into(),
                                uid: field(&pod, "/metadata/uid")?.into(),
                                node: field(&pod, "/spec/nodeName")?.into(),
                            },
                            terminated_container_ids: ids,
                        };
                        persist(&stopped)?;
                        receipt = Some(stopped);
                        let finalizers = pod
                            .pointer("/metadata/finalizers")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        if finalizers.iter().any(|f| f == STOP_FINALIZER) {
                            let kept: Vec<_> = finalizers
                                .into_iter()
                                .filter(|v| v != STOP_FINALIZER)
                                .collect();
                            let patch = json!([{"op":"test","path":"/metadata/uid","value":field(&pod,"/metadata/uid")?},{"op":"test","path":"/metadata/resourceVersion","value":field(&pod,"/metadata/resourceVersion")?},{"op":"replace","path":"/metadata/finalizers","value":kept}]);
                            self.request(
                                Method::PATCH,
                                &self.path("pods", Some(name))?,
                                Some(&patch),
                                true,
                            )
                            .await?;
                        }
                    }
                }
            }
            ensure!(
                Instant::now() < deadline,
                "Kubelet physical stop remains unconfirmed"
            );
            sleep(Duration::from_millis(250)).await;
        }
    }
    pub async fn delete_sandbox(&self, owned: &OwnedSandbox) -> Result<()> {
        let Some(sb) = self.get("sandboxes", &owned.name).await? else {
            return Ok(());
        };
        self.verify_owned(&sb, owned)?;
        ensure!(
            sb.pointer("/spec/operatingMode").and_then(Value::as_str) == Some("Suspended"),
            "Must suspend before removal"
        );
        ensure!(
            self.get(
                "pods",
                owned
                    .pod
                    .as_ref()
                    .map(|p| p.name.as_str())
                    .unwrap_or(&owned.name)
            )
            .await?
            .is_none(),
            "Owned Pod still present"
        );
        let body = json!({"apiVersion":"v1","kind":"DeleteOptions","preconditions":{"uid":field(&sb,"/metadata/uid")?},"propagationPolicy":"Foreground"});
        self.request(
            Method::DELETE,
            &self.path("sandboxes", Some(&owned.name))?,
            Some(&body),
            false,
        )
        .await?;
        Ok(())
    }
    pub async fn delete_app_resource(
        &self,
        kind: &str,
        name: &str,
        nonce: &str,
        uid: Option<&str>,
    ) -> Result<()> {
        ensure!(
            matches!(kind, "persistentvolumeclaims" | "networkpolicies"),
            "Unsupported app resource"
        );
        let Some(value) = self.get(kind, name).await? else {
            return Ok(());
        };
        let owned = OwnedSandbox {
            name: name.into(),
            nonce: nonce.into(),
            uid: uid.map(str::to_owned),
            pod: None,
            stopped: None,
        };
        self.verify_owned(&value, &owned)?;
        ensure!(
            value
                .pointer("/metadata/ownerReferences")
                .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty)),
            "App resource acquired foreign owner"
        );
        let body = json!({"apiVersion":"v1","kind":"DeleteOptions","preconditions":{"uid":field(&value,"/metadata/uid")?}});
        self.request(
            Method::DELETE,
            &self.path(kind, Some(name))?,
            Some(&body),
            false,
        )
        .await?;
        Ok(())
    }
}
/// Every regular and init container must have positively reported termination.
pub fn terminated_containers(pod: &Value) -> Option<Vec<String>> {
    let mut ids = Vec::new();
    for (spec, status) in [
        ("containers", "containerStatuses"),
        ("initContainers", "initContainerStatuses"),
    ] {
        let expected = pod
            .pointer(&format!("/spec/{spec}"))
            .and_then(Value::as_array);
        if let Some(expected) = expected.filter(|a| !a.is_empty()) {
            let statuses = pod
                .pointer(&format!("/status/{status}"))
                .and_then(Value::as_array)?;
            for container in expected {
                let s = statuses.iter().find(|s| s["name"] == container["name"])?;
                s.pointer("/state/terminated/finishedAt")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())?;
                let id = s["containerID"].as_str().filter(|s| !s.is_empty())?;
                ids.push(id.into());
            }
        }
    }
    if ids.is_empty() { None } else { Some(ids) }
}
fn normalized_policy(spec: &Value) -> Value {
    let mut result = spec.clone();
    for key in ["ingress", "egress"] {
        if result.get(key).is_none() {
            result[key] = json!([]);
        }
    }
    result
}
fn selector_matches(selector: &Value, labels: &BTreeMap<String, String>) -> bool {
    if let Some(required) = selector["matchLabels"].as_object()
        && required
            .iter()
            .any(|(key, value)| labels.get(key).map(String::as_str) != value.as_str())
    {
        return false;
    }
    if let Some(expressions) = selector["matchExpressions"].as_array() {
        for rule in expressions {
            let key = rule["key"].as_str().unwrap_or("");
            let found = labels.get(key);
            let values = rule["values"].as_array();
            let in_values = found
                .is_some_and(|v| values.is_some_and(|a| a.iter().any(|i| i.as_str() == Some(v))));
            let matched = match rule["operator"].as_str() {
                Some("In") => in_values,
                Some("NotIn") => !in_values,
                Some("Exists") => found.is_some(),
                Some("DoesNotExist") => found.is_none(),
                _ => true,
            };
            if !matched {
                return false;
            }
        }
    }
    true
}

/// Verify the actual immutable Pod inputs before execution or preview. Defaults
/// omitted by the Kubernetes JSON encoder are normalized only for optional false
/// flags and empty lists; identity, source names and mount paths stay exact.
fn verify_pod_template(pod: &Value, sandbox: &Value) -> Result<()> {
    let desired = &sandbox["spec"]["podTemplate"]["spec"];
    let actual = &pod["spec"];
    ensure!(
        actual["serviceAccountName"] == "cowork-workload"
            && desired["serviceAccountName"] == "cowork-workload",
        "Unexpected workload ServiceAccount"
    );
    let container = &actual["containers"][0];
    let expected = &desired["containers"][0];
    let empty = |v: &Value| v.is_null() || v.as_array().is_some_and(Vec::is_empty);
    let effective = |key: &str| {
        container["securityContext"]
            .get(key)
            .or_else(|| actual["securityContext"].get(key))
            .cloned()
            .unwrap_or(Value::Null)
    };
    ensure!(
        effective("runAsNonRoot") == true
            && effective("runAsUser") == 1000
            && effective("runAsGroup") == 1000
            && effective("seccompProfile")["type"] == "RuntimeDefault"
            && actual["securityContext"]["fsGroup"] == 1000
            && empty(&actual["securityContext"]["supplementalGroups"])
            && container["securityContext"]["allowPrivilegeEscalation"] == false
            && container["securityContext"]["readOnlyRootFilesystem"] == true
            && container["securityContext"]["privileged"] != true
            && empty(&container["securityContext"]["capabilities"]["add"]),
        "Pod effective identity or privilege controls changed"
    );
    for key in ["image", "command", "args"] {
        ensure!(
            container[key] == expected[key],
            "Pod runtime executable or image changed"
        );
    }
    ensure!(
        (empty(&container["env"]) && empty(&expected["env"]))
            || container["env"] == expected["env"],
        "Injected runtime environment denied"
    );
    ensure!(
        empty(&container["envFrom"]) && empty(&container["volumeDevices"]),
        "Injected runtime input denied"
    );
    let mounts = container["volumeMounts"]
        .as_array()
        .context("Pod mounts absent")?;
    let expected_mounts = expected["volumeMounts"]
        .as_array()
        .context("Expected Pod mounts absent")?;
    ensure!(
        mounts.len() == expected_mounts.len(),
        "Injected Pod mount denied"
    );
    for mount in mounts {
        let expected = expected_mounts
            .iter()
            .find(|m| m["name"] == mount["name"])
            .context("Foreign Pod mount")?;
        ensure!(
            mount["mountPath"] == expected["mountPath"]
                && mount["readOnly"].as_bool().unwrap_or(false)
                    == expected["readOnly"].as_bool().unwrap_or(false)
                && mount["subPath"].as_str().unwrap_or("").is_empty()
                && mount["subPathExpr"].as_str().unwrap_or("").is_empty()
                && mount["mountPropagation"]
                    .as_str()
                    .is_none_or(|s| s == "None"),
            "Pod workspace mount changed"
        );
    }
    let volumes = actual["volumes"].as_array().context("Pod volumes absent")?;
    let expected_volumes = desired["volumes"]
        .as_array()
        .context("Expected Pod volumes absent")?;
    ensure!(
        volumes.len() == expected_volumes.len(),
        "Injected Pod volume denied"
    );
    for volume in volumes {
        let expected = expected_volumes
            .iter()
            .find(|v| v["name"] == volume["name"])
            .context("Foreign Pod volume")?;
        if volume["name"] == "workspace" {
            ensure!(
                volume["persistentVolumeClaim"]["claimName"]
                    == expected["persistentVolumeClaim"]["claimName"]
                    && volume["persistentVolumeClaim"]["readOnly"]
                        .as_bool()
                        .unwrap_or(false)
                        == expected["persistentVolumeClaim"]["readOnly"]
                            .as_bool()
                            .unwrap_or(false),
                "Pod mounted a different workspace PVC"
            );
        } else {
            ensure!(
                volume["name"] == "tmp" && volume["emptyDir"]["medium"] == "Memory",
                "Pod temporary volume changed"
            );
        }
    }
    Ok(())
}

fn verify_service_binding(
    service: &Value,
    slices: &Value,
    pod: &Value,
    owned: &OwnedSandbox,
) -> Result<()> {
    let sandbox_uid = owned.uid.as_deref().context("Sandbox UID absent")?;
    let pod_uid = field(pod, "/metadata/uid")?;
    let service_uid = field(service, "/metadata/uid")?;
    ensure!(
        field(service, "/metadata/name")? == owned.name
            && service.pointer("/metadata/deletionTimestamp").is_none()
            && field(service, "/spec/clusterIP")? == "None"
            && service
                .pointer("/spec/publishNotReadyAddresses")
                .is_none_or(|v| v == false)
            && service["metadata"]["ownerReferences"]
                .as_array()
                .is_some_and(|refs| refs.iter().any(|r| r["kind"] == "Sandbox"
                    && r["uid"] == sandbox_uid
                    && r["controller"] == true)),
        "Foreign or unsafe Sandbox Service"
    );
    let tracking = field(pod, "/metadata/labels/agents.x-k8s.io~1sandbox-name-hash")?;
    ensure!(
        !tracking.is_empty()
            && service["spec"]["selector"] == json!({"agents.x-k8s.io/sandbox-name-hash":tracking}),
        "Sandbox Service selector changed"
    );
    let mut addresses = pod["status"]["podIPs"]
        .as_array()
        .map(|ips| {
            ips.iter()
                .filter_map(|v| v["ip"].as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(ip) = pod["status"]["podIP"].as_str() {
        addresses.push(ip);
    }
    ensure!(!addresses.is_empty(), "Pod IP absent");
    let mut ready = false;
    for slice in slices["items"]
        .as_array()
        .context("Invalid EndpointSlice list")?
    {
        ensure!(
            slice["metadata"]["ownerReferences"]
                .as_array()
                .is_some_and(|refs| refs.iter().any(|r| r["kind"] == "Service"
                    && r["uid"] == service_uid
                    && r["controller"] == true)),
            "Foreign EndpointSlice owner"
        );
        for endpoint in slice["endpoints"]
            .as_array()
            .context("Invalid EndpointSlice endpoints")?
        {
            if endpoint["conditions"]["ready"] == false {
                continue;
            }
            ensure!(
                endpoint["conditions"]["terminating"] != true
                    && endpoint["targetRef"]["kind"] == "Pod"
                    && endpoint["targetRef"]["uid"] == pod_uid
                    && endpoint["targetRef"]["name"]
                        == owned
                            .pod
                            .as_ref()
                            .map(|p| p.name.as_str())
                            .unwrap_or_else(|| pod["metadata"]["name"].as_str().unwrap_or("")),
                "Service endpoint targets a different Pod UID"
            );
            let ips = endpoint["addresses"]
                .as_array()
                .context("Endpoint IP absent")?;
            ensure!(
                !ips.is_empty()
                    && ips
                        .iter()
                        .all(|ip| ip.as_str().is_some_and(|s| addresses.contains(&s))),
                "Service endpoint IP differs from owned Pod"
            );
            ready = true;
        }
    }
    ensure!(ready, "Owned Pod has no ready Service endpoint");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    fn configuration(dir: &std::path::Path) -> Configuration {
        Configuration {
            api_url: "https://kubernetes.default.svc/".into(),
            namespace: "tenant".into(),
            namespace_uid: "namespace-uid".into(),
            cluster_domain: cluster_domain(),
            approved_images: vec![format!(
                "registry.example/runtime@sha256:{}",
                "a".repeat(64)
            )],
            token_file: dir.join("token"),
            ca_file: dir.join("ca"),
            runtime_class: "kata-qemu".into(),
            runtime_handler: "kata-qemu".into(),
            storage_class: "qualified".into(),
            pvc_access_mode: "ReadWriteOncePod".into(),
            worker_namespace: "control".into(),
            worker_label_value: "a".repeat(32),
            web_proxy_policies: BTreeMap::new(),
            egress_probe_address: None,
            preview_base_domain: None,
            preview_allowed_ports: preview_ports(),
            preview_validator_token_file: None,
            preview_gateway: None,
            idle_suspend_seconds: idle_default(),
            max_active_sessions: active_default(),
            readiness_timeout_seconds: 1,
            stop_timeout_seconds: 1,
        }
    }
    #[test]
    fn operator_config_rejects_ambient_and_unpinned_authority() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = configuration(dir.path());
        assert!(c.validate().is_ok());
        c.api_url = "http://kubernetes.default.svc/".into();
        assert!(c.validate().is_err());
        c.api_url = "https://user:secret@cluster/".into();
        assert!(c.validate().is_err());
        c = configuration(dir.path());
        c.namespace_uid.clear();
        assert!(c.validate().is_err());
        c = configuration(dir.path());
        c.worker_label_value = "a".repeat(64);
        assert!(c.validate().is_err());
        c = configuration(dir.path());
        assert!(c.approve_image(&c.approved_images[0]).is_ok());
        assert!(
            c.approve_image(&format!("attacker/runtime@sha256:{}", "a".repeat(64)))
                .is_err()
        );
        c.approved_images.clear();
        assert!(c.validate().is_err());
    }
    #[test]
    fn proxy_environment_is_accepted_by_case_insensitive_supervisor_validation() {
        let policy = WebProxyPolicy {
            url: "http://proxy.proxy.svc.cluster.local:3128/".into(),
            namespace: "proxy".into(),
            pod_labels: BTreeMap::new(),
            port: 3128,
            probe_url: "http://example.com/".into(),
            direct_probe_address: "192.0.2.1:80".into(),
        };
        let environment = policy.environment();
        assert_eq!(environment.len(), 4);
        assert_eq!(
            environment
                .keys()
                .map(|k| k.to_ascii_uppercase())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            environment.len()
        );
        assert_eq!(environment["HTTP_PROXY"], policy.url);
        assert_eq!(environment["HTTPS_PROXY"], policy.url);
        assert_eq!(environment["ALL_PROXY"], policy.url);
        assert!(environment["NO_PROXY"].is_empty());
    }
    #[test]
    fn all_container_termination_must_be_positive() {
        let mut pod = json!({"spec":{"containers":[{"name":"runtime"}]},"status":{"containerStatuses":[{"name":"runtime","containerID":"containerd://original","state":{"running":{}}}]}});
        assert!(terminated_containers(&pod).is_none());
        pod["status"]["containerStatuses"][0]["state"] =
            json!({"terminated":{"finishedAt":"2026-10-17T00:00:00Z"}});
        assert_eq!(
            terminated_containers(&pod).unwrap(),
            vec!["containerd://original"]
        );
        pod["spec"]["initContainers"] = json!([{"name":"injected"}]);
        assert!(terminated_containers(&pod).is_none());
    }
    #[test]
    fn service_binding_rejects_replaced_endpoint_and_selector() {
        let owned = OwnedSandbox {
            name: "sandbox".into(),
            nonce: "a".repeat(32),
            uid: Some("sandbox-uid".into()),
            pod: None,
            stopped: None,
        };
        let pod = json!({"metadata":{"name":"sandbox","uid":"pod-uid","labels":{"agents.x-k8s.io/sandbox-name-hash":"tracking"}},"status":{"podIP":"10.0.0.1"}});
        let mut service = json!({"metadata":{"name":"sandbox","uid":"service-uid","ownerReferences":[{"kind":"Sandbox","uid":"sandbox-uid","controller":true}]},"spec":{"clusterIP":"None","selector":{"agents.x-k8s.io/sandbox-name-hash":"tracking"}}});
        let mut slices = json!({"items":[{"metadata":{"ownerReferences":[{"kind":"Service","uid":"service-uid","controller":true}]},"endpoints":[{"conditions":{"ready":true},"targetRef":{"kind":"Pod","name":"sandbox","uid":"pod-uid"},"addresses":["10.0.0.1"]}]}]});
        assert!(verify_service_binding(&service, &slices, &pod, &owned).is_ok());
        slices["items"][0]["endpoints"][0]["targetRef"]["uid"] = json!("replacement-pod");
        assert!(verify_service_binding(&service, &slices, &pod, &owned).is_err());
        slices["items"][0]["endpoints"][0]["targetRef"]["uid"] = json!("pod-uid");
        service["spec"]["selector"] = json!({});
        assert!(verify_service_binding(&service, &slices, &pod, &owned).is_err());
    }
    #[test]
    fn actual_pod_rejects_injected_privileges_and_changed_workspace_mounts() {
        let spec = json!({"serviceAccountName":"cowork-workload","securityContext":{"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,"fsGroup":1000,"seccompProfile":{"type":"RuntimeDefault"}},
            "containers":[{"image":"approved@sha256:digest","command":["/usr/local/bin/sandboxd"],"args":["--root-dir=/workspace","--auth-token-sha256=expected"],"env":[],
                "securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},
                "volumeMounts":[{"name":"workspace","mountPath":"/workspace","readOnly":false},{"name":"tmp","mountPath":"/tmp"}]}],
            "volumes":[{"name":"workspace","persistentVolumeClaim":{"claimName":"exact-pvc","readOnly":false}},{"name":"tmp","emptyDir":{"medium":"Memory","sizeLimit":"256Mi"}}]});
        let original = json!({"spec":spec.clone()});
        let sandbox = json!({"spec":{"podTemplate":{"spec":spec}}});
        assert!(verify_pod_template(&original, &sandbox).is_ok());
        let changes = [
            (
                "/spec/containers/0/securityContext/capabilities/add",
                json!(["SYS_ADMIN"]),
            ),
            ("/spec/containers/0/securityContext/runAsUser", json!(0)),
            (
                "/spec/containers/0/securityContext/allowPrivilegeEscalation",
                json!(true),
            ),
            (
                "/spec/containers/0/volumeMounts/0/mountPath",
                json!("/other"),
            ),
            ("/spec/containers/0/volumeMounts/0/readOnly", json!(true)),
            (
                "/spec/volumes/0/persistentVolumeClaim/claimName",
                json!("foreign-pvc"),
            ),
            (
                "/spec/containers/0/args",
                json!(["--auth-token-sha256=attacker"]),
            ),
        ];
        for (path, value) in changes {
            let mut changed = original.clone();
            // JSON pointer mutation requires the leaf to exist; create optional
            // security fields explicitly for the admission-injection cases.
            if path.ends_with("/add") {
                changed["spec"]["containers"][0]["securityContext"]["capabilities"]["add"] = value;
            } else if path.ends_with("/runAsUser") {
                changed["spec"]["containers"][0]["securityContext"]["runAsUser"] = value;
            } else {
                *changed.pointer_mut(path).unwrap() = value;
            }
            assert!(
                verify_pod_template(&changed, &sandbox).is_err(),
                "Accepted mutated Pod field {path}"
            );
        }
    }
    #[tokio::test]
    async fn unapproved_image_is_rejected_before_any_mutation() {
        let (plane, owned, mutations, server, _dir) =
            fixture(true, Arc::new(AtomicBool::new(false))).await;
        let limits = Resources {
            memory_bytes: 1024,
            cpu_millis: 100,
            scratch_bytes: 1024,
        };
        assert!(
            plane
                .create(
                    &owned,
                    "workspace",
                    &format!("attacker/runtime@sha256:{}", "a".repeat(64)),
                    &limits,
                    &"b".repeat(64),
                    false
                )
                .await
                .is_err()
        );
        assert_eq!(mutations.load(Ordering::SeqCst), 0);
        server.abort();
    }
    async fn fixture(
        terminated: bool,
        persisted: Arc<AtomicBool>,
    ) -> (
        ControlPlane,
        OwnedSandbox,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
        tempfile::TempDir,
    ) {
        use axum::{
            Router,
            body::Body,
            http::{Request, Response},
            routing::any,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut config = configuration(dir.path());
        std::fs::write(&config.token_file, "t".repeat(64)).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        config.api_url = format!("http://{}/", listener.local_addr().unwrap());
        let removed = Arc::new(AtomicBool::new(false));
        let patched = Arc::new(AtomicUsize::new(0));
        let mutations = patched.clone();
        let router=Router::new().fallback(any(move|req:Request<Body>|{let removed=removed.clone();let patched=patched.clone();let persisted=persisted.clone();async move{
            assert_eq!(req.headers().get("authorization").unwrap(),format!("Bearer {}","t".repeat(64)).as_str());
            let path=req.uri().path();let is_patch=req.method()==Method::PATCH;
            if path.contains("/sandboxes/") {
                if is_patch{patched.fetch_add(1,Ordering::SeqCst);}
                let sb=json!({"metadata":{"name":"sb","namespace":"tenant","uid":"sb-uid","resourceVersion":"1","labels":{"cowork.local/runner":"a".repeat(32),"cowork.local/nonce":"nonce"}},"spec":{"operatingMode":"Suspended"},"status":{"conditions":[{"type":"Suspended","status":if removed.load(Ordering::SeqCst){"True"}else{"False"}}]}});
                return Response::builder().header("content-type","application/json").body(Body::from(sb.to_string())).unwrap();
            }
            if removed.load(Ordering::SeqCst){return Response::builder().status(404).body(Body::empty()).unwrap()}
            let pod=json!({"metadata":{"name":"sb","namespace":"tenant","uid":"pod-uid","resourceVersion":"2","finalizers":[STOP_FINALIZER],"labels":{"cowork.local/runner":"a".repeat(32),"cowork.local/nonce":"nonce"},"ownerReferences":[{"kind":"Sandbox","uid":"sb-uid","controller":true}]},"spec":{"nodeName":"node-1","automountServiceAccountToken":false,"enableServiceLinks":false,"containers":[{"name":"runtime","securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}}}],"volumes":[{"name":"workspace","persistentVolumeClaim":{"claimName":"data"}},{"name":"tmp","emptyDir":{}}]},"status":{"containerStatuses":[{"name":"runtime","containerID":"containerd://physical","state":if terminated{json!({"terminated":{"finishedAt":"2026-10-17T00:00:00Z"}})}else{json!({"running":{}})}}]}});
            if is_patch{assert!(persisted.load(Ordering::SeqCst),"finalizer removed before durable physical receipt");removed.store(true,Ordering::SeqCst);patched.fetch_add(1,Ordering::SeqCst);}
            Response::builder().header("content-type","application/json").body(Body::from(pod.to_string())).unwrap()
        }}));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let plane = ControlPlane {
            config,
            client: Client::builder().no_proxy().build().unwrap(),
        };
        let owned = OwnedSandbox {
            name: "sb".into(),
            nonce: "nonce".into(),
            uid: Some("sb-uid".into()),
            pod: Some(PodIdentity {
                name: "sb".into(),
                uid: "pod-uid".into(),
                node: "node-1".into(),
            }),
            stopped: None,
        };
        (plane, owned, mutations, server, dir)
    }
    #[tokio::test]
    async fn finalizer_release_follows_kubelet_and_durable_receipt() {
        let persisted = Arc::new(AtomicBool::new(false));
        let (plane, owned, mutations, server, _dir) = fixture(true, persisted.clone()).await;
        let receipt = plane
            .suspend(&owned, |_| {
                persisted.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(receipt.pod.uid, "pod-uid");
        assert_eq!(mutations.load(Ordering::SeqCst), 2);
        server.abort();
    }
    #[tokio::test]
    async fn running_or_partitioned_pod_keeps_fence_and_no_stop_receipt() {
        let persisted = Arc::new(AtomicBool::new(false));
        let (plane, owned, mutations, server, _dir) = fixture(false, persisted.clone()).await;
        assert!(
            plane
                .suspend(&owned, |_| {
                    persisted.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .await
                .is_err()
        );
        assert!(!persisted.load(Ordering::SeqCst));
        assert_eq!(mutations.load(Ordering::SeqCst), 1);
        server.abort();
    }
    #[tokio::test]
    async fn replacement_sandbox_uid_is_never_mutated() {
        let (plane, mut owned, mutations, server, _dir) =
            fixture(true, Arc::new(AtomicBool::new(false))).await;
        owned.uid = Some("replacement".into());
        assert!(plane.suspend(&owned, |_| Ok(())).await.is_err());
        assert_eq!(mutations.load(Ordering::SeqCst), 0);
        server.abort();
    }
}
