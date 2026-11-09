use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Extension, Path, Query, State,
    },
    http::{HeaderValue, StatusCode},
    response::Response,
    Json,
};
use chrono::Utc;
use cowork_contracts::{
    CreateDesktopSessionRequest, DesktopDimensions, DesktopSession, DesktopSessionState,
    DesktopStreamTicket, DesktopStreamTicketRequest, ExecutorRegistration, ExecutorTarget,
    PersonalDeviceRemoteControlMode, ProjectRole, RunEventKind, RunState,
    SandboxDesktopSessionResult, SandboxDesktopSessionSpec, SandboxLimits, SandboxNetwork,
    SCHEMA_VERSION,
};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message as RunnerMessage};
use uuid::Uuid;

use crate::{
    auth::{self, Principal},
    db,
    error::ApiError,
    organization, AppState,
};

type HmacSha256 = Hmac<Sha256>;

fn ensure_runner_success(status: u16) -> Result<()> {
    if !(200..300).contains(&status) {
        bail!("runner desktop request was refused with status {status}");
    }
    Ok(())
}

fn external_desktop_platform(
    registration: &ExecutorRegistration,
    managed_windows: bool,
) -> Option<&'static str> {
    let has_capability = |name: &str| {
        registration
            .capabilities
            .iter()
            .any(|capability| capability.name.0 == name)
    };
    let operating_system = registration.labels.get("os").map(String::as_str);
    if managed_windows {
        return (operating_system == Some("windows") && has_capability("desktop.windows"))
            .then_some("windows");
    }
    match operating_system {
        Some("windows") if has_capability("desktop.windows") => Some("windows"),
        Some("linux") if has_capability("desktop.linux") => Some("linux"),
        Some(_) => None,
        None if has_capability("desktop.windows") && !has_capability("desktop.linux") => {
            Some("windows")
        }
        None if has_capability("desktop.linux") && !has_capability("desktop.windows") => {
            Some("linux")
        }
        _ => None,
    }
}

#[derive(Clone)]
pub struct RunnerControl {
    http: Client,
    url: String,
    signing_key: Arc<Vec<u8>>,
    proof: Option<crate::runner_desktop_proof::ProofTransport>,
}

impl RunnerControl {
    pub(crate) fn http_client(&self) -> Client {
        self.proof
            .as_ref()
            .map(|proof| proof.http.clone())
            .unwrap_or_else(|| self.http.clone())
    }
    pub fn new(url: String, signing_key: String) -> Result<Self> {
        let proof = crate::runner_desktop_proof::ProofTransport::from_env(&url)?;
        let http = if std::env::var("COWORK_RUNNER_BACKEND").as_deref() == Ok("kubernetes") {
            crate::management_transport::client_with_ca_file(
                &url,
                std::time::Duration::from_secs(24 * 60 * 60),
                "COWORK_RUNNER_CA_CERTIFICATE_FILE",
            )?
        } else {
            Client::new()
        };
        Ok(Self {
            http,
            url,
            signing_key: Arc::new(signing_key.into_bytes()),
            proof,
        })
    }

    pub(crate) async fn stop_execution(
        &self,
        request: &crate::runner_admission::StopRequest,
    ) -> Result<crate::runner_admission::VerifiedStop> {
        let path = "/v1/execution-stop";
        let body = serde_json::to_vec(request)?;
        let (timestamp, nonce, signature) = self.signature("POST", path, &body)?;
        let mut attempt =
            crate::runner_admission::RequestAttempt::new("POST", path, &timestamp, &nonce, &body);
        let response = self
            .http_client()
            .post(format!("{}{path}", self.url.trim_end_matches('/')))
            .header("x-cowork-timestamp", timestamp)
            .header("x-cowork-nonce", nonce)
            .header("x-cowork-signature", signature)
            .header("content-type", "application/json")
            .body(body)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await?;
        let status = response.status();
        let headers = response.headers().clone();
        if response
            .content_length()
            .is_some_and(|length| length > 32 * 1024 * 1024)
        {
            bail!("runner stop response too large");
        }
        let bytes = response.bytes().await?;
        attempt
            .verify_stop(
                &self.signing_key,
                status.as_u16(),
                &headers,
                &bytes,
                request,
            )
            .ok_or_else(|| anyhow!("runner physical stop is unconfirmed; execution remains held"))
    }

    async fn start(
        &self,
        spec: &SandboxDesktopSessionSpec,
        lease: Uuid,
        attempt: Uuid,
    ) -> Result<Value> {
        let body = serde_json::to_vec(spec)?;
        let route = if spec.workspace_id.is_some() {
            "/v2/desktop-sessions"
        } else {
            "/v1/desktop-sessions"
        };
        let path = if self.proof.is_some() {
            format!("{route}?lease_token={lease}&attempt_id={attempt}")
        } else {
            route.to_owned()
        };
        let (timestamp, nonce, signature) = self.signature("POST", &path, &body)?;
        let mut pending = crate::runner_desktop_proof::ProofAttempt::new(
            "POST",
            &path,
            &timestamp,
            &nonce,
            &body,
            tokio::time::Instant::now(),
        );
        let client = self
            .proof
            .as_ref()
            .map(|proof| &proof.http)
            .unwrap_or(&self.http);
        crate::instance_disk_quotas::write_gate("workspace", 0)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let response = client
            .post(format!("{}{path}", self.url.trim_end_matches('/')))
            .header("x-cowork-timestamp", timestamp)
            .header("x-cowork-nonce", nonce)
            .header("x-cowork-signature", signature)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await?;
        let (status, headers, bytes) =
            crate::runner_desktop_proof::response_bytes(response).await?;
        ensure_runner_success(status)?;
        let result: SandboxDesktopSessionResult = serde_json::from_slice(&bytes)?;
        let mut metadata = serde_json::to_value(result)?;
        if let Some(transport) = &self.proof {
            let binding = pending.creation(
                &self.signing_key,
                status,
                &headers,
                &bytes,
                spec,
                lease,
                attempt,
            )?;
            transport.accepts(&binding)?;
            metadata["runner_job_binding"] = serde_json::to_value(binding)?;
        }
        Ok(metadata)
    }

    pub(crate) async fn probe_desktop(
        &self,
        binding: &crate::runner_desktop_proof::JobBinding,
    ) -> Result<crate::runner_desktop_proof::VerifiedJob> {
        let transport = self
            .proof
            .as_ref()
            .ok_or_else(|| anyhow!("physical desktop proofs are not configured"))?;
        transport.accepts(binding)?;
        let request = crate::runner_desktop_proof::ProbeRequest::new(binding.clone())?;
        let body = serde_json::to_vec(&request)?;
        let path = crate::runner_desktop_proof::PROBE_PATH;
        let (timestamp, nonce, signature) = self.signature("POST", path, &body)?;
        let started = tokio::time::Instant::now();
        let mut pending = crate::runner_desktop_proof::ProofAttempt::new(
            "POST", path, &timestamp, &nonce, &body, started,
        );
        let work = async {
            let response = transport
                .http
                .post(format!("{}{path}", self.url.trim_end_matches('/')))
                .header("x-cowork-timestamp", timestamp)
                .header("x-cowork-nonce", nonce)
                .header("x-cowork-signature", signature)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await?;
            let (status, headers, bytes) =
                crate::runner_desktop_proof::response_bytes(response).await?;
            pending.probe(&self.signing_key, status, &headers, &bytes, &request)
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), work).await?
    }

    pub(crate) async fn proven_stream(
        &self,
        binding: &crate::runner_desktop_proof::JobBinding,
        control: bool,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    > {
        let transport = self
            .proof
            .as_ref()
            .ok_or_else(|| anyhow!("physical desktop proofs are not configured"))?;
        let fresh = self.probe_desktop(binding).await?;
        let mut url = reqwest::Url::parse(&self.url)?;
        url.set_scheme("wss")
            .map_err(|_| anyhow!("invalid runner WebSocket origin"))?;
        url.set_path(&format!(
            "/v1/desktop-sessions/{}/stream",
            binding.session_id
        ));
        url.set_query(Some(&format!(
            "control={control}&binding_sha256={}",
            binding.digest()?
        )));
        let path = format!("{}?{}", url.path(), url.query().unwrap_or_default());
        let (timestamp, nonce, signature) = self.signature("GET", &path, &[])?;
        let mut request = url.as_str().into_client_request()?;
        request
            .headers_mut()
            .insert("x-cowork-timestamp", HeaderValue::from_str(&timestamp)?);
        request
            .headers_mut()
            .insert("x-cowork-nonce", HeaderValue::from_str(&nonce)?);
        request
            .headers_mut()
            .insert("x-cowork-signature", HeaderValue::from_str(&signature)?);
        fresh.ensure_fresh()?;
        transport.websocket(request, fresh.deadline()).await
    }

    pub(crate) async fn stop_bound_desktop(
        &self,
        binding: &crate::runner_desktop_proof::JobBinding,
    ) -> Result<()> {
        let transport = self
            .proof
            .as_ref()
            .ok_or_else(|| anyhow!("physical desktop proofs are not configured"))?;
        transport.accepts(binding)?;
        // A physical stop is issued once. Missing/forged/lost responses are
        // unknown and leave the durable stop_pending marker held.
        let path = format!(
            "/v1/desktop-sessions/{}?binding_sha256={}",
            binding.session_id,
            binding.digest()?
        );
        let (timestamp, nonce, signature) = self.signature("DELETE", &path, &[])?;
        let mut pending = crate::runner_desktop_proof::ProofAttempt::new(
            "DELETE",
            &path,
            &timestamp,
            &nonce,
            &[],
            tokio::time::Instant::now(),
        );
        let response = transport
            .http
            .delete(format!("{}{path}", self.url.trim_end_matches('/')))
            .header("x-cowork-timestamp", timestamp)
            .header("x-cowork-nonce", nonce)
            .header("x-cowork-signature", signature)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await?;
        let (status, headers, bytes) =
            crate::runner_desktop_proof::response_bytes(response).await?;
        pending.stopped(&self.signing_key, status, &headers, &bytes, binding)
    }

    async fn stop(&self, session_id: Uuid) -> Result<()> {
        if self.proof.as_ref().is_some_and(|proof| proof.native()) {
            bail!("native desktop stop requires its original physical binding");
        }
        let path = format!("/v1/desktop-sessions/{session_id}");
        let (timestamp, nonce, signature) = self.signature("DELETE", &path, &[])?;
        let response = self
            .http
            .delete(format!("{}{}", self.url.trim_end_matches('/'), path))
            .header("x-cowork-timestamp", timestamp)
            .header("x-cowork-nonce", nonce)
            .header("x-cowork-signature", signature)
            .send()
            .await?;
        if !response.status().is_success() && response.status() != reqwest::StatusCode::NOT_FOUND {
            bail!(
                "runner returned {} while stopping desktop",
                response.status()
            );
        }
        Ok(())
    }

    pub(crate) fn stream_request(
        &self,
        session_id: Uuid,
        control: bool,
    ) -> Result<axum::http::Request<()>> {
        let mut url = reqwest::Url::parse(&self.url)?;
        url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
            .map_err(|_| anyhow!("invalid runner URL scheme"))?;
        url.set_path(&format!("/v1/desktop-sessions/{session_id}/stream"));
        url.set_query(Some(if control {
            "control=true"
        } else {
            "control=false"
        }));
        let path = format!("/v1/desktop-sessions/{session_id}/stream?control={control}");
        let (timestamp, nonce, signature) = self.signature("GET", &path, &[])?;
        let mut request = url.as_str().into_client_request()?;
        request
            .headers_mut()
            .insert("x-cowork-timestamp", HeaderValue::from_str(&timestamp)?);
        request
            .headers_mut()
            .insert("x-cowork-nonce", HeaderValue::from_str(&nonce)?);
        request
            .headers_mut()
            .insert("x-cowork-signature", HeaderValue::from_str(&signature)?);
        Ok(request)
    }

    pub(crate) fn terminal_stream_request(
        &self,
        run_id: Uuid,
        workspace_id: Option<Uuid>,
        columns: u16,
        rows: u16,
    ) -> Result<axum::http::Request<()>> {
        let mut url = reqwest::Url::parse(&self.url)?;
        url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
            .map_err(|_| anyhow!("invalid runner URL scheme"))?;
        let path = crate::chat_workspaces::run_path(
            run_id,
            "terminal",
            &format!("columns={columns}&rows={rows}"),
            workspace_id,
        );
        let (route, query) = path.split_once('?').expect("workspace path query");
        url.set_path(route);
        url.set_query(Some(query));
        let (timestamp, nonce, signature) = self.signature("GET", &path, &[])?;
        let mut request = url.as_str().into_client_request()?;
        request
            .headers_mut()
            .insert("x-cowork-timestamp", HeaderValue::from_str(&timestamp)?);
        request
            .headers_mut()
            .insert("x-cowork-nonce", HeaderValue::from_str(&nonce)?);
        request
            .headers_mut()
            .insert("x-cowork-signature", HeaderValue::from_str(&signature)?);
        Ok(request)
    }

    pub(crate) async fn private_terminal_stream(
        &self,
        run_id: Uuid,
        workspace_id: Option<Uuid>,
        columns: u16,
        rows: u16,
    ) -> Result<
        Option<
            tokio_tungstenite::WebSocketStream<
                tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
            >,
        >,
    > {
        let Some(transport) = &self.proof else {
            return Ok(None);
        };
        let request = self.terminal_stream_request(run_id, workspace_id, columns, rows)?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        // Native TLS to the configured origin: no proxy or redirect traversal.
        // This supplies trust only, not physical-job/HA adoption authority.
        Ok(Some(transport.websocket(request, deadline).await?))
    }

    fn signature(
        &self,
        method: &str,
        path_and_query: &str,
        body: &[u8],
    ) -> Result<(String, String, String)> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs()
            .to_string();
        let nonce = Uuid::new_v4().to_string();
        let mut mac = HmacSha256::new_from_slice(&self.signing_key)
            .map_err(|_| anyhow!("invalid runner signing key"))?;
        mac.update(timestamp.as_bytes());
        mac.update(b"\n");
        mac.update(nonce.as_bytes());
        mac.update(b"\n");
        mac.update(method.as_bytes());
        mac.update(b"\n");
        mac.update(path_and_query.as_bytes());
        mac.update(b"\n");
        mac.update(body);
        Ok((timestamp, nonce, hex::encode(mac.finalize().into_bytes())))
    }
}

pub async fn start_session(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateDesktopSessionRequest>,
) -> Result<(StatusCode, Json<DesktopSession>), ApiError> {
    let run = db::get_run(&state.pool, run_id).await?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        run.spec.project_id,
        run.spec.thread_id,
        ProjectRole::Viewer,
    )
    .await?;
    if !matches!(
        run.state,
        RunState::Running | RunState::WaitingApproval | RunState::WaitingInput
    ) {
        return Err(ApiError::Conflict(
            "a desktop can only be started for an active run".to_owned(),
        ));
    }
    let managed_windows = matches!(
        run.spec.executor_target,
        ExecutorTarget::ManagedWindowsPool { .. }
    );
    let personal_device = matches!(
        run.spec.executor_target,
        ExecutorTarget::PersonalDevice { .. }
    );
    if !managed_windows
        && !personal_device
        && !matches!(run.spec.executor_target, ExecutorTarget::ServerLinux { .. })
    {
        return Err(ApiError::Unprocessable(
            "desktop sessions require a Linux server, managed Windows, or personal device target"
                .to_owned(),
        ));
    }
    let executor_id = run.assigned_executor_id.unwrap_or(Uuid::nil());
    let personal_mode = personal_device_remote_control_mode(
        &state.pool,
        &run.spec.executor_target,
        run.assigned_executor_id,
        principal.user_id,
    )
    .await?;
    if personal_mode == Some(PersonalDeviceRemoteControlMode::Off) {
        return Err(ApiError::Conflict(
            "remote desktop access is disabled on the personal device".to_owned(),
        ));
    }
    let mut external_platform = None;
    if managed_windows || personal_device {
        if executor_id.is_nil() {
            return Err(ApiError::Conflict(
                "the external run has no assigned executor".to_owned(),
            ));
        }
        let expected_kind = if managed_windows {
            "managed_windows"
        } else {
            "personal_device"
        };
        let registration = sqlx::query_scalar::<_, Value>(
            "SELECT registration FROM executors WHERE id = $1 AND kind = $2",
        )
        .bind(executor_id)
        .bind(expected_kind)
        .fetch_optional(&state.pool)
        .await?;
        let registration: ExecutorRegistration = registration
            .ok_or_else(|| {
                ApiError::Conflict("the assigned external executor is unavailable".to_owned())
            })
            .and_then(|value| {
                serde_json::from_value(value).map_err(|error| ApiError::Internal(error.into()))
            })?;
        let platform = external_desktop_platform(&registration, managed_windows);
        let Some(platform) = platform else {
            return Err(ApiError::Conflict(
                "the assigned executor does not provide a platform-consistent desktop.windows or desktop.linux capability".to_owned(),
            ));
        };
        external_platform = Some(platform.to_owned());
    }
    let session_id = Uuid::new_v4();
    let dimensions = DesktopDimensions {
        width: request.width,
        height: request.height,
        scale_factor: 1.0,
    };
    let created_at = Utc::now();
    let mut creation = state.pool.begin().await?;
    let start_lease_token = reserve_desktop_creation(&mut creation, run_id).await?;
    let start_attempt_id = Uuid::new_v4();
    crate::chat_workspaces::use_contract(&mut creation).await?;
    sqlx::query(
        "INSERT INTO desktop_sessions (id, run_id, executor_id, state, dimensions, created_at, runner_metadata) VALUES ($1, $2, $3, 'starting', $4, $5, $6)",
    )
    .bind(session_id)
    .bind(run_id)
    .bind(executor_id)
    .bind(serde_json::to_value(&dimensions)?)
    .bind(created_at)
    .bind(json!({"start_pending":true,"start_lease_token":start_lease_token,"start_attempt_id":start_attempt_id}))
    .execute(&mut *creation)
    .await?;
    creation.commit().await?;
    if let Some(cluster) = &state.api_cluster {
        // The creating API binds before the external side effect. A later API
        // may adopt an unbound/stale Linux owner only with a fresh job proof.
        crate::api_cluster::directory::bind_desktop(
            cluster,
            session_id,
            (managed_windows || personal_device).then_some(executor_id),
        )
        .await
        .map_err(|_| {
            ApiError::Conflict("desktop connection owner is unavailable or fenced".to_owned())
        })?;
    }
    if managed_windows || personal_device {
        finish_desktop_start(
            &state.pool,
            run_id,
            session_id,
            json!({
                "transport":"executor_reverse_ws","personal_device":personal_device,
                "platform":external_platform.as_deref(),"remote_control_mode":personal_mode,
            }),
            false,
        )
        .await?;
        return Ok((
            StatusCode::CREATED,
            Json(DesktopSession {
                schema_version: SCHEMA_VERSION,
                id: session_id,
                run_id,
                executor_id,
                state: DesktopSessionState::AgentControlled,
                stream_protocol: "rfb.binary.v1".to_owned(),
                dimensions: Some(dimensions),
                controller_user_id: None,
                created_at,
                ended_at: None,
            }),
        ));
    }
    let runner = state
        .runner
        .as_ref()
        .ok_or_else(|| ApiError::Conflict("the Linux GUI runner is not configured".to_owned()))?;
    let spec = SandboxDesktopSessionSpec {
        schema_version: SCHEMA_VERSION,
        session_id,
        run_id,
        workspace_id: crate::chat_workspaces::for_run(&state.pool, run_id).await?,
        dimensions: dimensions.clone(),
        network: SandboxNetwork::FilteredEgress,
        limits: SandboxLimits {
            memory_bytes: 4 * 1024 * 1024 * 1024,
            pids: 1024,
            ..SandboxLimits::default()
        },
    };
    match runner
        .start(&spec, start_lease_token, start_attempt_id)
        .await
    {
        Ok(metadata) => {
            finish_desktop_start(
                &state.pool,
                run_id,
                session_id,
                serde_json::to_value(metadata)?,
                false,
            )
            .await?;
        }
        Err(error) => {
            record_uncertain_start(&state.pool, session_id, &error)
                .await
                .map_err(ApiError::Internal)?;
            return Err(ApiError::Internal(error));
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(DesktopSession {
            schema_version: SCHEMA_VERSION,
            id: session_id,
            run_id,
            executor_id,
            state: DesktopSessionState::AgentControlled,
            stream_protocol: "rfb.binary.v1".to_owned(),
            dimensions: Some(dimensions),
            controller_user_id: None,
            created_at,
            ended_at: None,
        }),
    ))
}

pub async fn list_sessions(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<DesktopSession>>, ApiError> {
    let run = db::get_run(&state.pool, run_id).await?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        run.spec.project_id,
        run.spec.thread_id,
        ProjectRole::Viewer,
    )
    .await?;
    personal_device_remote_control_mode(
        &state.pool,
        &run.spec.executor_target,
        run.assigned_executor_id,
        principal.user_id,
    )
    .await?;
    let rows = sqlx::query("SELECT * FROM desktop_sessions WHERE run_id = $1 ORDER BY created_at")
        .bind(run_id)
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(
        rows.iter().map(row_to_session).collect::<Result<_, _>>()?,
    ))
}

pub async fn stop_session(
    State(state): State<AppState>,
    Path((run_id, session_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, ApiError> {
    let run = db::get_run(&state.pool, run_id).await?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        run.spec.project_id,
        run.spec.thread_id,
        ProjectRole::Viewer,
    )
    .await?;
    personal_device_remote_control_mode(
        &state.pool,
        &run.spec.executor_target,
        run.assigned_executor_id,
        principal.user_id,
    )
    .await?;
    let session = sqlx::query(
        "SELECT runner_metadata FROM desktop_sessions WHERE id = $1 AND run_id = $2 AND state NOT IN ('ended', 'failed')",
    )
    .bind(session_id)
    .bind(run_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(session) = session else {
        return Err(ApiError::NotFound(
            "active desktop session was not found".to_owned(),
        ));
    };
    if let Some(cluster) = &state.api_cluster {
        let metadata: Value = session.try_get("runner_metadata")?;
        if metadata.get("transport").and_then(Value::as_str) != Some("executor_reverse_ws") {
            let runner = state.runner.as_deref().ok_or_else(|| {
                ApiError::Conflict("the Linux GUI runner is not configured".into())
            })?;
            cluster
                .stop_linux_desktop(
                    session_id,
                    run_id,
                    principal.user_id,
                    principal.session_id.ok_or_else(|| {
                        ApiError::Unauthorized(
                            "cluster desktop stops require a live user session".into(),
                        )
                    })?,
                    runner,
                )
                .await
                .map_err(|_| {
                    ApiError::Conflict(
                        "physical desktop stop is unconfirmed or its connection owner is fenced"
                            .into(),
                    )
                })?;
            return Ok(StatusCode::NO_CONTENT);
        }
        let owner = crate::api_cluster::directory::desktop_owner(cluster, session_id)
            .await
            .map_err(|_| ApiError::Conflict("desktop owner is unavailable or fenced".to_owned()))?;
        if owner.executor_id.is_none() && owner != cluster.local_owner() {
            return Err(ApiError::Conflict(
                "Linux desktop stop requires its creating API owner".to_owned(),
            ));
        }
    }
    let _ = session;
    let rows = sqlx::query("SELECT id FROM desktop_sessions WHERE id=$1 AND run_id=$2 AND state NOT IN ('ended','failed')")
        .bind(session_id).bind(run_id).fetch_all(&state.pool).await?;
    finish_session_rows(&state.pool, run_id, rows, state.runner.as_deref())
        .await
        .map_err(ApiError::Internal)?;
    let mut tx = state.pool.begin().await?;
    audit(
        &mut tx,
        principal.user_id,
        "desktop_session.end",
        session_id,
        json!({"run_id": run_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn create_stream_ticket(
    State(state): State<AppState>,
    Path((run_id, session_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<DesktopStreamTicketRequest>,
) -> Result<Json<DesktopStreamTicket>, ApiError> {
    if state.api_cluster.is_some() && principal.session_id.is_none() {
        return Err(ApiError::Unauthorized(
            "cluster desktop streams require a user session".to_owned(),
        ));
    }
    let run = db::get_run(&state.pool, run_id).await?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        run.spec.project_id,
        run.spec.thread_id,
        ProjectRole::Viewer,
    )
    .await?;
    let personal_mode = personal_device_remote_control_mode(
        &state.pool,
        &run.spec.executor_target,
        run.assigned_executor_id,
        principal.user_id,
    )
    .await?;
    if personal_mode == Some(PersonalDeviceRemoteControlMode::Off) {
        return Err(ApiError::Conflict(
            "remote desktop access is disabled on the personal device".to_owned(),
        ));
    }
    let active = if request.control {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM desktop_sessions WHERE id = $1 AND run_id = $2 AND state IN ('agent_controlled','paused') AND COALESCE(runner_metadata->>'cleanup_state','')!='stop_pending' AND COALESCE(runner_metadata->>'start_pending','false')!='true')")
            .bind(session_id).bind(run_id).fetch_one(&state.pool).await?
    } else {
        // Observers remain view-only at the runner's dedicated RFB endpoint and
        // may continue watching while another authorized user is in control.
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM desktop_sessions WHERE id = $1 AND run_id = $2 AND state IN ('agent_controlled','user_controlled','paused') AND COALESCE(runner_metadata->>'cleanup_state','')!='stop_pending' AND COALESCE(runner_metadata->>'start_pending','false')!='true')")
            .bind(session_id).bind(run_id).fetch_one(&state.pool).await?
    };
    if !active {
        return Err(ApiError::Conflict(
            "desktop session is not available for a new stream".to_owned(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    if request.control {
        let reauth = request.reauthentication_token.as_deref().ok_or_else(|| {
            ApiError::Unauthorized("desktop control requires reauthentication".to_owned())
        })?;
        let digest = auth::opaque_token_hash(reauth);
        let session = principal.session_id.ok_or_else(|| {
            ApiError::Unauthorized("desktop control requires a user session".to_owned())
        })?;
        let consumed = sqlx::query("UPDATE reauthentication_grants SET used_at = now() WHERE token_hash = $1 AND user_id = $2 AND session_id = $3 AND purpose = 'desktop_control' AND used_at IS NULL AND expires_at > now() RETURNING token_hash")
            .bind(digest.as_slice()).bind(principal.user_id).bind(session).fetch_optional(&mut *tx).await?;
        if consumed.is_none() {
            return Err(ApiError::Unauthorized(
                "reauthentication grant is invalid or expired".to_owned(),
            ));
        }
    }
    let token = auth::random_token()?;
    let digest = auth::opaque_token_hash(&token);
    let expires_at = Utc::now() + chrono::Duration::minutes(1);
    sqlx::query("INSERT INTO desktop_stream_tickets (token_hash, desktop_session_id, user_id, control, expires_at, auth_session_id) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(digest.as_slice()).bind(session_id).bind(principal.user_id).bind(request.control).bind(expires_at).bind(principal.session_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(DesktopStreamTicket {
        schema_version: SCHEMA_VERSION,
        token,
        control: request.control,
        expires_at,
    }))
}

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    ticket: String,
}

pub async fn stream(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<StreamQuery>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let digest = auth::opaque_token_hash(&query.ticket);
    let row = sqlx::query("UPDATE desktop_stream_tickets SET used_at = now() WHERE token_hash = $1 AND desktop_session_id = $2 AND used_at IS NULL AND expires_at > now() RETURNING user_id, control, auth_session_id")
        .bind(digest.as_slice()).bind(session_id).fetch_optional(&state.pool).await?
        .ok_or_else(|| ApiError::Unauthorized("desktop stream ticket is invalid or expired".to_owned()))?;
    let user_id: Uuid = row.get("user_id");
    let control: bool = row.get("control");
    let auth_session: Option<Uuid> = row.try_get("auth_session_id")?;
    crate::organisation_mfa::session_pool(&state.pool, row.get("user_id"), auth_session).await?;
    if state.api_cluster.is_some() && auth_session.is_none() {
        return Err(ApiError::Unauthorized(
            "cluster desktop streams require a live user session".to_owned(),
        ));
    }
    let session = sqlx::query(
        "SELECT run_id, executor_id, runner_metadata FROM desktop_sessions WHERE id = $1 AND state IN ('agent_controlled','user_controlled','paused') AND COALESCE(runner_metadata->>'cleanup_state','')!='stop_pending' AND COALESCE(runner_metadata->>'start_pending','false')!='true'",
    )
    .bind(session_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::Conflict("the desktop session is no longer active".to_owned()))?;
    let run_id: Uuid = session.try_get("run_id")?;
    crate::organisation_mfa::optional_scope(
        user_id,
        auth_session,
        crate::fleet_content::interactive_access(
            &state.pool,
            user_id,
            run_id,
            if control {
                ProjectRole::Runner
            } else {
                ProjectRole::Viewer
            },
        ),
    )
    .await?;
    let executor_id: Uuid = session.try_get("executor_id")?;
    let metadata: Option<Value> = session.try_get("runner_metadata")?;
    let external = metadata
        .as_ref()
        .and_then(|value| value.get("transport"))
        .and_then(Value::as_str)
        == Some("executor_reverse_ws");
    let external_stream = if external && state.api_cluster.is_none() {
        Some(
            state
                .executor_hub
                .request_desktop_stream(executor_id, run_id, session_id, control)
                .await?,
        )
    } else {
        None
    };
    if control {
        let mut tx = state.pool.begin().await?;
        let run_id = sqlx::query_scalar::<_, Uuid>("UPDATE desktop_sessions SET state = 'user_controlled', controller_user_id = $2 WHERE id = $1 AND state IN ('agent_controlled','paused') AND COALESCE(runner_metadata->>'cleanup_state','')!='stop_pending' AND COALESCE(runner_metadata->>'start_pending','false')!='true' RETURNING run_id")
            .bind(session_id).bind(user_id).fetch_optional(&mut *tx).await?;
        let Some(run_id) = run_id else {
            return Err(ApiError::Conflict(
                "desktop already has a controller".to_owned(),
            ));
        };
        db::append_event_tx(
            &mut tx,
            run_id,
            RunEventKind::DesktopSessionChanged,
            json!({"session_id": session_id, "state": "user_controlled", "controller_user_id": user_id, "takeover_started": true}),
        )
        .await?;
        audit(
            &mut tx,
            user_id,
            "desktop_session.takeover_start",
            session_id,
            json!({"run_id": run_id, "started_at": Utc::now()}),
        )
        .await?;
        tx.commit().await?;
    }
    let pool = state.pool.clone();
    let api_replica = state.api_ownership();
    if let Some(cluster) = state.api_cluster.clone() {
        let prepared = async {
            if !external {
                let runner = state
                    .runner
                    .as_deref()
                    .ok_or_else(|| anyhow!("the Linux GUI runner is not configured"))?;
                crate::api_cluster::linux::ensure_owner(&cluster, session_id, runner).await?;
            }
            let pending = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                cluster.prepare_stream(
                    session_id,
                    run_id,
                    user_id,
                    auth_session.ok_or_else(|| anyhow!("desktop user session missing"))?,
                    control,
                    external.then_some(executor_id),
                ),
            )
            .await??;
            let peer = cluster.open_user_stream(&pending).await;
            match peer {
                Ok(peer) => Ok((pending, peer)),
                Err(error) => {
                    crate::api_cluster::tickets::cancel(&cluster, pending.stream_id).await;
                    Err(error)
                }
            }
        }
        .await;
        let (pending, peer) = match prepared {
            Ok(value) => value,
            Err(_) => {
                finish_control_relay(&pool, session_id, user_id, control, Utc::now(), 0, 0, None)
                    .await;
                return Err(ApiError::Conflict(
                    "private desktop owner or one-use bridge is unavailable".to_owned(),
                ));
            }
        };
        return Ok(upgrade.on_upgrade(move |socket| async move {
            let started = Utc::now();
            let outcome = crate::organisation_mfa::guard_pool(
                &pool,
                user_id,
                auth_session,
                crate::runtime_status::guard_api_future(
                    api_replica,
                    crate::api_cluster::tickets::guard(
                        &cluster,
                        &pending,
                        crate::api_cluster::relay::user(socket, peer, control),
                    ),
                ),
            )
            .await;
            crate::api_cluster::tickets::cancel(&cluster, pending.stream_id).await;
            let (messages, bytes, digest) = outcome.unwrap_or((0, 0, None));
            finish_control_relay(
                &pool, session_id, user_id, control, started, messages, bytes, digest,
            )
            .await;
        }));
    }
    if let Some((stream_id, receiver)) = external_stream {
        let hub = state.executor_hub.clone();
        Ok(upgrade.on_upgrade(move |socket| {
            crate::organisation_mfa::optional_scope(
                user_id,
                auth_session,
                relay_external(
                    socket,
                    receiver,
                    hub,
                    stream_id,
                    pool,
                    session_id,
                    run_id,
                    user_id,
                    control,
                    api_replica,
                ),
            )
        }))
    } else {
        let runner = state.runner.clone().ok_or_else(|| {
            ApiError::Conflict("the Linux GUI runner is not configured".to_owned())
        })?;
        Ok(upgrade.on_upgrade(move |socket| {
            crate::organisation_mfa::optional_scope(
                user_id,
                auth_session,
                relay(
                    socket,
                    runner,
                    pool,
                    session_id,
                    run_id,
                    user_id,
                    control,
                    api_replica,
                ),
            )
        }))
    }
}

async fn guard_desktop_session<T>(
    pool: &PgPool,
    session_id: Uuid,
    operation: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::pin!(operation);
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _=interval.tick()=>{
                let available=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM desktop_sessions WHERE id=$1 AND state IN ('agent_controlled','user_controlled','paused') AND COALESCE(runner_metadata->>'cleanup_state','')!='stop_pending' AND COALESCE(runner_metadata->>'start_pending','false')!='true')")
                    .bind(session_id).fetch_one(pool).await?;
                if !available{bail!("desktop session stopped or requires cleanup; interactive connection closed");}
            },
            outcome=&mut operation=>return outcome,
        }
    }
}

/// Rechecked at both private bridge endpoints, including the executor callback
/// API. A ticket conveys no authority after its user/fleet/session policy changes.
pub(crate) async fn cluster_stream_access(
    pool: &PgPool,
    session: Uuid,
    run_id: Uuid,
    user: Uuid,
    control: bool,
) -> Result<()> {
    crate::fleet_content::interactive_access(
        pool,
        user,
        run_id,
        if control {
            ProjectRole::Runner
        } else {
            ProjectRole::Viewer
        },
    )
    .await?;
    let run = db::get_run(pool, run_id).await?;
    if !matches!(
        run.state,
        RunState::Running | RunState::WaitingApproval | RunState::WaitingInput
    ) {
        bail!("desktop run is no longer active");
    }
    let available:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM desktop_sessions WHERE id=$1 AND run_id=$2 AND state IN ('agent_controlled','user_controlled','paused') AND (NOT $3 OR (state='user_controlled' AND controller_user_id=$4)) AND COALESCE(runner_metadata->>'cleanup_state','')!='stop_pending' AND COALESCE(runner_metadata->>'start_pending','false')!='true')")
        .bind(session).bind(run_id).bind(control).bind(user).fetch_one(pool).await?;
    if !available {
        bail!("desktop session/controller is no longer available");
    }
    if personal_device_remote_control_mode(
        pool,
        &run.spec.executor_target,
        run.assigned_executor_id,
        user,
    )
    .await?
        == Some(PersonalDeviceRemoteControlMode::Off)
    {
        bail!("personal desktop access was disabled");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn relay(
    socket: WebSocket,
    runner: Arc<RunnerControl>,
    pool: PgPool,
    session_id: Uuid,
    run_id: Uuid,
    user_id: Uuid,
    control: bool,
    api_replica: Option<crate::runtime_status::ApiOwnership>,
) {
    let started = Utc::now();
    let outcome = crate::runtime_status::guard_api_future(
        api_replica,
        crate::fleet_content::guard_interactive(
            &pool,
            user_id,
            run_id,
            if control {
                ProjectRole::Runner
            } else {
                ProjectRole::Viewer
            },
            guard_desktop_session(
                &pool,
                session_id,
                relay_inner(socket, &runner, &pool, session_id, control),
            ),
        ),
    )
    .await;
    let (input_messages, input_bytes, input_digest) = match outcome {
        Ok(summary) => summary,
        Err(error) => {
            tracing::warn!(?error, %session_id, "desktop relay failed");
            (0, 0, None)
        }
    };
    finish_control_relay(
        &pool,
        session_id,
        user_id,
        control,
        started,
        input_messages,
        input_bytes,
        input_digest,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn relay_external(
    socket: WebSocket,
    receiver: tokio::sync::oneshot::Receiver<WebSocket>,
    hub: crate::executor_ws::ExecutorHub,
    stream_id: Uuid,
    pool: PgPool,
    session_id: Uuid,
    run_id: Uuid,
    user_id: Uuid,
    control: bool,
    api_replica: Option<crate::runtime_status::ApiOwnership>,
) {
    let started = Utc::now();
    // A personal device may be waiting for its owner to answer a native local
    // confirmation dialog. Keep the reverse channel bounded without turning a
    // normal human response delay into an immediate stream failure.
    let outcome = crate::runtime_status::guard_api_future(
        api_replica,
        crate::fleet_content::guard_interactive(
            &pool,
            user_id,
            run_id,
            if control {
                ProjectRole::Runner
            } else {
                ProjectRole::Viewer
            },
            guard_desktop_session(&pool, session_id, async {
                match tokio::time::timeout(std::time::Duration::from_secs(300), receiver).await {
                    Ok(Ok(executor_socket)) => {
                        relay_external_inner(socket, executor_socket, control).await
                    }
                    Ok(Err(_)) => Err(anyhow!("executor desktop stream request was canceled")),
                    Err(_) => {
                        hub.cancel_desktop_stream(stream_id).await;
                        Err(anyhow!("executor did not open its reverse desktop stream"))
                    }
                }
            }),
        ),
    )
    .await;
    hub.cancel_desktop_stream(stream_id).await;
    let (input_messages, input_bytes, input_digest) = match outcome {
        Ok(summary) => summary,
        Err(error) => {
            tracing::warn!(?error, %session_id, "external desktop relay failed");
            (0, 0, None)
        }
    };
    finish_control_relay(
        &pool,
        session_id,
        user_id,
        control,
        started,
        input_messages,
        input_bytes,
        input_digest,
    )
    .await;
}

async fn relay_external_inner(
    client: WebSocket,
    executor: WebSocket,
    control: bool,
) -> Result<(u64, u64, Option<String>)> {
    let (mut client_send, mut client_recv) = client.split();
    let (mut executor_send, mut executor_recv) = executor.split();
    let mut messages = 0_u64;
    let mut bytes = 0_u64;
    let mut digest = Sha256::new();
    loop {
        tokio::select! {
            incoming = client_recv.next() => match incoming {
                Some(Ok(Message::Binary(data))) => {
                    if control { messages += 1; bytes += data.len() as u64; digest.update(&data); }
                    executor_send.send(Message::Binary(data)).await?;
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(Message::Ping(data))) => client_send.send(Message::Pong(data)).await?,
                Some(Err(error)) => return Err(error.into()),
                _ => {}
            },
            incoming = executor_recv.next() => match incoming {
                Some(Ok(Message::Binary(data))) => client_send.send(Message::Binary(data)).await?,
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(Message::Ping(data))) => executor_send.send(Message::Pong(data)).await?,
                Some(Err(error)) => return Err(error.into()),
                _ => {}
            }
        }
    }
    Ok((
        messages,
        bytes,
        control.then(|| hex::encode(digest.finalize())),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn finish_control_relay(
    pool: &PgPool,
    session_id: Uuid,
    user_id: Uuid,
    control: bool,
    started: chrono::DateTime<Utc>,
    input_messages: u64,
    input_bytes: u64,
    input_digest: Option<String>,
) {
    if !control {
        return;
    }
    if let Ok(mut tx) = pool.begin().await {
        let run_id = sqlx::query_scalar::<_, Uuid>("UPDATE desktop_sessions SET state = 'agent_controlled', controller_user_id = NULL WHERE id = $1 AND state = 'user_controlled' RETURNING run_id")
            .bind(session_id).fetch_optional(&mut *tx).await.ok().flatten();
        if let Some(run_id) = run_id {
            let _ = db::append_event_tx(&mut tx, run_id, RunEventKind::DesktopSessionChanged, json!({"session_id": session_id, "state": "agent_controlled", "takeover_ended": true})).await;
            if input_messages > 0 {
                let _ = audit(&mut tx, user_id, "desktop_session.input_summary", session_id, json!({"run_id": run_id, "input_messages": input_messages, "input_bytes": input_bytes, "input_digest": input_digest})).await;
            }
            let _ = audit(
                &mut tx,
                user_id,
                "desktop_session.takeover_end",
                session_id,
                json!({"run_id": run_id, "started_at": started, "ended_at": Utc::now()}),
            )
            .await;
            let _ = tx.commit().await;
        }
    }
}

async fn relay_inner(
    socket: WebSocket,
    runner: &RunnerControl,
    pool: &PgPool,
    session_id: Uuid,
    control: bool,
) -> Result<(u64, u64, Option<String>)> {
    if runner.proof.is_some() {
        let binding = crate::api_cluster::linux::binding(pool, session_id).await?;
        return crate::api_cluster::linux::guard_job(pool, &binding, async {
            let remote = runner.proven_stream(&binding, control).await?;
            relay_runner_socket(socket, remote, control).await
        })
        .await;
    }
    let request = runner.stream_request(session_id, control)?;
    let (runner_socket, _) = tokio_tungstenite::connect_async(request).await?;
    relay_runner_socket(socket, runner_socket, control).await
}

async fn relay_runner_socket<S>(
    socket: WebSocket,
    runner_socket: tokio_tungstenite::WebSocketStream<S>,
    control: bool,
) -> Result<(u64, u64, Option<String>)>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    let (mut client_send, mut client_recv) = socket.split();
    let (mut runner_send, mut runner_recv) = runner_socket.split();
    let mut messages = 0_u64;
    let mut bytes = 0_u64;
    let mut digest = Sha256::new();
    loop {
        tokio::select! {
            incoming = client_recv.next() => match incoming {
                Some(Ok(Message::Binary(data))) => {
                    if control { messages += 1; bytes += data.len() as u64; digest.update(&data); }
                    runner_send.send(RunnerMessage::Binary(data.to_vec().into())).await?;
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(Message::Ping(data))) => client_send.send(Message::Pong(data)).await?,
                Some(Err(error)) => return Err(error.into()),
                _ => {}
            },
            incoming = runner_recv.next() => match incoming {
                Some(Ok(RunnerMessage::Binary(data))) => client_send.send(Message::Binary(data.to_vec().into())).await?,
                Some(Ok(RunnerMessage::Close(_))) | None => break,
                Some(Err(error)) => return Err(error.into()),
                _ => {}
            }
        }
    }
    Ok((
        messages,
        bytes,
        control.then(|| hex::encode(digest.finalize())),
    ))
}

fn row_to_session(row: &sqlx::postgres::PgRow) -> Result<DesktopSession, ApiError> {
    let state = match row.get::<String, _>("state").as_str() {
        "starting" => DesktopSessionState::Starting,
        "agent_controlled" => DesktopSessionState::AgentControlled,
        "user_controlled" => DesktopSessionState::UserControlled,
        "paused" => DesktopSessionState::Paused,
        "ended" => DesktopSessionState::Ended,
        "failed" => DesktopSessionState::Failed,
        other => {
            return Err(ApiError::Internal(anyhow!(
                "invalid desktop session state {other}"
            )))
        }
    };
    Ok(DesktopSession {
        schema_version: SCHEMA_VERSION,
        id: row.get("id"),
        run_id: row.get("run_id"),
        executor_id: row.get("executor_id"),
        state,
        stream_protocol: row.get("stream_protocol"),
        dimensions: row
            .try_get::<Option<Value>, _>("dimensions")?
            .map(serde_json::from_value)
            .transpose()?,
        controller_user_id: row.get("controller_user_id"),
        created_at: row.get("created_at"),
        ended_at: row.get("ended_at"),
    })
}

async fn personal_device_remote_control_mode(
    pool: &PgPool,
    target: &ExecutorTarget,
    assigned_executor_id: Option<Uuid>,
    user_id: Uuid,
) -> Result<Option<PersonalDeviceRemoteControlMode>, ApiError> {
    let ExecutorTarget::PersonalDevice { device_id } = target else {
        return Ok(None);
    };
    if assigned_executor_id.is_some_and(|assigned| assigned != *device_id) {
        return Err(ApiError::Conflict(
            "the personal-device run is not assigned to its target device".to_owned(),
        ));
    }
    let registration = sqlx::query_scalar::<_, Value>(
        "SELECT registration FROM executors WHERE id = $1 AND kind = 'personal_device' AND owner_user_id = $2",
    )
    .bind(device_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        ApiError::Unauthorized(
            "personal desktop sessions are available only to the device owner".to_owned(),
        )
    })?;
    let registration: ExecutorRegistration = serde_json::from_value(registration)?;
    Ok(Some(
        registration
            .personal_device_remote_control
            .unwrap_or_default(),
    ))
}

async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    action: &str,
    target_id: Uuid,
    metadata: Value,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, $3, 'desktop_session', $4, $5)")
        .bind(Uuid::new_v4()).bind(user_id).bind(action).bind(target_id).bind(metadata).execute(&mut **tx).await?;
    Ok(())
}

/// Serialize desktop creation with run completion and reject uncertain remote
/// operations. A new session always receives a new runner identity.
async fn reserve_desktop_creation(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Uuid, ApiError> {
    let active = sqlx::query_scalar::<_, Uuid>(
        "SELECT lease_token FROM runs WHERE id=$1 AND state IN ('running','waiting_approval','waiting_input') AND lease_expires_at>now() FOR UPDATE",
    ).bind(run_id).fetch_optional(&mut **tx).await?;
    if active.is_none() {
        return Err(ApiError::Conflict(
            "a desktop requires a current active run lease".to_owned(),
        ));
    }
    let uncertain = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM desktop_sessions WHERE run_id=$1 AND state IN ('starting','agent_controlled','user_controlled','paused') AND (state='starting' OR runner_metadata->>'cleanup_state'='stop_pending' OR runner_metadata->>'start_pending'='true'))",
    ).bind(run_id).fetch_one(&mut **tx).await?;
    if uncertain {
        return Err(ApiError::Conflict("desktop start or stop is unconfirmed; reconcile the existing session before starting another".to_owned()));
    }
    Ok(active.expect("active lease was verified"))
}

/// The durable start_pending marker survives cancellation of the HTTP request.
/// A cleanup cannot mistake a 404 during an in-flight start for a stopped GUI.
async fn finish_desktop_start(
    pool: &PgPool,
    run_id: Uuid,
    session_id: Uuid,
    metadata: Value,
    automatic: bool,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    let (active, lease_token) = sqlx::query_as::<_, (bool,Option<Uuid>)>(
        "SELECT state IN ('running','waiting_approval','waiting_input') AND lease_expires_at>now(),lease_token FROM runs WHERE id=$1 FOR UPDATE",
    ).bind(run_id).fetch_one(&mut *tx).await?;
    let row = sqlx::query(
        "SELECT state,runner_metadata FROM desktop_sessions WHERE id=$1 AND run_id=$2 FOR UPDATE",
    )
    .bind(session_id)
    .bind(run_id)
    .fetch_one(&mut *tx)
    .await?;
    let state: String = row.try_get("state")?;
    let mut current: Value = row
        .try_get::<Option<Value>, _>("runner_metadata")?
        .unwrap_or_else(|| json!({}));
    let stopping = current.get("cleanup_state").and_then(Value::as_str) == Some("stop_pending");
    let same_lease = current
        .get("start_lease_token")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        == lease_token
        && lease_token.is_some();
    let allowed = active && same_lease && state == "starting" && !stopping;
    if let (Some(current), Some(metadata)) = (current.as_object_mut(), metadata.as_object()) {
        current.extend(metadata.clone());
    }
    current["start_pending"] = json!(false);
    if !allowed {
        current["cleanup_state"] = json!("stop_pending");
    }
    sqlx::query("UPDATE desktop_sessions SET state=CASE WHEN $3 THEN 'agent_controlled' ELSE state END,runner_metadata=$2 WHERE id=$1")
        .bind(session_id).bind(current).bind(allowed).execute(&mut *tx).await?;
    if allowed {
        db::append_event_tx(
            &mut tx,
            run_id,
            RunEventKind::DesktopSessionChanged,
            json!({"session_id":session_id,"state":"agent_controlled","automatic":automatic}),
        )
        .await?;
    }
    tx.commit().await?;
    if !allowed {
        return Err(ApiError::Conflict(
            "desktop start finished after its run or session was stopped; cleanup remains pending"
                .to_owned(),
        ));
    }
    Ok(())
}

async fn record_uncertain_start(
    pool: &PgPool,
    session_id: Uuid,
    error: &anyhow::Error,
) -> Result<()> {
    sqlx::query("UPDATE desktop_sessions SET runner_metadata=COALESCE(runner_metadata,'{}'::jsonb)||$2 WHERE id=$1 AND state='starting'")
        .bind(session_id).bind(json!({"start_error":error.to_string(),"cleanup_state":"stop_pending"}))
        .execute(pool).await?;
    Ok(())
}

pub async fn ensure_worker_session(
    pool: &PgPool,
    runner: &RunnerControl,
    run_id: Uuid,
    executor_id: Uuid,
    lease_token: Uuid,
) -> Result<Uuid> {
    let mut creation = pool.begin().await?;
    db::verify_lease(&mut creation, run_id, executor_id, lease_token).await?;
    reserve_desktop_creation(&mut creation, run_id).await?;
    if let Some(id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM desktop_sessions WHERE run_id=$1 AND state IN ('agent_controlled','user_controlled','paused') AND COALESCE(runner_metadata->>'cleanup_state','')!='stop_pending' AND COALESCE(runner_metadata->>'start_pending','false')!='true'",
    ).bind(run_id).fetch_optional(&mut *creation).await? {
        creation.commit().await?;
        return Ok(id);
    }
    let session_id = Uuid::new_v4();
    let start_attempt_id = Uuid::new_v4();
    let dimensions = DesktopDimensions {
        width: 1440,
        height: 900,
        scale_factor: 1.0,
    };
    crate::chat_workspaces::use_contract(&mut creation).await?;
    sqlx::query("INSERT INTO desktop_sessions(id,run_id,executor_id,state,dimensions,runner_metadata) VALUES($1,$2,$3,'starting',$4,$5)")
        .bind(session_id).bind(run_id).bind(executor_id).bind(serde_json::to_value(&dimensions)?)
        .bind(json!({"start_pending":true,"start_lease_token":lease_token,"start_attempt_id":start_attempt_id})).execute(&mut *creation).await?;
    creation.commit().await?;
    let spec = SandboxDesktopSessionSpec {
        schema_version: cowork_contracts::SCHEMA_VERSION,
        session_id,
        run_id,
        workspace_id: crate::chat_workspaces::for_run(pool, run_id).await?,
        dimensions,
        network: SandboxNetwork::FilteredEgress,
        limits: SandboxLimits {
            memory_bytes: 4 * 1024 * 1024 * 1024,
            pids: 1024,
            ..SandboxLimits::default()
        },
    };
    match runner.start(&spec, lease_token, start_attempt_id).await {
        Ok(metadata) => {
            finish_desktop_start(
                pool,
                run_id,
                session_id,
                serde_json::to_value(metadata)?,
                true,
            )
            .await?;
            Ok(session_id)
        }
        Err(error) => {
            record_uncertain_start(pool, session_id, &error).await?;
            Err(error)
        }
    }
}

struct PreparedStop {
    id: Uuid,
    attempt: Uuid,
    external: bool,
    start_pending: bool,
}

/// This marker MUST commit before remote DELETE. If the caller is aborted or
/// the response is lost, later workers cannot adopt the affected desktop.
async fn prepare_session_stops(
    tx: &mut Transaction<'_, Postgres>,
    rows: Vec<sqlx::postgres::PgRow>,
) -> Result<Vec<PreparedStop>> {
    let mut prepared = Vec::new();
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let state: String = row.try_get("state")?;
        let metadata: Value = row
            .try_get::<Option<Value>, _>("runner_metadata")?
            .unwrap_or_else(|| json!({}));
        let attempt = Uuid::new_v4();
        let external =
            metadata.get("transport").and_then(Value::as_str) == Some("executor_reverse_ws");
        let start_pending = !external
            && metadata
                .get("start_pending")
                .and_then(Value::as_bool)
                .unwrap_or(state == "starting");
        sqlx::query("UPDATE desktop_sessions SET runner_metadata=COALESCE(runner_metadata,'{}'::jsonb)||$2 WHERE id=$1")
            .bind(id).bind(json!({"cleanup_state":"stop_pending","cleanup_attempt":attempt}))
            .execute(&mut **tx).await?;
        prepared.push(PreparedStop {
            id,
            attempt,
            external,
            start_pending,
        });
    }
    Ok(prepared)
}

pub async fn end_worker_leased_sessions(
    pool: &PgPool,
    runner: &RunnerControl,
    run_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    db::verify_lease(&mut tx, run_id, worker_id, lease_token).await?;
    let rows=sqlx::query("SELECT id,state,runner_metadata FROM desktop_sessions WHERE run_id=$1 AND state IN ('starting','agent_controlled','user_controlled','paused') FOR UPDATE")
        .bind(run_id).fetch_all(&mut *tx).await?;
    let prepared = prepare_session_stops(&mut tx, rows).await?;
    tx.commit().await?;
    finish_prepared_stops(pool, run_id, prepared, Some(runner)).await
}

pub async fn end_external_sessions(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let rows=sqlx::query("SELECT id FROM desktop_sessions WHERE run_id=$1 AND state IN ('starting','agent_controlled','user_controlled','paused') AND runner_metadata->>'transport'='executor_reverse_ws'")
        .bind(run_id).fetch_all(pool).await?;
    finish_session_rows(pool, run_id, rows, None).await
}

pub async fn end_sessions(
    pool: &PgPool,
    runner: Option<&RunnerControl>,
    run_id: Uuid,
) -> Result<()> {
    let rows=sqlx::query("SELECT id FROM desktop_sessions WHERE run_id=$1 AND state IN ('starting','agent_controlled','user_controlled','paused')")
        .bind(run_id).fetch_all(pool).await?;
    finish_session_rows(pool, run_id, rows, runner).await
}

pub async fn reap_terminal_sessions(
    pool: &PgPool,
    runner: Option<&RunnerControl>,
) -> Result<usize> {
    let run_ids=sqlx::query_scalar::<_,Uuid>("SELECT DISTINCT session.run_id FROM desktop_sessions session JOIN runs run ON run.id=session.run_id WHERE session.state IN ('starting','agent_controlled','user_controlled','paused') AND run.state IN ('interrupted','completed','failed','canceled','expired') LIMIT 100")
        .fetch_all(pool).await?;
    let mut count = 0;
    for run_id in run_ids {
        let mut tx = pool.begin().await?;
        let terminal=sqlx::query_scalar::<_,Uuid>("SELECT id FROM runs WHERE id=$1 AND state IN ('interrupted','completed','failed','canceled','expired') FOR UPDATE")
            .bind(run_id).fetch_optional(&mut *tx).await?;
        if terminal.is_none() {
            continue;
        }
        let rows=sqlx::query("SELECT id,state,runner_metadata FROM desktop_sessions WHERE run_id=$1 AND state IN ('starting','agent_controlled','user_controlled','paused') FOR UPDATE")
            .bind(run_id).fetch_all(&mut *tx).await?;
        let prepared = prepare_session_stops(&mut tx, rows).await?;
        tx.commit().await?;
        match finish_prepared_stops(pool, run_id, prepared, runner).await {
            Ok(()) => count += 1,
            Err(error) => {
                tracing::warn!(?error,%run_id,"terminal desktop cleanup remains unconfirmed")
            }
        }
    }
    Ok(count)
}

async fn finish_session_rows(
    pool: &PgPool,
    run_id: Uuid,
    rows: Vec<sqlx::postgres::PgRow>,
    runner: Option<&RunnerControl>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT id FROM runs WHERE id=$1 FOR UPDATE")
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await?;
    let ids: Vec<Uuid> = rows
        .iter()
        .map(|row| row.try_get("id"))
        .collect::<Result<_, _>>()?;
    let current=sqlx::query("SELECT id,state,runner_metadata FROM desktop_sessions WHERE run_id=$1 AND id=ANY($2) AND state IN ('starting','agent_controlled','user_controlled','paused') FOR UPDATE")
        .bind(run_id).bind(ids).fetch_all(&mut *tx).await?;
    let prepared = prepare_session_stops(&mut tx, current).await?;
    tx.commit().await?;
    finish_prepared_stops(pool, run_id, prepared, runner).await
}

async fn finish_prepared_stops(
    pool: &PgPool,
    run_id: Uuid,
    prepared: Vec<PreparedStop>,
    runner: Option<&RunnerControl>,
) -> Result<()> {
    finish_prepared_stops_with(pool, run_id, prepared, |id| async move {
        let runner = runner.ok_or_else(|| {
            anyhow!("cannot confirm a Linux desktop stop without a configured runner")
        })?;
        let value: Option<Value> = sqlx::query_scalar(
            "SELECT runner_metadata->'runner_job_binding' FROM desktop_sessions WHERE id=$1",
        )
        .bind(id)
        .fetch_one(pool)
        .await?;
        let stop = async {
            if let Some(value) = value {
                let binding: crate::runner_desktop_proof::JobBinding =
                    serde_json::from_value(value)?;
                if binding.session_id != id || binding.run_id != run_id {
                    bail!("desktop stop physical identity changed");
                }
                runner.stop_bound_desktop(&binding).await
            } else {
                runner.stop(id).await
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(15), stop)
            .await
            .map_err(|_| anyhow!("runner desktop stop was not confirmed before timeout"))??;
        Ok(())
    })
    .await
}

pub(crate) async fn stop_cluster_session(
    pool: &PgPool,
    runner: &RunnerControl,
    run: Uuid,
    session: Uuid,
    user: Uuid,
    expected_digest: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT id FROM runs WHERE id=$1 FOR UPDATE")
        .bind(run)
        .fetch_one(&mut *tx)
        .await?;
    let rows=sqlx::query("SELECT id,state,runner_metadata FROM desktop_sessions WHERE id=$1 AND run_id=$2 AND state IN ('agent_controlled','user_controlled','paused') FOR UPDATE")
        .bind(session).bind(run).fetch_all(&mut *tx).await?;
    if rows.len() != 1 {
        bail!("desktop stop session is no longer available");
    }
    let metadata: Value = rows[0].try_get("runner_metadata")?;
    let binding: crate::runner_desktop_proof::JobBinding =
        serde_json::from_value(metadata["runner_job_binding"].clone())?;
    if binding.digest()? != expected_digest
        || metadata["start_pending"] != false
        || metadata["cleanup_state"] == "stop_pending"
    {
        bail!("desktop stop physical identity or cleanup state changed");
    }
    let prepared = prepare_session_stops(&mut tx, rows).await?;
    tx.commit().await?;
    finish_prepared_stops(pool, run, prepared, Some(runner)).await?;
    let mut tx = pool.begin().await?;
    audit(
        &mut tx,
        user,
        "desktop_session.end",
        session,
        json!({"run_id":run,"physical_identity_confirmed":true}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn finish_prepared_stops_with<F, Fut>(
    pool: &PgPool,
    run_id: Uuid,
    prepared: Vec<PreparedStop>,
    mut stop_remote: F,
) -> Result<()>
where
    F: FnMut(Uuid) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    for stop in prepared {
        if stop.start_pending {
            bail!("desktop start is unconfirmed; a stop response cannot prove the session has stopped");
        }
        if !stop.external {
            stop_remote(stop.id).await?;
        }
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT id FROM runs WHERE id=$1 FOR UPDATE")
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await?;
        let changed=sqlx::query("UPDATE desktop_sessions SET state='ended',controller_user_id=NULL,ended_at=now(),runner_metadata=runner_metadata||'{\"cleanup_state\":\"confirmed\"}'::jsonb WHERE id=$1 AND run_id=$2 AND runner_metadata->>'cleanup_attempt'=$3 AND state IN ('starting','agent_controlled','user_controlled','paused')")
            .bind(stop.id).bind(run_id).bind(stop.attempt.to_string()).execute(&mut *tx).await?.rows_affected();
        if changed == 1 {
            db::append_event_tx(
                &mut tx,
                run_id,
                RunEventKind::DesktopSessionChanged,
                json!({"session_id":stop.id,"state":"ended","automatic":true}),
            )
            .await?;
        }
        tx.commit().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cowork_contracts::{
        Capability, CapabilityDescriptor, ExecutorKind, ExecutorRegistration,
        PersonalDeviceRemoteControlMode, SCHEMA_VERSION,
    };
    use uuid::Uuid;

    use super::{
        external_desktop_platform, finish_desktop_start, finish_prepared_stops_with,
        prepare_session_stops, reserve_desktop_creation,
    };

    async fn isolated_desktop_pool() -> sqlx::PgPool {
        let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated test database URL");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        // One connection owns session-local tables. Public application data and
        // migrations remain untouched even when the test fails.
        sqlx::raw_sql(r#"
            CREATE TEMP TABLE runs(id uuid PRIMARY KEY,state text,lease_expires_at timestamptz,lease_token uuid DEFAULT '00000000-0000-0000-0000-000000000001',next_event_sequence bigint DEFAULT 1);
            CREATE TEMP TABLE desktop_sessions(id uuid PRIMARY KEY,run_id uuid,state text,runner_metadata jsonb,controller_user_id uuid,ended_at timestamptz);
            CREATE TEMP TABLE run_events(run_id uuid,sequence bigint,event_id uuid,kind text,payload jsonb,created_at timestamptz);
        "#).execute(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_canceled_stop_remains_durable_and_forbids_desktop_adoption() {
        let pool = isolated_desktop_pool().await;
        let run = Uuid::new_v4();
        let session = Uuid::new_v4();
        sqlx::query("INSERT INTO runs(id,state,lease_expires_at) VALUES($1,'running',now()+interval '60 seconds')")
            .bind(run).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO desktop_sessions(id,run_id,state,runner_metadata) VALUES($1,$2,'agent_controlled','{\"start_pending\":false}')")
            .bind(session).bind(run).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let rows = sqlx::query(
            "SELECT id,state,runner_metadata FROM desktop_sessions WHERE id=$1 FOR UPDATE",
        )
        .bind(session)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        let prepared = prepare_session_stops(&mut tx, rows).await.unwrap();
        tx.commit().await.unwrap();
        let (dispatched, observed) = tokio::sync::oneshot::channel();
        let cleanup_pool = pool.clone();
        let mut dispatched = Some(dispatched);
        let cleanup = tokio::spawn(async move {
            finish_prepared_stops_with(&cleanup_pool, run, prepared, move |_| {
                let dispatched = dispatched.take();
                async move {
                    dispatched.unwrap().send(()).unwrap();
                    std::future::pending::<anyhow::Result<()>>().await
                }
            })
            .await
        });
        observed.await.unwrap();
        cleanup.abort();
        assert!(cleanup.await.unwrap_err().is_cancelled());
        let metadata: serde_json::Value =
            sqlx::query_scalar("SELECT runner_metadata FROM desktop_sessions WHERE id=$1")
                .bind(session)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(metadata["cleanup_state"], "stop_pending");
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT state FROM desktop_sessions WHERE id=$1")
                .bind(session)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "agent_controlled"
        );
        let mut resumed = pool.begin().await.unwrap();
        assert!(
            reserve_desktop_creation(&mut resumed, run).await.is_err(),
            "a resumed worker cannot adopt the desktop affected by a late stop"
        );
        resumed.rollback().await.unwrap();
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_late_start_does_not_resurrect_a_terminal_run_or_allow_capacity_release() {
        let pool = isolated_desktop_pool().await;
        let run = Uuid::new_v4();
        let session = Uuid::new_v4();
        sqlx::query("INSERT INTO runs(id,state,lease_expires_at) VALUES($1,'running',now()+interval '60 seconds')")
            .bind(run).execute(&pool).await.unwrap();
        let mut creation = pool.begin().await.unwrap();
        reserve_desktop_creation(&mut creation, run).await.unwrap();
        sqlx::query("INSERT INTO desktop_sessions(id,run_id,state,runner_metadata) VALUES($1,$2,'starting','{\"start_pending\":true}')")
            .bind(session).bind(run).execute(&mut *creation).await.unwrap();
        creation.commit().await.unwrap();
        // Completion obtains the same run lock and sees the durable pending
        // start before it can accept a server_execution_stopped proof.
        let mut completion = pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM runs WHERE id=$1 FOR UPDATE")
            .bind(run)
            .fetch_one(&mut *completion)
            .await
            .unwrap();
        assert!(
            !sqlx::query_scalar::<_, bool>(crate::db::SERVER_DESKTOPS_STOPPED_SQL)
                .bind(run)
                .fetch_one(&mut *completion)
                .await
                .unwrap()
        );
        sqlx::query("UPDATE runs SET state='completed' WHERE id=$1")
            .bind(run)
            .execute(&mut *completion)
            .await
            .unwrap();
        completion.commit().await.unwrap();
        assert!(finish_desktop_start(
            &pool,
            run,
            session,
            serde_json::json!({"container_name":"late-start"}),
            true
        )
        .await
        .is_err());
        let metadata: serde_json::Value =
            sqlx::query_scalar("SELECT runner_metadata FROM desktop_sessions WHERE id=$1")
                .bind(session)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(metadata["start_pending"], false);
        assert_eq!(metadata["cleanup_state"], "stop_pending");
        let mut retry = pool.begin().await.unwrap();
        assert!(reserve_desktop_creation(&mut retry, run).await.is_err());
        retry.rollback().await.unwrap();
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_unknown_start_is_never_stopped_or_reported_as_confirmed() {
        let pool = isolated_desktop_pool().await;
        let run = Uuid::new_v4();
        let session = Uuid::new_v4();
        sqlx::query("INSERT INTO runs(id,state,lease_expires_at) VALUES($1,'running',now()+interval '60 seconds')")
            .bind(run).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO desktop_sessions(id,run_id,state,runner_metadata) VALUES($1,$2,'starting','{\"start_pending\":true}')")
            .bind(session).bind(run).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let rows = sqlx::query(
            "SELECT id,state,runner_metadata FROM desktop_sessions WHERE id=$1 FOR UPDATE",
        )
        .bind(session)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        let prepared = prepare_session_stops(&mut tx, rows).await.unwrap();
        tx.commit().await.unwrap();
        let outcome = finish_prepared_stops_with(&pool, run, prepared, |_| async {
            panic!("a stop during an unknown start can return 404 before the container starts");
            #[allow(unreachable_code)]
            Ok(())
        })
        .await;
        assert!(outcome.is_err());
        assert!(
            !sqlx::query_scalar::<_, bool>(crate::db::SERVER_DESKTOPS_STOPPED_SQL)
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap()
        );
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_confirmed_retry_ends_the_captured_session_and_restores_safe_creation() {
        let pool = isolated_desktop_pool().await;
        let run = Uuid::new_v4();
        let session = Uuid::new_v4();
        sqlx::query("INSERT INTO runs(id,state,lease_expires_at) VALUES($1,'running',now()+interval '60 seconds')")
            .bind(run).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO desktop_sessions(id,run_id,state,runner_metadata) VALUES($1,$2,'agent_controlled','{\"start_pending\":false}')")
            .bind(session).bind(run).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let rows = sqlx::query(
            "SELECT id,state,runner_metadata FROM desktop_sessions WHERE id=$1 FOR UPDATE",
        )
        .bind(session)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        let prepared = prepare_session_stops(&mut tx, rows).await.unwrap();
        tx.commit().await.unwrap();
        assert!(finish_prepared_stops_with(&pool, run, prepared, |_| async {
            Err(anyhow::anyhow!("stop response lost"))
        })
        .await
        .is_err());
        assert!(
            !sqlx::query_scalar::<_, bool>(crate::db::SERVER_DESKTOPS_STOPPED_SQL)
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap()
        );
        let mut retry = pool.begin().await.unwrap();
        let rows = sqlx::query(
            "SELECT id,state,runner_metadata FROM desktop_sessions WHERE id=$1 FOR UPDATE",
        )
        .bind(session)
        .fetch_all(&mut *retry)
        .await
        .unwrap();
        let prepared = prepare_session_stops(&mut retry, rows).await.unwrap();
        retry.commit().await.unwrap();
        finish_prepared_stops_with(&pool, run, prepared, |id| async move {
            assert_eq!(id, session);
            Ok(())
        })
        .await
        .unwrap();
        assert!(
            sqlx::query_scalar::<_, bool>(crate::db::SERVER_DESKTOPS_STOPPED_SQL)
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap()
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT state FROM desktop_sessions WHERE id=$1")
                .bind(session)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "ended"
        );
        let mut creation = pool.begin().await.unwrap();
        reserve_desktop_creation(&mut creation, run).await.unwrap();
        creation.rollback().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM run_events WHERE run_id=$1")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_late_start_is_fenced_from_successor_lease() {
        let pool = isolated_desktop_pool().await;
        for replaced in [false, true] {
            let run = Uuid::new_v4();
            let session = Uuid::new_v4();
            sqlx::query("INSERT INTO runs(id,state,lease_expires_at) VALUES($1,'running',now()+interval '60 seconds')")
                .bind(run).execute(&pool).await.unwrap();
            let mut creation = pool.begin().await.unwrap();
            let token = reserve_desktop_creation(&mut creation, run).await.unwrap();
            sqlx::query("INSERT INTO desktop_sessions(id,run_id,state,runner_metadata) VALUES($1,$2,'starting',$3)")
                .bind(session).bind(run).bind(serde_json::json!({"start_pending":true,"start_lease_token":token}))
                .execute(&mut *creation).await.unwrap();
            creation.commit().await.unwrap();
            if replaced {
                sqlx::query("UPDATE runs SET lease_token=$2 WHERE id=$1")
                    .bind(run)
                    .bind(Uuid::new_v4())
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let result = finish_desktop_start(
                &pool,
                run,
                session,
                serde_json::json!({"container_name":"generation-fixture"}),
                true,
            )
            .await;
            assert_eq!(result.is_err(), replaced);
            let (state, metadata) = sqlx::query_as::<_, (String, serde_json::Value)>(
                "SELECT state,runner_metadata FROM desktop_sessions WHERE id=$1",
            )
            .bind(session)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                state,
                if replaced {
                    "starting"
                } else {
                    "agent_controlled"
                }
            );
            assert_eq!(metadata["start_pending"], false);
            if replaced {
                assert_eq!(metadata["cleanup_state"], "stop_pending");
                let mut creation = pool.begin().await.unwrap();
                assert!(reserve_desktop_creation(&mut creation, run).await.is_err());
                creation.rollback().await.unwrap();
            }
        }
        pool.close().await;
    }

    fn registration(os: &str, names: &[&str]) -> ExecutorRegistration {
        ExecutorRegistration {
            schema_version: SCHEMA_VERSION,
            executor_id: Uuid::new_v4(),
            kind: ExecutorKind::PersonalDevice,
            pool_id: None,
            owner_user_id: None,
            display_name: "test device".to_owned(),
            protocol_version: SCHEMA_VERSION,
            capabilities: names
                .iter()
                .map(|name| CapabilityDescriptor {
                    schema_version: SCHEMA_VERSION,
                    name: Capability::from(*name),
                    version: "test".to_owned(),
                    attributes: BTreeMap::new(),
                })
                .collect(),
            labels: BTreeMap::from([("os".to_owned(), os.to_owned())]),
            personal_device_remote_control: Some(
                PersonalDeviceRemoteControlMode::ConfirmEachSession,
            ),
            max_concurrent_runs: 1,
        }
    }

    #[test]
    fn external_desktop_platform_accepts_matching_personal_linux_and_windows_devices() {
        assert_eq!(
            external_desktop_platform(&registration("linux", &["desktop.linux"]), false),
            Some("linux")
        );
        assert_eq!(
            external_desktop_platform(&registration("windows", &["desktop.windows"]), false),
            Some("windows")
        );
        assert_eq!(
            external_desktop_platform(&registration("linux", &["desktop.windows"]), false),
            None
        );
        assert_eq!(
            external_desktop_platform(&registration("macos", &["desktop.windows"]), false),
            None
        );
    }

    #[test]
    fn managed_pool_never_accepts_linux_desktop_capability() {
        assert_eq!(
            external_desktop_platform(&registration("linux", &["desktop.linux"]), true),
            None
        );
        assert_eq!(
            external_desktop_platform(&registration("windows", &["desktop.windows"]), true),
            Some("windows")
        );
        assert_eq!(
            external_desktop_platform(&registration("linux", &["desktop.windows"]), true),
            None
        );
    }
}
