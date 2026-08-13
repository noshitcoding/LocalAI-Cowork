use std::sync::Arc;

use anyhow::{anyhow, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Extension, Path, Query, State,
    },
    http::StatusCode,
    response::Response,
    Json,
};
use chrono::Utc;
use cowork_contracts::{
    CreateTerminalSessionRequest, ExecutorTarget, ProjectRole, RunState, TerminalSessionTicket,
    SCHEMA_VERSION,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use tokio_tungstenite::tungstenite::Message as RunnerMessage;
use uuid::Uuid;

use crate::{
    auth::{self, Principal},
    db,
    desktop::RunnerControl,
    error::ApiError,
    organization, AppState,
};

const MAX_TERMINALS_PER_RUN: i64 = 4;
const MAX_TERMINAL_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_TERMINAL_INPUT_BYTES: u64 = 32 * 1024 * 1024;

pub async fn create_session(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateTerminalSessionRequest>,
) -> Result<(StatusCode, Json<TerminalSessionTicket>), ApiError> {
    if !(20..=400).contains(&request.columns) || !(5..=200).contains(&request.rows) {
        return Err(ApiError::BadRequest(
            "terminal dimensions are outside the supported range".to_owned(),
        ));
    }
    let run = db::get_run(&state.pool, run_id).await?;
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        run.spec.project_id,
        ProjectRole::Runner,
    )
    .await?;
    if !matches!(
        run.state,
        RunState::Running | RunState::WaitingApproval | RunState::WaitingInput
    ) {
        return Err(ApiError::Conflict(
            "a terminal can only be opened for an active run".to_owned(),
        ));
    }
    if !matches!(run.spec.executor_target, ExecutorTarget::ServerLinux { .. }) {
        return Err(ApiError::Unprocessable(
            "interactive terminals currently require a Linux server target".to_owned(),
        ));
    }
    if state.runner.is_none() {
        return Err(ApiError::Conflict(
            "the Linux sandbox runner is not configured".to_owned(),
        ));
    }

    let session_id = Uuid::new_v4();
    let token = auth::random_token()?;
    let token_hash = auth::opaque_token_hash(&token);
    let expires_at = Utc::now() + chrono::Duration::minutes(1);
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "UPDATE terminal_sessions SET state = 'ended', ended_at = now(), failure = 'stream ticket expired before connection' WHERE run_id = $1 AND state = 'created' AND created_at < now() - interval '1 minute'",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    let active = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM terminal_sessions WHERE run_id = $1 AND state IN ('created','connected')",
    )
    .bind(run_id)
    .fetch_one(&mut *tx)
    .await?;
    if active >= MAX_TERMINALS_PER_RUN {
        return Err(ApiError::Conflict(format!(
            "a run may have at most {MAX_TERMINALS_PER_RUN} active terminals"
        )));
    }
    sqlx::query(
        "INSERT INTO terminal_sessions (id, run_id, user_id, state, columns, rows) VALUES ($1, $2, $3, 'created', $4, $5)",
    )
    .bind(session_id)
    .bind(run_id)
    .bind(principal.user_id)
    .bind(i32::from(request.columns))
    .bind(i32::from(request.rows))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO terminal_stream_tickets (token_hash, terminal_session_id, user_id, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(token_hash.as_slice())
    .bind(session_id)
    .bind(principal.user_id)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    audit(
        &mut tx,
        principal.user_id,
        "terminal_session.create",
        session_id,
        json!({"run_id": run_id, "columns": request.columns, "rows": request.rows}),
    )
    .await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(TerminalSessionTicket {
            schema_version: SCHEMA_VERSION,
            session_id,
            token,
            expires_at,
            protocol: "terminal.binary.v1".to_owned(),
        }),
    ))
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
    let token_hash = auth::opaque_token_hash(&query.ticket);
    let mut tx = state.pool.begin().await?;
    let ticket = sqlx::query(
        "UPDATE terminal_stream_tickets SET used_at = now() WHERE token_hash = $1 AND terminal_session_id = $2 AND used_at IS NULL AND expires_at > now() RETURNING user_id",
    )
    .bind(token_hash.as_slice())
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError::Unauthorized("terminal stream ticket is invalid or expired".to_owned())
    })?;
    let user_id: Uuid = ticket.get("user_id");
    let session = sqlx::query(
        "UPDATE terminal_sessions SET state = 'connected', connected_at = now() WHERE id = $1 AND user_id = $2 AND state = 'created' RETURNING run_id, columns, rows",
    )
    .bind(session_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::Conflict("terminal session is no longer available".to_owned()))?;
    let run_id: Uuid = session.get("run_id");
    let columns = u16::try_from(session.get::<i32, _>("columns"))
        .map_err(|_| ApiError::Internal(anyhow!("invalid terminal column count")))?;
    let rows = u16::try_from(session.get::<i32, _>("rows"))
        .map_err(|_| ApiError::Internal(anyhow!("invalid terminal row count")))?;
    audit(
        &mut tx,
        user_id,
        "terminal_session.connect",
        session_id,
        json!({"run_id": run_id}),
    )
    .await?;
    tx.commit().await?;

    let runner = state.runner.clone().ok_or_else(|| {
        ApiError::Conflict("the Linux sandbox runner is not configured".to_owned())
    })?;
    let pool = state.pool.clone();
    Ok(upgrade.on_upgrade(move |socket| {
        relay(
            socket, runner, pool, session_id, run_id, user_id, columns, rows,
        )
    }))
}

#[allow(clippy::too_many_arguments)]
async fn relay(
    socket: WebSocket,
    runner: Arc<RunnerControl>,
    pool: PgPool,
    session_id: Uuid,
    run_id: Uuid,
    user_id: Uuid,
    columns: u16,
    rows: u16,
) {
    let outcome = relay_inner(socket, &runner, run_id, columns, rows).await;
    let (state, input_bytes, output_bytes, input_digest, failure) = match outcome {
        Ok(summary) => (
            "ended",
            summary.input_bytes,
            summary.output_bytes,
            Some(summary.input_digest),
            None,
        ),
        Err(error) => {
            tracing::warn!(?error, %session_id, %run_id, "terminal relay failed");
            (
                "failed",
                0,
                0,
                None,
                Some(error.to_string().chars().take(500).collect::<String>()),
            )
        }
    };
    if let Ok(mut tx) = pool.begin().await {
        let changed = sqlx::query(
            "UPDATE terminal_sessions SET state = $2, input_bytes = $3, output_bytes = $4, ended_at = now(), failure = $5 WHERE id = $1 AND state = 'connected'",
        )
        .bind(session_id)
        .bind(state)
        .bind(i64::try_from(input_bytes).unwrap_or(i64::MAX))
        .bind(i64::try_from(output_bytes).unwrap_or(i64::MAX))
        .bind(&failure)
        .execute(&mut *tx)
        .await;
        if changed
            .as_ref()
            .is_ok_and(|result| result.rows_affected() == 1)
        {
            let _ = audit(
                &mut tx,
                user_id,
                "terminal_session.end",
                session_id,
                json!({
                    "run_id": run_id,
                    "state": state,
                    "input_bytes": input_bytes,
                    "output_bytes": output_bytes,
                    "input_digest": input_digest,
                    "failure": failure,
                }),
            )
            .await;
            let _ = tx.commit().await;
        } else {
            let _ = tx.rollback().await;
        }
    }
}

struct RelaySummary {
    input_bytes: u64,
    output_bytes: u64,
    input_digest: String,
}

async fn relay_inner(
    client: WebSocket,
    runner: &RunnerControl,
    run_id: Uuid,
    columns: u16,
    rows: u16,
) -> Result<RelaySummary> {
    let request = runner.terminal_stream_request(run_id, columns, rows)?;
    let (runner_socket, _) = tokio_tungstenite::connect_async(request).await?;
    let (mut client_send, mut client_recv) = client.split();
    let (mut runner_send, mut runner_recv) = runner_socket.split();
    let mut input_bytes = 0_u64;
    let mut output_bytes = 0_u64;
    let mut input_digest = Sha256::new();
    loop {
        tokio::select! {
            incoming = client_recv.next() => match incoming {
                Some(Ok(Message::Binary(data))) => {
                    if data.len() > MAX_TERMINAL_MESSAGE_BYTES {
                        return Err(anyhow!("terminal input message exceeded the size limit"));
                    }
                    input_bytes = input_bytes.saturating_add(data.len() as u64);
                    if input_bytes > MAX_TERMINAL_INPUT_BYTES {
                        return Err(anyhow!("terminal input exceeded the session limit"));
                    }
                    input_digest.update(&data);
                    runner_send.send(RunnerMessage::Binary(data.to_vec().into())).await?;
                }
                Some(Ok(Message::Text(data))) => {
                    let bytes = data.as_bytes();
                    if bytes.len() > MAX_TERMINAL_MESSAGE_BYTES {
                        return Err(anyhow!("terminal input message exceeded the size limit"));
                    }
                    input_bytes = input_bytes.saturating_add(bytes.len() as u64);
                    if input_bytes > MAX_TERMINAL_INPUT_BYTES {
                        return Err(anyhow!("terminal input exceeded the session limit"));
                    }
                    input_digest.update(bytes);
                    runner_send.send(RunnerMessage::Binary(bytes.to_vec().into())).await?;
                }
                Some(Ok(Message::Ping(data))) => client_send.send(Message::Pong(data)).await?,
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(error)) => return Err(error.into()),
                _ => {}
            },
            incoming = runner_recv.next() => match incoming {
                Some(Ok(RunnerMessage::Binary(data))) => {
                    output_bytes = output_bytes.saturating_add(data.len() as u64);
                    client_send.send(Message::Binary(data.to_vec().into())).await?;
                }
                Some(Ok(RunnerMessage::Text(data))) => {
                    output_bytes = output_bytes.saturating_add(data.len() as u64);
                    client_send.send(Message::Text(data.to_string().into())).await?;
                }
                Some(Ok(RunnerMessage::Ping(data))) => runner_send.send(RunnerMessage::Pong(data)).await?,
                Some(Ok(RunnerMessage::Close(_))) | None => break,
                Some(Err(error)) => return Err(error.into()),
                _ => {}
            }
        }
    }
    Ok(RelaySummary {
        input_bytes,
        output_bytes,
        input_digest: hex::encode(input_digest.finalize()),
    })
}

async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    action: &str,
    target_id: Uuid,
    metadata: serde_json::Value,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, $3, 'terminal_session', $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(action)
    .bind(target_id)
    .bind(metadata)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_and_limits_are_deliberately_bounded() {
        assert!((20..=400).contains(&20));
        assert!((5..=200).contains(&200));
        assert_eq!(MAX_TERMINAL_MESSAGE_BYTES, 65_536);
        assert_eq!(MAX_TERMINAL_INPUT_BYTES, 33_554_432);
    }
}
