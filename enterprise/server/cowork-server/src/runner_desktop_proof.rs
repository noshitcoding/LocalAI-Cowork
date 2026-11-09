//! Private physical desktop identity with distinct Docker and Kubernetes domains.
use anyhow::{ensure, Context, Result};
use axum::http::{HeaderMap, Request};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use cowork_contracts::{
    KubernetesJobIdentity, SandboxDesktopSessionResult, SandboxDesktopSessionSpec,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Read, sync::Arc, time::Duration};
use tokio::{net::TcpStream, time::Instant};
use tokio_rustls::{
    rustls::{
        self,
        pki_types::{CertificateDer, ServerName},
        RootCertStore,
    },
    TlsConnector,
};
use uuid::Uuid;

pub(crate) const PROBE_PATH: &str = "/v1/desktop-session-probe";
pub(crate) const MAX_RESPONSE: usize = 64 * 1024;
const MAX_BINDING: usize = 8 * 1024;
const MAX_CA: usize = 1024 * 1024;
const FRESH_PROBE: Duration = Duration::from_secs(5);
const CREATE_DOMAIN: &[u8] = b"open-cowork/runner-desktop-create/v1\0";
const PROBE_DOMAIN: &[u8] = b"open-cowork/runner-desktop-probe/v1\0";
const STOP_DOMAIN: &[u8] = b"open-cowork/runner-desktop-stop/v1\0";
const KUBE_CREATE_DOMAIN: &[u8] = b"open-cowork/runner-kubernetes-desktop-create/v2\0";
const KUBE_PROBE_DOMAIN: &[u8] = b"open-cowork/runner-kubernetes-desktop-probe/v2\0";
const KUBE_STOP_DOMAIN: &[u8] = b"open-cowork/runner-kubernetes-desktop-stop/v2\0";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct JobBinding {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kubernetes: Option<KubernetesJobIdentity>,
}
impl JobBinding {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.version, 1 | 2)
                && [
                    self.runner_id,
                    self.reservation_id,
                    self.attempt_id,
                    self.run_id,
                    self.session_id,
                    self.lease_token
                ]
                .iter()
                .all(|id| !id.is_nil())
                && self.workspace_id.is_none_or(|id| !id.is_nil()),
            "invalid desktop job identity"
        );
        if self.version == 1 {
            ensure!(
                self.kubernetes.is_none(),
                "unexpected Kubernetes identity on Docker job"
            );
            ensure!(
                self.container_id.len() == 64
                    && self
                        .container_id
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "invalid immutable desktop container ID"
            );
            ensure!(
                !self.engine_id.is_empty()
                    && self.engine_id.len() <= 256
                    && self.engine_id.bytes().all(|byte| byte.is_ascii_graphic()),
                "invalid desktop engine identity"
            );
        }
        let resource = |value: &str| {
            !value.is_empty()
                && value.len() <= 256
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        };
        if self.version == 1 {
            ensure!(
                resource(&self.scope_prefix)
                    && self.scope_prefix.len() <= 96
                    && self.container_name
                        == format!("{}desktop-{}", self.scope_prefix, self.session_id.simple())
                    && resource(&self.workspace_volume)
                    && self.workspace_volume.starts_with(&self.scope_prefix),
                "invalid desktop resource scope"
            );
        } else {
            let native = self
                .kubernetes
                .as_ref()
                .context("native desktop physical identity missing")?;
            native.validate().map_err(anyhow::Error::msg)?;
            let workspace = self.workspace_id.unwrap_or(self.run_id);
            let claim = hex::encode(Sha256::digest(
                format!("{}:{}", native.generation, workspace).as_bytes(),
            ));
            ensure!(
                self.engine_id.is_empty()
                    && self.container_id.is_empty()
                    && self.scope_prefix == format!("kubernetes-{}", native.instance_id.simple())
                    && self.container_name == format!("desktop-{}", self.session_id.simple())
                    && self.workspace_volume == format!("ws-{}", &claim[..48]),
                "native desktop original resource identity differs"
            );
        }
        for time in [&self.created_at, &self.started_at] {
            ensure!(
                time.len() <= 64 && chrono::DateTime::parse_from_rfc3339(time).is_ok(),
                "invalid desktop lifecycle identity"
            );
        }
        Ok(())
    }
    pub(crate) fn digest(&self) -> Result<String> {
        self.validate()?;
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(self)?)))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LiveJob {
    pub binding: JobBinding,
    pub running: bool,
    pub paused: bool,
    pub restarting: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_resource_version: Option<String>,
}
impl LiveJob {
    fn validate(&self) -> Result<()> {
        self.binding.validate()?;
        if self.binding.version == 2 {
            ensure!(
                self.observed_resource_version
                    .as_ref()
                    .is_some_and(|v| !v.is_empty()
                        && v.len() <= 128
                        && v.bytes().all(|c| c.is_ascii_graphic())),
                "fresh native desktop resource readback missing"
            );
        } else {
            ensure!(
                self.observed_resource_version.is_none(),
                "unexpected native resource version on Docker proof"
            );
        }
        ensure!(
            self.running && !self.paused && !self.restarting,
            "desktop job is not continuously running"
        );
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProbeRequest {
    pub version: u8,
    pub challenge: Uuid,
    pub binding: JobBinding,
}
impl ProbeRequest {
    pub(crate) fn new(binding: JobBinding) -> Result<Self> {
        binding.validate()?;
        Ok(Self {
            version: binding.version,
            challenge: Uuid::new_v4(),
            binding,
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeResponse {
    version: u8,
    challenge: Uuid,
    job: LiveJob,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StopResponse {
    version: u8,
    binding: JobBinding,
    execution_stopped: bool,
}

/// Private constructor: only a verified, fresh request response can mint this.
pub(crate) struct VerifiedJob {
    binding: JobBinding,
    deadline: Instant,
}
impl VerifiedJob {
    pub(crate) fn binding(&self) -> &JobBinding {
        &self.binding
    }
    pub(crate) fn ensure_fresh(&self) -> Result<()> {
        ensure!(
            Instant::now() < self.deadline,
            "desktop probe answered after its request deadline"
        );
        Ok(())
    }
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }
}

pub(crate) struct ProofAttempt {
    fields: [String; 5],
    started: Instant,
    consumed: bool,
}
impl ProofAttempt {
    pub(crate) fn new(
        method: &str,
        path: &str,
        timestamp: &str,
        nonce: &str,
        body: &[u8],
        started: Instant,
    ) -> Self {
        Self {
            fields: [
                method.into(),
                path.into(),
                timestamp.into(),
                nonce.into(),
                hex::encode(Sha256::digest(body)),
            ],
            started,
            consumed: false,
        }
    }
    fn consume(&mut self) -> Result<()> {
        ensure!(!self.consumed, "desktop request proof was already consumed");
        self.consumed = true;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn verify(
        &self,
        key: &[u8],
        status: u16,
        headers: &HeaderMap,
        response: &[u8],
        domain: &[u8],
        name: &str,
        binding: Option<&[u8]>,
    ) -> Result<()> {
        ensure!(
            status == 200 && response.len() <= MAX_RESPONSE,
            "invalid desktop proof response"
        );
        let proof_values = headers.get_all(name);
        let mut values = proof_values.iter();
        let value = values
            .next()
            .context("desktop response proof missing")?
            .to_str()?;
        ensure!(values.next().is_none(), "duplicate desktop response proof");
        let prefix = if [KUBE_CREATE_DOMAIN, KUBE_PROBE_DOMAIN, KUBE_STOP_DOMAIN].contains(&domain)
        {
            "v2:"
        } else {
            "v1:"
        };
        let signature = hex::decode(
            value
                .strip_prefix(prefix)
                .context("unsupported desktop proof version")?,
        )?;
        ensure!(
            signature.len() == 32,
            "invalid desktop proof signature length"
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(key)
            .map_err(|_| anyhow::anyhow!("invalid desktop proof key"))?;
        mac.update(domain);
        let response_digest = hex::encode(Sha256::digest(response));
        for field in self
            .fields
            .iter()
            .map(String::as_str)
            .chain(["200", response_digest.as_str()])
        {
            mac.update(&(field.len() as u64).to_be_bytes());
            mac.update(field.as_bytes());
        }
        if let Some(binding) = binding {
            let digest = hex::encode(Sha256::digest(binding));
            mac.update(&(digest.len() as u64).to_be_bytes());
            mac.update(digest.as_bytes());
        }
        mac.verify_slice(&signature)
            .map_err(|_| anyhow::anyhow!("desktop response proof does not match this request"))?;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn creation(
        &mut self,
        key: &[u8],
        status: u16,
        headers: &HeaderMap,
        response: &[u8],
        spec: &SandboxDesktopSessionSpec,
        lease: Uuid,
        attempt: Uuid,
    ) -> Result<JobBinding> {
        self.consume()?;
        let binding_values = headers.get_all("x-cowork-desktop-binding");
        let mut values = binding_values.iter();
        let encoded = values
            .next()
            .context("desktop creation binding missing")?
            .to_str()?;
        ensure!(
            encoded.len() <= MAX_BINDING * 2 && values.next().is_none(),
            "invalid desktop creation binding header"
        );
        let bytes = URL_SAFE_NO_PAD.decode(encoded)?;
        ensure!(
            bytes.len() <= MAX_BINDING,
            "desktop creation binding too large"
        );
        let job: LiveJob = serde_json::from_slice(&bytes)?;
        job.validate()?;
        if let Some(native) = &job.binding.kubernetes {
            ensure!(
                native.pod_pids_limit <= spec.limits.pids,
                "actual native desktop PID limit exceeds original request"
            );
        }
        self.verify(
            key,
            status,
            headers,
            response,
            if job.binding.version == 2 {
                KUBE_CREATE_DOMAIN
            } else {
                CREATE_DOMAIN
            },
            "x-cowork-desktop-proof",
            Some(&bytes),
        )?;
        let result: SandboxDesktopSessionResult = serde_json::from_slice(response)?;
        ensure!(
            job.binding.run_id == spec.run_id
                && job.binding.session_id == spec.session_id
                && job.binding.workspace_id == spec.workspace_id
                && job.binding.lease_token == lease
                && job.binding.attempt_id == attempt
                && result.run_id == spec.run_id
                && result.session_id == spec.session_id
                && result.schema_version == spec.schema_version
                && result.dimensions.width == spec.dimensions.width
                && result.dimensions.height == spec.dimensions.height
                && result.dimensions.scale_factor == spec.dimensions.scale_factor
                && result.container_name == job.binding.container_name
                && result.workspace_volume == job.binding.workspace_volume,
            "desktop creation proof names a different job"
        );
        Ok(job.binding)
    }
    pub(crate) fn probe(
        &mut self,
        key: &[u8],
        status: u16,
        headers: &HeaderMap,
        response: &[u8],
        request: &ProbeRequest,
    ) -> Result<VerifiedJob> {
        self.consume()?;
        self.verify(
            key,
            status,
            headers,
            response,
            if request.binding.version == 2 {
                KUBE_PROBE_DOMAIN
            } else {
                PROBE_DOMAIN
            },
            "x-cowork-desktop-proof",
            None,
        )?;
        let result: ProbeResponse = serde_json::from_slice(response)?;
        result.job.validate()?;
        ensure!(
            request.version == request.binding.version
                && !request.challenge.is_nil()
                && result.version == request.version
                && result.challenge == request.challenge
                && result.job.binding == request.binding,
            "desktop probe identity changed"
        );
        let job = VerifiedJob {
            binding: result.job.binding,
            deadline: self.started + FRESH_PROBE,
        };
        job.ensure_fresh()?;
        Ok(job)
    }
    pub(crate) fn stopped(
        &mut self,
        key: &[u8],
        status: u16,
        headers: &HeaderMap,
        response: &[u8],
        binding: &JobBinding,
    ) -> Result<()> {
        self.consume()?;
        self.verify(
            key,
            status,
            headers,
            response,
            if binding.version == 2 {
                KUBE_STOP_DOMAIN
            } else {
                STOP_DOMAIN
            },
            "x-cowork-desktop-proof",
            None,
        )?;
        let result: StopResponse = serde_json::from_slice(response)?;
        result.binding.validate()?;
        ensure!(
            result.version == binding.version
                && result.execution_stopped
                && result.binding == *binding,
            "physical desktop stop is unconfirmed"
        );
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct ProofTransport {
    endpoint: reqwest::Url,
    pub http: reqwest::Client,
    tls: TlsConnector,
    version: u8,
}
impl ProofTransport {
    pub(crate) fn from_env(endpoint: &str) -> Result<Option<Self>> {
        let enabled = |name: &str| -> Result<bool> {
            match std::env::var(name) {
                Err(std::env::VarError::NotPresent) => Ok(false),
                Ok(v) if v == "false" => Ok(false),
                Ok(v) if v == "true" => Ok(true),
                _ => anyhow::bail!("invalid runner desktop proof opt-in"),
            }
        };
        let docker = enabled("COWORK_RUNNER_DESKTOP_JOB_PROOFS")?;
        let native = enabled("COWORK_RUNNER_KUBERNETES_DESKTOP_PROOFS")?;
        ensure!(
            !(docker && native),
            "desktop proof backends must be explicitly exclusive"
        );
        if !docker && !native {
            return Ok(None);
        }
        if native {
            for name in [
                "COWORK_INSTANCE_ID",
                "COWORK_ORGANISATION_ID",
                "COWORK_KUBE_NAMESPACE_UID",
                "COWORK_KUBE_CLUSTER_UID",
                "COWORK_KUBE_GENERATION",
            ] {
                ensure!(
                    !Uuid::parse_str(
                        &std::env::var(name)
                            .context("original native desktop API scope missing")?
                    )?
                    .is_nil(),
                    "empty native desktop API scope"
                );
            }
        }
        let endpoint = Self::endpoint(endpoint)?;
        let ca_path = std::env::var("COWORK_RUNNER_CA_CERTIFICATE_FILE")
            .context("runner desktop proofs require explicit CA trust")?;
        let file = std::fs::File::open(ca_path)?;
        ensure!(
            file.metadata()?.is_file() && file.metadata()?.len() <= MAX_CA as u64,
            "runner public trust file is too large"
        );
        let mut ca = Vec::new();
        file.take(MAX_CA as u64 + 1).read_to_end(&mut ca)?;
        let tls = Self::connector(&ca)?;
        let http = crate::management_transport::client_with_ca_file(
            endpoint.as_str(),
            Duration::from_secs(30),
            "COWORK_RUNNER_CA_CERTIFICATE_FILE",
        )?;
        Ok(Some(Self {
            endpoint,
            http,
            tls,
            version: if native { 2 } else { 1 },
        }))
    }
    pub(crate) fn accepts(&self, binding: &JobBinding) -> Result<()> {
        binding.validate()?;
        ensure!(
            binding.version == self.version,
            "runner desktop backend changed"
        );
        if let Some(native) = &binding.kubernetes {
            for (name, actual) in [
                ("COWORK_INSTANCE_ID", native.instance_id),
                ("COWORK_ORGANISATION_ID", native.organisation_id),
                ("COWORK_KUBE_NAMESPACE_UID", native.namespace_uid),
                ("COWORK_KUBE_CLUSTER_UID", native.cluster_uid),
                ("COWORK_KUBE_GENERATION", native.generation),
            ] {
                ensure!(
                    Uuid::parse_str(
                        &std::env::var(name)
                            .context("original native desktop API scope missing")?
                    )? == actual,
                    "native desktop API scope changed"
                );
            }
        }
        Ok(())
    }
    pub(crate) fn native(&self) -> bool {
        self.version == 2
    }
    fn connector(ca: &[u8]) -> Result<TlsConnector> {
        ensure!(
            !ca.is_empty()
                && ca.len() <= MAX_CA
                && !ca.windows(11).any(|value| value == b"PRIVATE KEY"),
            "invalid runner public trust roots"
        );
        let mut roots = RootCertStore::empty();
        for certificate in openssl::x509::X509::stack_from_pem(ca)? {
            roots.add(CertificateDer::from(certificate.to_der()?))?;
        }
        ensure!(!roots.is_empty(), "runner public trust roots are empty");
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
        Ok(TlsConnector::from(Arc::new(config)))
    }
    fn endpoint(value: &str) -> Result<reqwest::Url> {
        let url = reqwest::Url::parse(value)?;
        ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && matches!(url.path(), "" | "/"),
            "runner desktop proofs require an unambiguous HTTPS origin"
        );
        Ok(url)
    }
    pub(crate) async fn websocket(
        &self,
        request: Request<()>,
        deadline: Instant,
    ) -> Result<tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>>
    {
        let host = self
            .endpoint
            .host_str()
            .context("configured runner hostname missing")?;
        ensure!(
            request.uri().scheme_str() == Some("wss")
                && request.uri().host() == Some(host)
                && request.uri().port_u16().unwrap_or(443)
                    == self.endpoint.port_or_known_default().unwrap_or(443),
            "runner WebSocket target differs from configured origin"
        );
        let name = ServerName::try_from(host.to_owned())?;
        let work = async {
            let tcp =
                TcpStream::connect((host, self.endpoint.port_or_known_default().unwrap_or(443)))
                    .await?;
            let tls = self.tls.connect(name, tcp).await?;
            let (socket, _) = tokio_tungstenite::client_async(request, tls).await?;
            Ok(socket)
        };
        tokio::time::timeout_at(deadline, work).await?
    }
}

pub(crate) async fn response_bytes(
    mut response: reqwest::Response,
) -> Result<(u16, HeaderMap, Vec<u8>)> {
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= MAX_RESPONSE as u64),
        "desktop proof response is too large"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= MAX_RESPONSE,
            "desktop proof response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, headers, bytes))
}

#[cfg(test)]
pub(crate) fn verified_for_test(binding: JobBinding) -> VerifiedJob {
    VerifiedJob {
        binding,
        deadline: Instant::now() + FRESH_PROBE,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    const KEY: &[u8] = b"private-fixture-signing-key-32bytes";
    const TS: &str = "1700000000";
    const NONCE: &str = "00000000-0000-4000-8000-000000000001";

    fn private_tls_fixture() -> (Vec<u8>, tokio_rustls::TlsAcceptor) {
        use openssl::{
            asn1::Asn1Time,
            bn::BigNum,
            hash::MessageDigest,
            pkey::PKey,
            rsa::Rsa,
            x509::{
                extension::{BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName},
                X509NameBuilder, X509,
            },
        };
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
        let ca_key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
        let server_key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "private test CA").unwrap();
        let name = name.build();
        let mut ca = X509::builder().unwrap();
        ca.set_version(2).unwrap();
        ca.set_serial_number(&BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap())
            .unwrap();
        ca.set_subject_name(&name).unwrap();
        ca.set_issuer_name(&name).unwrap();
        ca.set_pubkey(&ca_key).unwrap();
        ca.set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        ca.set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        ca.append_extension(BasicConstraints::new().critical().ca().build().unwrap())
            .unwrap();
        ca.append_extension(
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .crl_sign()
                .build()
                .unwrap(),
        )
        .unwrap();
        ca.sign(&ca_key, MessageDigest::sha256()).unwrap();
        let ca = ca.build();
        let mut server = X509::builder().unwrap();
        server.set_version(2).unwrap();
        server
            .set_serial_number(&BigNum::from_u32(2).unwrap().to_asn1_integer().unwrap())
            .unwrap();
        server.set_subject_name(&name).unwrap();
        server.set_issuer_name(ca.subject_name()).unwrap();
        server.set_pubkey(&server_key).unwrap();
        server
            .set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        server
            .set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        server
            .append_extension(BasicConstraints::new().critical().build().unwrap())
            .unwrap();
        server
            .append_extension(
                KeyUsage::new()
                    .critical()
                    .digital_signature()
                    .key_encipherment()
                    .build()
                    .unwrap(),
            )
            .unwrap();
        server
            .append_extension(ExtendedKeyUsage::new().server_auth().build().unwrap())
            .unwrap();
        let san = SubjectAlternativeName::new()
            .dns("localhost")
            .build(&server.x509v3_context(Some(&ca), None))
            .unwrap();
        server.append_extension(san).unwrap();
        server.sign(&ca_key, MessageDigest::sha256()).unwrap();
        let certificate = CertificateDer::from(server.build().to_der().unwrap());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            server_key.private_key_to_pkcs8().unwrap(),
        ));
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate], key)
        .unwrap();
        (
            ca.to_pem().unwrap(),
            tokio_rustls::TlsAcceptor::from(Arc::new(config)),
        )
    }

    #[tokio::test]
    async fn private_ca_websocket_verifies_hostname_and_trust_without_redirects() {
        use futures_util::{SinkExt, StreamExt};
        use tokio::io::AsyncWriteExt;
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
        for case in ["trusted", "wrong_hostname", "foreign_ca", "redirect"] {
            let (ca, acceptor) = private_tls_fixture();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let host = if case == "wrong_hostname" {
                "127.0.0.1"
            } else {
                "localhost"
            };
            let roots = if case == "foreign_ca" {
                private_tls_fixture().0
            } else {
                ca
            };
            let transport = ProofTransport {
                endpoint: ProofTransport::endpoint(&format!("https://{host}:{port}")).unwrap(),
                http: reqwest::Client::new(),
                tls: ProofTransport::connector(&roots).unwrap(),
                version: 1,
            };
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let Ok(mut tls) = acceptor.accept(socket).await else {
                    return;
                };
                if case == "redirect" {
                    tls.write_all(b"HTTP/1.1 302 Found\r\nLocation: wss://unconfigured.example/terminal\r\nContent-Length: 0\r\n\r\n").await.unwrap();
                    return;
                }
                let mut socket = tokio_tungstenite::accept_async(tls).await.unwrap();
                socket
                    .send(Message::Binary(b"private terminal bytes".to_vec().into()))
                    .await
                    .unwrap();
            });
            let request = format!(
                "wss://{host}:{port}/v1/runs/{}/terminal?columns=80&rows=24",
                Uuid::new_v4()
            )
            .into_client_request()
            .unwrap();
            let socket = transport
                .websocket(request, Instant::now() + Duration::from_secs(5))
                .await;
            if case == "trusted" {
                let mut socket = socket.unwrap();
                let bytes = tokio::time::timeout(Duration::from_secs(2), socket.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    bytes,
                    Message::Binary(b"private terminal bytes".to_vec().into())
                );
            } else {
                assert!(socket.is_err(), "private TLS must fail closed: {case}");
            }
            tokio::time::timeout(Duration::from_secs(2), server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[test]
    fn private_ca_connector_rejects_empty_oversized_malformed_or_key_material() {
        for ca in [
            b"".as_slice(),
            b"not a certificate",
            b"-----BEGIN PRIVATE KEY-----",
        ] {
            assert!(ProofTransport::connector(ca).is_err());
        }
        assert!(ProofTransport::connector(&vec![b'x'; MAX_CA + 1]).is_err());
    }
    pub(crate) fn binding() -> JobBinding {
        let id = |last: u128| Uuid::from_u128(last);
        JobBinding {
            version: 1,
            runner_id: id(1),
            engine_id: "private-fixture-engine".into(),
            scope_prefix: "owned-fixture-".into(),
            reservation_id: id(2),
            attempt_id: id(3),
            container_id: "a".repeat(64),
            container_name: format!("owned-fixture-desktop-{}", id(4).simple()),
            created_at: "2026-10-09T00:00:00Z".into(),
            started_at: "2026-10-09T00:00:01Z".into(),
            run_id: id(5),
            session_id: id(4),
            lease_token: id(6),
            workspace_id: Some(id(7)),
            workspace_volume: "owned-fixture-workspace-00000000000000000000000000000007".into(),
            kubernetes: None,
        }
    }
    fn probe(binding: JobBinding) -> ProbeRequest {
        ProbeRequest {
            version: 1,
            challenge: Uuid::from_u128(8),
            binding,
        }
    }
    fn reply(request: &ProbeRequest) -> Vec<u8> {
        serde_json::to_vec(&ProbeResponse {
            version: 1,
            challenge: request.challenge,
            job: LiveJob {
                binding: request.binding.clone(),
                running: true,
                paused: false,
                restarting: false,
                observed_resource_version: None,
            },
        })
        .unwrap()
    }
    fn attempt(body: &[u8]) -> ProofAttempt {
        ProofAttempt::new("POST", PROBE_PATH, TS, NONCE, body, Instant::now())
    }
    fn signed(
        pending: &ProofAttempt,
        response: &[u8],
        domain: &[u8],
        binding: Option<&[u8]>,
    ) -> HeaderMap {
        let mut mac = Hmac::<Sha256>::new_from_slice(KEY).unwrap();
        mac.update(domain);
        let digest = hex::encode(Sha256::digest(response));
        for field in pending
            .fields
            .iter()
            .map(String::as_str)
            .chain(["200", digest.as_str()])
        {
            mac.update(&(field.len() as u64).to_be_bytes());
            mac.update(field.as_bytes());
        }
        if let Some(binding) = binding {
            let digest = hex::encode(Sha256::digest(binding));
            mac.update(&(digest.len() as u64).to_be_bytes());
            mac.update(digest.as_bytes());
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cowork-desktop-proof",
            format!("v1:{}", hex::encode(mac.finalize().into_bytes()))
                .parse()
                .unwrap(),
        );
        headers
    }
    #[test]
    fn probe_is_bound_to_request_response_domain_and_consumed_once() {
        let request = probe(binding());
        let body = serde_json::to_vec(&request).unwrap();
        let response = reply(&request);
        let headers = signed(&attempt(&body), &response, PROBE_DOMAIN, None);
        let mut pending = attempt(&body);
        assert!(pending
            .probe(KEY, 200, &headers, &response, &request)
            .is_ok());
        assert!(pending
            .probe(KEY, 200, &headers, &response, &request)
            .is_err());
        for mut changed in [
            ProofAttempt::new("GET", PROBE_PATH, TS, NONCE, &body, Instant::now()),
            ProofAttempt::new(
                "POST",
                "/v1/desktop-session-probe?foreign=1",
                TS,
                NONCE,
                &body,
                Instant::now(),
            ),
            ProofAttempt::new(
                "POST",
                PROBE_PATH,
                "1700000001",
                NONCE,
                &body,
                Instant::now(),
            ),
            ProofAttempt::new("POST", PROBE_PATH, TS, "fresh-nonce", &body, Instant::now()),
            attempt(b"different body"),
        ] {
            assert!(changed
                .probe(KEY, 200, &headers, &response, &request)
                .is_err());
        }
        for domain in [
            CREATE_DOMAIN,
            STOP_DOMAIN,
            b"open-cowork/runner-stop/v1\0",
            b"open-cowork/runner-no-start/v1\0",
        ] {
            assert!(attempt(&body)
                .probe(
                    KEY,
                    200,
                    &signed(&attempt(&body), &response, domain, None),
                    &response,
                    &request
                )
                .is_err());
        }
        assert!(attempt(&body)
            .probe(KEY, 429, &headers, &response, &request)
            .is_err());
        assert!(attempt(&body)
            .probe(b"foreign signing key", 200, &headers, &response, &request)
            .is_err());
        let mut duplicate = headers.clone();
        duplicate.append(
            "x-cowork-desktop-proof",
            headers["x-cowork-desktop-proof"].clone(),
        );
        assert!(attempt(&body)
            .probe(KEY, 200, &duplicate, &response, &request)
            .is_err());
    }
    #[test]
    fn genuine_but_different_job_challenge_and_nonlive_response_are_refused() {
        let request = probe(binding());
        let body = serde_json::to_vec(&request).unwrap();
        let base: serde_json::Value = serde_json::from_slice(&reply(&request)).unwrap();
        for (field, value) in [
            ("engine_id", serde_json::json!("another-engine")),
            ("container_id", serde_json::json!("b".repeat(64))),
            ("created_at", serde_json::json!("2026-10-09T00:00:02Z")),
            ("started_at", serde_json::json!("2026-10-09T00:00:03Z")),
            ("lease_token", serde_json::json!(Uuid::new_v4())),
            ("attempt_id", serde_json::json!(Uuid::new_v4())),
            ("workspace_id", serde_json::json!(Uuid::new_v4())),
        ] {
            let mut changed = base.clone();
            changed["job"]["binding"][field] = value;
            let response = serde_json::to_vec(&changed).unwrap();
            assert!(attempt(&body)
                .probe(
                    KEY,
                    200,
                    &signed(&attempt(&body), &response, PROBE_DOMAIN, None),
                    &response,
                    &request
                )
                .is_err());
        }
        for (field, value) in [("running", false), ("paused", true), ("restarting", true)] {
            let mut changed = base.clone();
            changed["job"][field] = value.into();
            let response = serde_json::to_vec(&changed).unwrap();
            assert!(attempt(&body)
                .probe(
                    KEY,
                    200,
                    &signed(&attempt(&body), &response, PROBE_DOMAIN, None),
                    &response,
                    &request
                )
                .is_err());
        }
        let mut changed = base;
        changed["challenge"] = serde_json::json!(Uuid::new_v4());
        let response = serde_json::to_vec(&changed).unwrap();
        assert!(attempt(&body)
            .probe(
                KEY,
                200,
                &signed(&attempt(&body), &response, PROBE_DOMAIN, None),
                &response,
                &request
            )
            .is_err());
    }
    #[test]
    fn delayed_probe_never_extends_request_start_authority() {
        let request = probe(binding());
        let body = serde_json::to_vec(&request).unwrap();
        let response = reply(&request);
        let mut pending = ProofAttempt::new(
            "POST",
            PROBE_PATH,
            TS,
            NONCE,
            &body,
            Instant::now() - Duration::from_secs(6),
        );
        let headers = signed(&pending, &response, PROBE_DOMAIN, None);
        assert!(pending
            .probe(KEY, 200, &headers, &response, &request)
            .is_err());
    }
    #[test]
    fn immutable_identity_rejects_nil_ids_foreign_scope_and_ambiguous_https_origins() {
        let mut job = binding();
        job.attempt_id = Uuid::nil();
        assert!(job.validate().is_err());
        let mut job = binding();
        job.container_name = "another-scope-desktop".into();
        assert!(job.validate().is_err());
        let mut job = binding();
        job.container_id = "A".repeat(64);
        assert!(job.validate().is_err());
        for url in [
            "http://runner",
            "wss://runner",
            "https://u:p@runner",
            "https://runner?callback=x",
            "https://runner#secret",
            "https://runner/path",
        ] {
            assert!(ProofTransport::endpoint(url).is_err());
        }
        assert!(ProofTransport::endpoint("https://runner:8443/").is_ok());
    }
    #[test]
    fn physical_stop_requires_matching_binding_signed_body_and_success() {
        let job = binding();
        let path = format!(
            "/v1/desktop-sessions/{}?binding_sha256={}",
            job.session_id,
            job.digest().unwrap()
        );
        let fresh = || ProofAttempt::new("DELETE", &path, TS, NONCE, &[], Instant::now());
        let response = serde_json::to_vec(&StopResponse {
            version: 1,
            binding: job.clone(),
            execution_stopped: true,
        })
        .unwrap();
        let headers = signed(&fresh(), &response, STOP_DOMAIN, None);
        assert!(fresh().stopped(KEY, 200, &headers, &response, &job).is_ok());
        assert!(fresh()
            .stopped(KEY, 404, &headers, &response, &job)
            .is_err());
        let mut wrong = job.clone();
        wrong.engine_id = "foreign-engine".into();
        assert!(fresh()
            .stopped(KEY, 200, &headers, &response, &wrong)
            .is_err());
        let denied = serde_json::to_vec(&StopResponse {
            version: 1,
            binding: job.clone(),
            execution_stopped: false,
        })
        .unwrap();
        assert!(fresh()
            .stopped(
                KEY,
                200,
                &signed(&fresh(), &denied, STOP_DOMAIN, None),
                &denied,
                &job
            )
            .is_err());
    }
    #[test]
    fn missing_creation_header_consumes_request_before_a_replacement_response() {
        let job = binding();
        let spec = SandboxDesktopSessionSpec {
            schema_version: cowork_contracts::SCHEMA_VERSION,
            session_id: job.session_id,
            run_id: job.run_id,
            workspace_id: job.workspace_id,
            dimensions: cowork_contracts::DesktopDimensions {
                width: 1024,
                height: 768,
                scale_factor: 1.0,
            },
            network: cowork_contracts::SandboxNetwork::FilteredEgress,
            limits: cowork_contracts::SandboxLimits::default(),
        };
        let body = serde_json::to_vec(&spec).unwrap();
        let path = format!(
            "/v2/desktop-sessions?lease_token={}&attempt_id={}",
            job.lease_token, job.attempt_id
        );
        let mut pending = ProofAttempt::new("POST", &path, TS, NONCE, &body, Instant::now());
        let response = serde_json::to_vec(&SandboxDesktopSessionResult {
            schema_version: spec.schema_version,
            session_id: job.session_id,
            run_id: job.run_id,
            container_name: job.container_name.clone(),
            workspace_volume: job.workspace_volume.clone(),
            dimensions: spec.dimensions.clone(),
        })
        .unwrap();
        assert!(pending
            .creation(
                KEY,
                200,
                &HeaderMap::new(),
                &response,
                &spec,
                job.lease_token,
                job.attempt_id
            )
            .is_err());
        let raw = serde_json::to_vec(&LiveJob {
            binding: job.clone(),
            running: true,
            paused: false,
            restarting: false,
            observed_resource_version: None,
        })
        .unwrap();
        let mut headers = signed(&pending, &response, CREATE_DOMAIN, Some(&raw));
        headers.insert(
            "x-cowork-desktop-binding",
            URL_SAFE_NO_PAD.encode(&raw).parse().unwrap(),
        );
        assert!(pending
            .creation(
                KEY,
                200,
                &headers,
                &response,
                &spec,
                job.lease_token,
                job.attempt_id
            )
            .is_err());
        let mut fresh = ProofAttempt::new("POST", &path, TS, NONCE, &body, Instant::now());
        assert_eq!(
            fresh
                .creation(
                    KEY,
                    200,
                    &headers,
                    &response,
                    &spec,
                    job.lease_token,
                    job.attempt_id
                )
                .unwrap(),
            job
        );
        let mut fresh = ProofAttempt::new("POST", &path, TS, NONCE, &body, Instant::now());
        assert!(fresh
            .creation(
                KEY,
                200,
                &headers,
                &response,
                &spec,
                job.lease_token,
                Uuid::new_v4()
            )
            .is_err());
    }
}
