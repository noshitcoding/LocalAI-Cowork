//! Explicit operator-owned HTTPS transport for the durable V2 runner protocol.
//! No request can install an adapter, disable TLS, or retry an uncertain launch.
use cowork_contracts::ExecutionEnvironmentRef;
use cowork_domain::environments::{EnvironmentObservation, EnvironmentQualification};
use cowork_domain::{Error, Result};
use cowork_runner_protocol::{
    Binding, EnvironmentInstall, ExecutionLeaseRenewal, FenceRequest, LaunchRequest, Receipt,
    SandboxLifecycleReceipt, SandboxLifecycleRequest, SandboxPreviewGrant, SandboxPreviewRequest,
    Signed, SigningKey,
};
use cowork_runtime::{ProcessRunnerAccess, ProcessRunnerConfig};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
mod managed;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    format: u32,
    endpoint: String,
    binding: Binding,
    control_key_file: PathBuf,
    transport_token_file: PathBuf,
    ca_certificate_file: Option<PathBuf>,
    client_identity_file: Option<PathBuf>,
}

fn read_private(path: &Path, limit: u64) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        path.is_absolute(),
        "Runner authority paths must be absolute"
    );
    for ancestor in path.ancestors() {
        let observed = std::fs::symlink_metadata(ancestor)?;
        anyhow::ensure!(
            !observed.file_type().is_symlink(),
            "Runner authority cannot traverse a symlink"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            anyhow::ensure!(
                observed.file_attributes() & 0x400 == 0,
                "Runner authority cannot traverse a reparse point"
            );
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= limit,
        "Runner authority must be a bounded regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.mode() & 0o077 == 0
                && metadata.nlink() == 1
                && metadata.uid() == unsafe { libc::geteuid() },
            "Runner authority must be private and operator-owned"
        );
    }
    #[cfg(windows)]
    crate::private_secret_file::validate(&file)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 == metadata.len(),
        "Runner authority changed while reading"
    );
    Ok(bytes)
}

pub fn configured() -> anyhow::Result<Option<ProcessRunnerConfig>> {
    let Some(path) = std::env::var_os("COWORK_PROCESS_RUNNER_CONFIG") else {
        return Ok(None);
    };
    let configuration: Configuration =
        serde_json::from_slice(&read_private(Path::new(&path), 65536)?)?;
    anyhow::ensure!(
        configuration.format == 1,
        "Unsupported process runner configuration"
    );
    let url = reqwest::Url::parse(&configuration.endpoint)?;
    anyhow::ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "Runner endpoint must be an explicit HTTPS origin"
    );
    let token = String::from_utf8(read_private(&configuration.transport_token_file, 16384)?)?
        .trim()
        .to_owned();
    anyhow::ensure!(
        token.len() >= 48 && !token.chars().any(char::is_whitespace),
        "Invalid private runner transport credential"
    );
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5));
    if let Some(path) = configuration.ca_certificate_file {
        builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&read_private(
            &path,
            1024 * 1024,
        )?)?);
    }
    if let Some(path) = configuration.client_identity_file {
        builder = builder.identity(reqwest::Identity::from_pem(&read_private(
            &path,
            1024 * 1024,
        )?)?);
    }
    let control = SigningKey::from_pkcs8(&read_private(&configuration.control_key_file, 16384)?)?;
    let access = Arc::new(HttpsRunner {
        client: builder.build()?,
        origin: url,
        token,
        binding: configuration.binding.clone(),
    });
    Ok(Some(ProcessRunnerConfig::new(
        configuration.binding,
        control,
        access,
    )?))
}

struct HttpsRunner {
    client: reqwest::Client,
    origin: reqwest::Url,
    token: String,
    binding: Binding,
}

impl HttpsRunner {
    async fn post<T: Serialize>(&self, path: &str, value: &T) -> Result<reqwest::Response> {
        let url = self.origin.join(path).map_err(|_| Error::Forbidden)?;
        self.client
            .post(url)
            .bearer_auth(&self.token)
            .json(value)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| Error::UncertainEffect)
    }
    async fn body<T: DeserializeOwned>(mut response: reqwest::Response) -> Result<T> {
        if !response.status().is_success() {
            return Err(Error::UncertainEffect);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::UncertainEffect)? {
            if bytes.len().saturating_add(chunk.len()) > 2 * 1024 * 1024 {
                return Err(Error::UncertainEffect);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::UncertainEffect)
    }
}

#[async_trait::async_trait]
impl ProcessRunnerAccess for HttpsRunner {
    async fn managed_result_delivery_history(
        &self,
        request: Signed<cowork_runner_protocol::ManagedResultDeliveryRequest>,
    ) -> Result<Signed<cowork_runner_protocol::ManagedResultDeliveryReceipt>> {
        self.read_managed_delivery_history(request).await
    }
    async fn managed_result_delivery(
        &self,
        request: Signed<cowork_runner_protocol::ManagedResultDeliveryRequest>,
    ) -> Result<cowork_runtime::ManagedResultDeliveryResponse> {
        self.deliver_managed_result(request).await
    }
    async fn managed_result_retirement(
        &self,
        request: Signed<cowork_runner_protocol::ManagedResultRetirementQuery>,
    ) -> Result<Signed<cowork_runner_protocol::ManagedResultRetirementReceipt>> {
        self.observe_managed_retirement(request).await
    }
    async fn managed_result_context(
        &self,
        request: Signed<cowork_runner_protocol::ManagedResultContextRequest>,
    ) -> Result<Signed<cowork_runner_protocol::ManagedResultReadContext>> {
        self.observe_managed_result_context(request).await
    }
    async fn managed_result_readiness(
        &self,
        request: cowork_runner_protocol::ManagedReadyRequest,
    ) -> Result<Signed<cowork_runner_protocol::ManagedReadiness>> {
        self.managed_readiness_request(request).await
    }
    async fn managed_result_manifest(
        &self,
        request: Signed<cowork_runner_protocol::ManagedResultReadRequest>,
    ) -> Result<cowork_runner_protocol::ManagedResultManifest> {
        self.read_managed_manifest(request).await
    }
    async fn managed_result_chunk(
        &self,
        request: Signed<cowork_runner_protocol::ManagedResultReadRequest>,
        expected_size: u64,
    ) -> Result<Vec<u8>> {
        self.read_managed_chunk(request, expected_size).await
    }
    async fn managed_input_ready(
        &self,
        environment: &ExecutionEnvironmentRef,
        target: &cowork_contracts::ExecutionTargetRef,
    ) -> Result<()> {
        self.managed_readiness(environment, target)
            .await
            .map(|_| ())
    }
    async fn managed_launch(
        &self,
        request: Signed<LaunchRequest>,
        input: Arc<dyn cowork_runtime::ManagedInputAccess>,
        stop: CancellationToken,
    ) -> Result<Signed<Receipt>> {
        // Cancellation can interrupt body reads as well as sends. The existing
        // independent runner retains physical ownership after transport loss.
        tokio::select! {
            biased;
            _ = stop.cancelled() => Err(Error::UncertainEffect),
            result = self.managed_launch_once(request, input) => result,
        }
    }
    async fn sandbox_preview(
        &self,
        request: Signed<SandboxPreviewRequest>,
    ) -> Result<Signed<SandboxPreviewGrant>> {
        request
            .verify("sandbox_preview_request", &self.binding.control_public_key)
            .map_err(|_| Error::Forbidden)?;
        let url = self
            .origin
            .join("v2/sandbox-previews/issue")
            .map_err(|_| Error::Forbidden)?;
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.token)
            .json(&request)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|_| Error::UncertainEffect)?;
        let grant: Signed<SandboxPreviewGrant> = Self::body(response).await?;
        grant
            .verify("sandbox_preview", &self.binding.runner_public_key)
            .map_err(|_| Error::Forbidden)?;
        Ok(grant)
    }
    async fn sandbox_lifecycle_receipt(
        &self,
        request_id: &str,
    ) -> Result<Signed<SandboxLifecycleReceipt>> {
        if uuid::Uuid::parse_str(request_id).is_err() {
            return Err(Error::Forbidden);
        }
        let url = self
            .origin
            .join(&format!("v2/sandboxes/operations/{request_id}"))
            .map_err(|_| Error::Forbidden)?;
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|_| Error::UncertainEffect)?;
        let receipt: Signed<SandboxLifecycleReceipt> = Self::body(response).await?;
        receipt
            .verify("sandbox_lifecycle_receipt", &self.binding.runner_public_key)
            .map_err(|_| Error::Forbidden)?;
        Ok(receipt)
    }
    async fn sandbox_lifecycle(
        &self,
        request: Signed<SandboxLifecycleRequest>,
    ) -> Result<Signed<SandboxLifecycleReceipt>> {
        request
            .verify("sandbox_lifecycle", &self.binding.control_public_key)
            .map_err(|_| Error::Forbidden)?;
        let url = self
            .origin
            .join("v2/sandboxes/lifecycle")
            .map_err(|_| Error::Forbidden)?;
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.token)
            .json(&request)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|_| Error::UncertainEffect)?;
        // This header is emitted exclusively before the trusted receiver writes
        // an intent. A generic 409/404 or lost response is never this proof.
        if !response.status().is_success()
            && response
                .headers()
                .get("x-cowork-admission")
                .and_then(|v| v.to_str().ok())
                == Some("not-admitted")
        {
            if request.payload.operation
                == cowork_runner_protocol::SandboxLifecycleOperation::Inspect
                && response
                    .headers()
                    .get("x-cowork-session")
                    .and_then(|v| v.to_str().ok())
                    == Some("absent")
            {
                return Err(Error::SandboxSessionNotFound);
            }
            return Err(Error::WorkspaceBusy);
        }
        let receipt: Signed<SandboxLifecycleReceipt> = Self::body(response).await?;
        receipt
            .verify("sandbox_lifecycle_receipt", &self.binding.runner_public_key)
            .map_err(|_| Error::Forbidden)?;
        Ok(receipt)
    }
    async fn receipt(&self, attempt: &str) -> Result<Signed<Receipt>> {
        if attempt.is_empty()
            || attempt.len() > 128
            || !attempt
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        {
            return Err(Error::Forbidden);
        }
        let url = self
            .origin
            .join(&format!("v2/receipts/{attempt}"))
            .map_err(|_| Error::Forbidden)?;
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|_| Error::UncertainEffect)?;
        let signed: Signed<Receipt> = Self::body(response).await?;
        signed
            .verify("receipt", &self.binding.runner_public_key)
            .map_err(|_| Error::Forbidden)?;
        Ok(signed)
    }
    async fn install_environment(
        &self,
        request: Signed<EnvironmentInstall>,
    ) -> Result<Signed<EnvironmentQualification>> {
        request
            .verify("environment_install", &self.binding.control_public_key)
            .map_err(|_| Error::Forbidden)?;
        let url = self
            .origin
            .join("v2/environments/install")
            .map_err(|_| Error::Forbidden)?;
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.token)
            .json(&request)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|_| Error::UncertainEffect)?;
        let signed: Signed<EnvironmentQualification> = Self::body(response).await?;
        signed
            .verify("environment_qualification", &self.binding.runner_public_key)
            .map_err(|_| Error::Forbidden)?;
        Ok(signed)
    }
    async fn observe(
        &self,
        reference: &ExecutionEnvironmentRef,
    ) -> Result<Signed<EnvironmentObservation>> {
        let signed: Signed<EnvironmentObservation> =
            Self::body(self.post("v2/environments/observe", reference).await?).await?;
        signed
            .verify("environment_observation", &self.binding.runner_public_key)
            .map_err(|_| Error::Forbidden)?;
        Ok(signed)
    }
    async fn fence(&self, request: Signed<FenceRequest>) -> Result<()> {
        request
            .verify("fence", &self.binding.control_public_key)
            .map_err(|_| Error::Forbidden)?;
        let response = self.post("v2/fences", &request).await?;
        if !response.status().is_success() {
            return Err(Error::UncertainEffect);
        }
        Ok(())
    }
    async fn renew_execution_lease(&self, request: Signed<ExecutionLeaseRenewal>) -> Result<()> {
        request
            .verify("execution_lease", &self.binding.control_public_key)
            .map_err(|_| Error::Forbidden)?;
        let response = self.post("v2/execution-leases", &request).await?;
        if !response.status().is_success() {
            return Err(Error::UncertainEffect);
        }
        Ok(())
    }
    async fn launch(
        &self,
        request: Signed<LaunchRequest>,
        stop: CancellationToken,
    ) -> Result<Signed<Receipt>> {
        request
            .verify("launch", &self.binding.control_public_key)
            .map_err(|_| Error::Forbidden)?;
        let id = &request.payload.process_attempt_id;
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        {
            return Err(Error::Forbidden);
        }
        // Exactly one POST. A lost reply returns uncertainty, never another launch.
        let response = self.post("v2/launches", &request).await?;
        if response.status() != reqwest::StatusCode::ACCEPTED {
            let receipt: Signed<Receipt> = Self::body(response).await?;
            receipt
                .verify("receipt", &self.binding.runner_public_key)
                .map_err(|_| Error::UncertainEffect)?;
            return Ok(receipt);
        }
        let url = self
            .origin
            .join(&format!("v2/receipts/{id}"))
            .map_err(|_| Error::Forbidden)?;
        loop {
            tokio::select! {
                biased;
                _=stop.cancelled()=>return Err(Error::UncertainEffect),
                _=tokio::time::sleep(Duration::from_millis(250))=>{}
            }
            let response = self
                .client
                .get(url.clone())
                .bearer_auth(&self.token)
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .map_err(|_| Error::UncertainEffect)?;
            if response.status() == reqwest::StatusCode::ACCEPTED {
                continue;
            }
            let receipt: Signed<Receipt> = Self::body(response).await?;
            receipt
                .verify("receipt", &self.binding.runner_public_key)
                .map_err(|_| Error::UncertainEffect)?;
            return Ok(receipt);
        }
    }
}
