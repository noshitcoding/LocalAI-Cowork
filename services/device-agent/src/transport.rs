//! Device-credential HTTP transport. Every mutation is sent once; execution is
//! never replayed after an absent acknowledgement.
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use cowork_contracts::DeviceExecutorRegistration;
use reqwest::{Client, Method, Response, StatusCode, Url, header};
use serde_json::{Value, json};
use std::{fmt, net::IpAddr, time::Duration};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::execution::DeviceControl;

const JSON_BYTES: usize = 1024 * 1024;
const CHUNK_BYTES: usize = 4 * 1024 * 1024;
const ARTIFACT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct AgentClient {
    http: Client,
    server: Url,
    executor: Uuid,
    token: Zeroizing<String>,
}

/// The body is deliberately excluded: a server error must not echo credentials
/// or arbitrary model/tool output into the agent's log.
#[derive(Debug)]
pub struct AgentResponseError {
    pub status: StatusCode,
}
impl fmt::Display for AgentResponseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Device server rejected the operation (HTTP {})",
            self.status.as_u16()
        )
    }
}
impl std::error::Error for AgentResponseError {}

pub fn authority_lost(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<AgentResponseError>()
        .is_some_and(|e| matches!(e.status.as_u16(), 401 | 403 | 404 | 409 | 410))
}

impl AgentClient {
    pub fn executor_id(&self) -> Uuid {
        self.executor
    }

    pub fn websocket_request(
        &self,
        suffix: &str,
    ) -> Result<tokio_tungstenite::tungstenite::http::Request<()>> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut url = self.url(suffix)?;
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme)
            .map_err(|_| anyhow::anyhow!("Invalid device WebSocket origin"))?;
        let mut request = url
            .as_str()
            .into_client_request()
            .context("Invalid device WebSocket request")?;
        let mut credential = tokio_tungstenite::tungstenite::http::HeaderValue::from_str(&format!(
            "Bearer {}",
            self.token.as_str()
        ))
        .context("Invalid device credential")?;
        credential.set_sensitive(true);
        request.headers_mut().insert(
            tokio_tungstenite::tungstenite::http::header::AUTHORIZATION,
            credential,
        );
        Ok(request)
    }

    pub fn new(server_url: &str, executor: Uuid, token: String) -> Result<Self> {
        let token = Zeroizing::new(token);
        let server = Url::parse(server_url).context("Invalid device server origin")?;
        let loopback = server.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
        if executor.is_nil()
            || server.host_str().is_none()
            || !server.username().is_empty()
            || server.password().is_some()
            || server.query().is_some()
            || server.fragment().is_some()
            || !matches!(server.path(), "" | "/")
            || !(server.scheme() == "https" || server.scheme() == "http" && loopback)
        {
            bail!(
                "Device server must be a canonical HTTPS origin; HTTP is allowed only on loopback"
            );
        }
        if token.len() < 32 || token.len() > 8192 || !token.bytes().all(|b| (33..=126).contains(&b))
        {
            bail!("Invalid device credential");
        }
        let http = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .pool_idle_timeout(Duration::from_secs(30))
            .user_agent(concat!("LocalAI-Cowork-Device/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("Device HTTP transport unavailable")?;
        Ok(Self {
            http,
            server,
            executor,
            token,
        })
    }

    fn url(&self, suffix: &str) -> Result<Url> {
        if suffix.len() > 768
            || suffix.contains(['?', '#', '\\', '%'])
            || suffix.starts_with("//")
            || suffix
                .trim_start_matches('/')
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            bail!("Invalid private device route");
        }
        let mut url = self.server.clone();
        url.set_path(&format!(
            "/api/v1/agent/executors/{}/{}",
            self.executor,
            suffix.trim_start_matches('/')
        ));
        Ok(url)
    }

    fn request(
        &self,
        method: Method,
        url: Url,
        lease: Option<Uuid>,
    ) -> Result<reqwest::RequestBuilder> {
        let mut credential =
            header::HeaderValue::from_str(&format!("Bearer {}", self.token.as_str()))
                .context("Invalid device credential")?;
        credential.set_sensitive(true);
        let mut request = self
            .http
            .request(method, url)
            .header(header::AUTHORIZATION, credential);
        if let Some(lease) = lease {
            request = request.header("x-cowork-lease-token", lease.to_string());
        }
        Ok(request)
    }

    async fn bounded(mut response: Response, bound: usize) -> Result<Vec<u8>> {
        let status = response.status();
        if !status.is_success() {
            return Err(AgentResponseError { status }.into());
        }
        if response
            .content_length()
            .is_some_and(|size| size > bound as u64)
        {
            bail!("Device server response exceeds its size limit");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("Device server response interrupted")?
        {
            if chunk.len() > bound.saturating_sub(bytes.len()) {
                bail!("Device server response exceeds its size limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn json(
        &self,
        method: Method,
        suffix: &str,
        lease: Option<Uuid>,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut request = self.request(method, self.url(suffix)?, lease)?;
        if let Some(body) = body {
            let raw = serde_json::to_vec(&body).context("Device request encoding failed")?;
            if raw.len() > JSON_BYTES {
                bail!("Device JSON request exceeds its size limit");
            }
            request = request
                .header(header::CONTENT_TYPE, "application/json")
                .body(raw);
        }
        let response = request
            .send()
            .await
            .context("Device server request failed")?;
        let bytes = Self::bounded(response, JSON_BYTES).await?;
        serde_json::from_slice(&bytes).context("Invalid device server JSON response")
    }

    pub async fn register(&self, registration: &DeviceExecutorRegistration) -> Result<Value> {
        if registration.executor_id != self.executor {
            bail!("Device registration does not match the configured executor");
        }
        self.json(
            Method::POST,
            "register",
            None,
            Some(serde_json::to_value(registration)?),
        )
        .await
    }

    pub async fn heartbeat(&self, active: &[Uuid]) -> Result<Value> {
        if active.len() > 16 {
            bail!("Too many active device leases");
        }
        self.json(
            Method::POST,
            "heartbeat",
            None,
            Some(json!({
                "protocol_version":2,"active_run_ids":active,"health":{"status":"ready"}
            })),
        )
        .await
    }

    pub async fn claim(&self) -> Result<Value> {
        self.json(Method::POST, "claim", None, None).await
    }

    pub async fn lease_heartbeat(&self, run: Uuid, lease: Uuid) -> Result<Value> {
        self.json(
            Method::POST,
            &format!("runs/{run}/heartbeat"),
            None,
            Some(json!({"lease_token":lease})),
        )
        .await
    }

    pub async fn complete(
        &self,
        run: Uuid,
        lease: Uuid,
        result: Value,
        snapshot: Option<Uuid>,
        diff: Value,
    ) -> Result<Value> {
        self.json(Method::POST, &format!("runs/{run}/complete"), None, Some(json!({
            "lease_token":lease,"result":result,"result_snapshot_manifest_id":snapshot,"result_diff_summary":diff
        }))).await
    }

    pub async fn fail(&self, run: Uuid, lease: Uuid, error: Value) -> Result<Value> {
        self.json(
            Method::POST,
            &format!("runs/{run}/fail"),
            None,
            Some(json!({"lease_token":lease,"error":error})),
        )
        .await
    }
}

fn live_lease(run: Uuid, lease: Uuid) -> Result<()> {
    if run.is_nil() || lease.is_nil() {
        bail!("Invalid device run lease");
    }
    Ok(())
}

fn valid_digest(digest: &str) -> Result<()> {
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Invalid snapshot chunk digest");
    }
    Ok(())
}

#[async_trait]
impl DeviceControl for AgentClient {
    async fn run_json(
        &self,
        run: Uuid,
        lease: Uuid,
        method: &str,
        suffix: &str,
        body: Value,
    ) -> Result<Value> {
        live_lease(run, lease)?;
        let method = match method {
            "GET" => Method::GET,
            "POST" => Method::POST,
            _ => bail!("Unsupported device JSON method"),
        };
        let body = if method == Method::GET {
            if !body.is_null() {
                bail!("Device GET requests have no body");
            }
            None
        } else {
            if body
                .get("lease_token")
                .is_some_and(|value| value.as_str() != Some(lease.to_string().as_str()))
            {
                bail!("Device request body does not match its lease");
            }
            Some(body)
        };
        self.json(
            method,
            &format!("runs/{run}/{}", suffix.trim_start_matches('/')),
            Some(lease),
            body,
        )
        .await
    }

    async fn get_chunk(&self, run: Uuid, lease: Uuid, digest: &str) -> Result<Vec<u8>> {
        live_lease(run, lease)?;
        valid_digest(digest)?;
        let url = self.url(&format!("runs/{run}/snapshot/chunks/{digest}"))?;
        let response = self
            .request(Method::GET, url, Some(lease))?
            .send()
            .await
            .context("Device snapshot download failed")?;
        Self::bounded(response, CHUNK_BYTES).await
    }

    async fn put_chunk(
        &self,
        run: Uuid,
        lease: Uuid,
        manifest: Uuid,
        digest: &str,
        bytes: Vec<u8>,
    ) -> Result<Value> {
        live_lease(run, lease)?;
        valid_digest(digest)?;
        if manifest.is_nil() || bytes.len() > CHUNK_BYTES {
            bail!("Invalid or oversized device snapshot chunk");
        }
        let url = self.url(&format!(
            "runs/{run}/result-snapshot/{manifest}/chunks/{digest}"
        ))?;
        let response = self
            .request(Method::PUT, url, Some(lease))?
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .send()
            .await
            .context("Device snapshot upload failed")?;
        let bytes = Self::bounded(response, JSON_BYTES).await?;
        serde_json::from_slice(&bytes).context("Invalid snapshot upload response")
    }

    async fn artifact(
        &self,
        run: Uuid,
        lease: Uuid,
        path: &str,
        source: Uuid,
        bytes: Vec<u8>,
    ) -> Result<Value> {
        live_lease(run, lease)?;
        if source.is_nil()
            || path.is_empty()
            || path.len() > 1000
            || path.starts_with('/')
            || path.contains(['\\', ':'])
            || path.chars().any(char::is_control)
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || bytes.is_empty()
            || bytes.len() > ARTIFACT_BYTES
        {
            bail!("Invalid or oversized device artifact");
        }
        let mut url = self.url(&format!("runs/{run}/artifacts"))?;
        url.query_pairs_mut()
            .append_pair("path", path)
            .append_pair("source", "standalone_v2_device")
            .append_pair("source_event_id", &source.to_string());
        let response = self
            .request(Method::POST, url, Some(lease))?
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .send()
            .await
            .context("Device artifact upload failed")?;
        let bytes = Self::bounded(response, JSON_BYTES).await?;
        serde_json::from_slice(&bytes).context("Invalid device artifact response")
    }
}
