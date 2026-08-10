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
}

impl RunnerControl {
    pub fn new(url: String, signing_key: String) -> Self {
        Self {
            http: Client::new(),
            url,
            signing_key: Arc::new(signing_key.into_bytes()),
        }
    }

    async fn start(&self, spec: &SandboxDesktopSessionSpec) -> Result<SandboxDesktopSessionResult> {
        let body = serde_json::to_vec(spec)?;
        let path = "/v1/desktop-sessions";
        let (timestamp, nonce, signature) = self.signature("POST", path, &body)?;
        let response = self
            .http
            .post(format!("{}{path}", self.url.trim_end_matches('/')))
            .header("x-cowork-timestamp", timestamp)
            .header("x-cowork-nonce", nonce)
            .header("x-cowork-signature", signature)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!(
                "runner returned {status}: {}",
                body.chars().take(2_000).collect::<String>()
            );
        }
        Ok(serde_json::from_str(&body)?)
    }

    async fn stop(&self, session_id: Uuid) -> Result<()> {
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

    fn stream_request(&self, session_id: Uuid, control: bool) -> Result<axum::http::Request<()>> {
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
        columns: u16,
        rows: u16,
    ) -> Result<axum::http::Request<()>> {
        let mut url = reqwest::Url::parse(&self.url)?;
        url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
            .map_err(|_| anyhow!("invalid runner URL scheme"))?;
        url.set_path(&format!("/v1/runs/{run_id}/terminal"));
        let query = format!("columns={columns}&rows={rows}");
        url.set_query(Some(&query));
        let path = format!("/v1/runs/{run_id}/terminal?{query}");
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
    sqlx::query(
        "INSERT INTO desktop_sessions (id, run_id, executor_id, state, dimensions, created_at) VALUES ($1, $2, $3, 'starting', $4, $5)",
    )
    .bind(session_id)
    .bind(run_id)
    .bind(executor_id)
    .bind(serde_json::to_value(&dimensions)?)
    .bind(created_at)
    .execute(&state.pool)
    .await?;
    if managed_windows || personal_device {
        let mut tx = state.pool.begin().await?;
        sqlx::query("UPDATE desktop_sessions SET state = 'agent_controlled', runner_metadata = $2 WHERE id = $1")
            .bind(session_id)
            .bind(json!({
                "transport": "executor_reverse_ws",
                "personal_device": personal_device,
                "platform": external_platform.as_deref(),
                "remote_control_mode": personal_mode,
            }))
            .execute(&mut *tx)
            .await?;
        db::append_event_tx(
            &mut tx,
            run_id,
            RunEventKind::DesktopSessionChanged,
            json!({
                "session_id": session_id,
                "state": "agent_controlled",
                "platform": external_platform.as_deref(),
                "personal_device": personal_device,
                "local_confirmation_required": personal_mode == Some(PersonalDeviceRemoteControlMode::ConfirmEachSession),
            }),
        )
        .await?;
        tx.commit().await?;
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
        dimensions: dimensions.clone(),
        network: SandboxNetwork::FilteredEgress,
        limits: SandboxLimits {
            memory_bytes: 4 * 1024 * 1024 * 1024,
            pids: 1024,
            ..SandboxLimits::default()
        },
    };
    match runner.start(&spec).await {
        Ok(metadata) => {
            let mut tx = state.pool.begin().await?;
            sqlx::query("UPDATE desktop_sessions SET state = 'agent_controlled', runner_metadata = $2 WHERE id = $1")
                .bind(session_id)
                .bind(serde_json::to_value(metadata)?)
                .execute(&mut *tx)
                .await?;
            db::append_event_tx(
                &mut tx,
                run_id,
                RunEventKind::DesktopSessionChanged,
                json!({"session_id": session_id, "state": "agent_controlled"}),
            )
            .await?;
            tx.commit().await?;
        }
        Err(error) => {
            sqlx::query("UPDATE desktop_sessions SET state = 'failed', ended_at = now(), runner_metadata = $2 WHERE id = $1")
                .bind(session_id)
                .bind(json!({"error": error.to_string()}))
                .execute(&state.pool)
                .await?;
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
    let metadata: Option<Value> = session.try_get("runner_metadata")?;
    let external = metadata
        .as_ref()
        .and_then(|value| value.get("transport"))
        .and_then(Value::as_str)
        == Some("executor_reverse_ws");
    if !external {
        let runner = state.runner.as_ref().ok_or_else(|| {
            ApiError::Conflict("the Linux GUI runner is not configured".to_owned())
        })?;
        runner.stop(session_id).await.map_err(ApiError::Internal)?;
    }
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query("UPDATE desktop_sessions SET state = 'ended', controller_user_id = NULL, ended_at = now() WHERE id = $1 AND run_id = $2 AND state NOT IN ('ended', 'failed')")
        .bind(session_id).bind(run_id).execute(&mut *tx).await?.rows_affected();
    if changed == 0 {
        return Err(ApiError::Conflict(
            "desktop session state changed while it was being stopped".to_owned(),
        ));
    }
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::DesktopSessionChanged,
        json!({"session_id": session_id, "state": "ended", "actor_user_id": principal.user_id}),
    )
    .await?;
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
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM desktop_sessions WHERE id = $1 AND run_id = $2 AND state IN ('agent_controlled','paused'))")
            .bind(session_id).bind(run_id).fetch_one(&state.pool).await?
    } else {
        // Observers remain view-only at the runner's dedicated RFB endpoint and
        // may continue watching while another authorized user is in control.
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM desktop_sessions WHERE id = $1 AND run_id = $2 AND state IN ('agent_controlled','user_controlled','paused'))")
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
    sqlx::query("INSERT INTO desktop_stream_tickets (token_hash, desktop_session_id, user_id, control, expires_at) VALUES ($1, $2, $3, $4, $5)")
        .bind(digest.as_slice()).bind(session_id).bind(principal.user_id).bind(request.control).bind(expires_at).execute(&mut *tx).await?;
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
    let row = sqlx::query("UPDATE desktop_stream_tickets SET used_at = now() WHERE token_hash = $1 AND desktop_session_id = $2 AND used_at IS NULL AND expires_at > now() RETURNING user_id, control")
        .bind(digest.as_slice()).bind(session_id).fetch_optional(&state.pool).await?
        .ok_or_else(|| ApiError::Unauthorized("desktop stream ticket is invalid or expired".to_owned()))?;
    let user_id: Uuid = row.get("user_id");
    let control: bool = row.get("control");
    let session = sqlx::query(
        "SELECT run_id, executor_id, runner_metadata FROM desktop_sessions WHERE id = $1 AND state IN ('agent_controlled','user_controlled','paused')",
    )
    .bind(session_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::Conflict("the desktop session is no longer active".to_owned()))?;
    let run_id: Uuid = session.try_get("run_id")?;
    let executor_id: Uuid = session.try_get("executor_id")?;
    let metadata: Option<Value> = session.try_get("runner_metadata")?;
    let external = metadata
        .as_ref()
        .and_then(|value| value.get("transport"))
        .and_then(Value::as_str)
        == Some("executor_reverse_ws");
    let external_stream = if external {
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
        let run_id = sqlx::query_scalar::<_, Uuid>("UPDATE desktop_sessions SET state = 'user_controlled', controller_user_id = $2 WHERE id = $1 AND state IN ('agent_controlled','paused') RETURNING run_id")
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
    if let Some((stream_id, receiver)) = external_stream {
        let hub = state.executor_hub.clone();
        Ok(upgrade.on_upgrade(move |socket| {
            relay_external(
                socket, receiver, hub, stream_id, pool, session_id, user_id, control,
            )
        }))
    } else {
        let runner = state.runner.clone().ok_or_else(|| {
            ApiError::Conflict("the Linux GUI runner is not configured".to_owned())
        })?;
        Ok(upgrade
            .on_upgrade(move |socket| relay(socket, runner, pool, session_id, user_id, control)))
    }
}

async fn relay(
    socket: WebSocket,
    runner: Arc<RunnerControl>,
    pool: PgPool,
    session_id: Uuid,
    user_id: Uuid,
    control: bool,
) {
    let started = Utc::now();
    let outcome = relay_inner(socket, &runner, session_id, control).await;
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
    user_id: Uuid,
    control: bool,
) {
    let started = Utc::now();
    // A personal device may be waiting for its owner to answer a native local
    // confirmation dialog. Keep the reverse channel bounded without turning a
    // normal human response delay into an immediate stream failure.
    let outcome = match tokio::time::timeout(std::time::Duration::from_secs(300), receiver).await {
        Ok(Ok(executor_socket)) => relay_external_inner(socket, executor_socket, control).await,
        Ok(Err(_)) => Err(anyhow!("executor desktop stream request was canceled")),
        Err(_) => {
            hub.cancel_desktop_stream(stream_id).await;
            Err(anyhow!("executor did not open its reverse desktop stream"))
        }
    };
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
    session_id: Uuid,
    control: bool,
) -> Result<(u64, u64, Option<String>)> {
    let request = runner.stream_request(session_id, control)?;
    let (runner_socket, _) = tokio_tungstenite::connect_async(request).await?;
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

pub async fn ensure_worker_session(
    pool: &PgPool,
    runner: &RunnerControl,
    run_id: Uuid,
    executor_id: Uuid,
) -> Result<Uuid> {
    if let Some(id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM desktop_sessions WHERE run_id = $1 AND state IN ('starting','agent_controlled','user_controlled','paused')",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    {
        return Ok(id);
    }
    let session_id = Uuid::new_v4();
    let dimensions = DesktopDimensions {
        width: 1440,
        height: 900,
        scale_factor: 1.0,
    };
    sqlx::query("INSERT INTO desktop_sessions (id, run_id, executor_id, state, dimensions) VALUES ($1, $2, $3, 'starting', $4)")
        .bind(session_id).bind(run_id).bind(executor_id).bind(serde_json::to_value(&dimensions)?).execute(pool).await?;
    let spec = SandboxDesktopSessionSpec {
        schema_version: SCHEMA_VERSION,
        session_id,
        run_id,
        dimensions,
        network: SandboxNetwork::FilteredEgress,
        limits: SandboxLimits {
            memory_bytes: 4 * 1024 * 1024 * 1024,
            pids: 1024,
            ..SandboxLimits::default()
        },
    };
    match runner.start(&spec).await {
        Ok(metadata) => {
            let mut tx = pool.begin().await?;
            sqlx::query("UPDATE desktop_sessions SET state = 'agent_controlled', runner_metadata = $2 WHERE id = $1")
                .bind(session_id).bind(serde_json::to_value(metadata)?).execute(&mut *tx).await?;
            db::append_event_tx(
                &mut tx,
                run_id,
                RunEventKind::DesktopSessionChanged,
                json!({"session_id": session_id, "state": "agent_controlled", "automatic": true}),
            )
            .await?;
            tx.commit().await?;
            Ok(session_id)
        }
        Err(error) => {
            sqlx::query("UPDATE desktop_sessions SET state = 'failed', ended_at = now(), runner_metadata = $2 WHERE id = $1")
                .bind(session_id).bind(json!({"error": error.to_string()})).execute(pool).await?;
            Err(error)
        }
    }
}

pub async fn end_worker_sessions(
    pool: &PgPool,
    runner: &RunnerControl,
    run_id: Uuid,
) -> Result<()> {
    end_sessions(pool, Some(runner), run_id).await
}

pub async fn end_external_sessions(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id, runner_metadata FROM desktop_sessions WHERE run_id = $1 AND state IN ('starting','agent_controlled','user_controlled','paused') AND runner_metadata->>'transport' = 'executor_reverse_ws'",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;
    finish_session_rows(pool, run_id, rows, None).await
}

pub async fn end_sessions(
    pool: &PgPool,
    runner: Option<&RunnerControl>,
    run_id: Uuid,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id, runner_metadata FROM desktop_sessions WHERE run_id = $1 AND state IN ('starting','agent_controlled','user_controlled','paused')",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;
    finish_session_rows(pool, run_id, rows, runner).await
}

pub async fn reap_terminal_sessions(
    pool: &PgPool,
    runner: Option<&RunnerControl>,
) -> Result<usize> {
    let run_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT DISTINCT session.run_id
        FROM desktop_sessions session
        JOIN runs run ON run.id = session.run_id
        WHERE session.state IN ('starting','agent_controlled','user_controlled','paused')
          AND run.state IN ('interrupted','completed','failed','canceled','expired')
        LIMIT 100
        "#,
    )
    .fetch_all(pool)
    .await?;
    for run_id in &run_ids {
        end_sessions(pool, runner, *run_id).await?;
    }
    Ok(run_ids.len())
}

async fn finish_session_rows(
    pool: &PgPool,
    run_id: Uuid,
    rows: Vec<sqlx::postgres::PgRow>,
    runner: Option<&RunnerControl>,
) -> Result<()> {
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let metadata: Option<Value> = row.try_get("runner_metadata")?;
        let external = metadata
            .as_ref()
            .and_then(|value| value.get("transport"))
            .and_then(Value::as_str)
            == Some("executor_reverse_ws");
        if !external {
            let Some(runner) = runner else {
                tracing::warn!(session_id = %id, "cannot clean up a Linux desktop without a configured runner");
                continue;
            };
            if let Err(error) = runner.stop(id).await {
                tracing::warn!(?error, session_id = %id, "failed to stop runner desktop session");
            }
        }
        let mut tx = pool.begin().await?;
        sqlx::query("UPDATE desktop_sessions SET state = 'ended', controller_user_id = NULL, ended_at = now() WHERE id = $1 AND state NOT IN ('ended','failed')")
            .bind(id).execute(&mut *tx).await?;
        db::append_event_tx(
            &mut tx,
            run_id,
            RunEventKind::DesktopSessionChanged,
            json!({"session_id": id, "state": "ended", "automatic": true}),
        )
        .await?;
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

    use super::external_desktop_platform;

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
