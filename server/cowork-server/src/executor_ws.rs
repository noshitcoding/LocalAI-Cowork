use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Extension, Path, State, WebSocketUpgrade,
    },
    response::Response,
};
use cowork_contracts::{
    ensure_compatible, ExecutorClientMessage, ExecutorServerMessage, RunLease, SCHEMA_VERSION,
};
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;

use crate::{auth::ExecutorPrincipal, db, desktop, error::ApiError, AppState};

#[derive(Clone, Default)]
pub struct ExecutorHub {
    connections: Arc<Mutex<HashMap<Uuid, ExecutorConnection>>>,
    pending_streams: Arc<Mutex<HashMap<Uuid, PendingDesktopStream>>>,
}

#[derive(Clone)]
struct ExecutorConnection {
    connection_id: Uuid,
    commands: mpsc::Sender<ExecutorServerMessage>,
}

struct PendingDesktopStream {
    executor_id: Uuid,
    session_id: Uuid,
    socket: oneshot::Sender<WebSocket>,
}

impl ExecutorHub {
    async fn register(&self, executor_id: Uuid) -> (Uuid, mpsc::Receiver<ExecutorServerMessage>) {
        let connection_id = Uuid::new_v4();
        let (commands, receiver) = mpsc::channel(16);
        self.connections.lock().await.insert(
            executor_id,
            ExecutorConnection {
                connection_id,
                commands,
            },
        );
        (connection_id, receiver)
    }

    async fn unregister(&self, executor_id: Uuid, connection_id: Uuid) {
        let mut connections = self.connections.lock().await;
        if connections
            .get(&executor_id)
            .is_some_and(|connection| connection.connection_id == connection_id)
        {
            connections.remove(&executor_id);
        }
    }

    pub async fn request_desktop_stream(
        &self,
        executor_id: Uuid,
        run_id: Uuid,
        session_id: Uuid,
        control: bool,
    ) -> Result<(Uuid, oneshot::Receiver<WebSocket>), ApiError> {
        let connection = self
            .connections
            .lock()
            .await
            .get(&executor_id)
            .cloned()
            .ok_or_else(|| ApiError::Conflict("the Windows executor is offline".to_owned()))?;
        let stream_id = Uuid::new_v4();
        let (sender, receiver) = oneshot::channel();
        self.pending_streams.lock().await.insert(
            stream_id,
            PendingDesktopStream {
                executor_id,
                session_id,
                socket: sender,
            },
        );
        if connection
            .commands
            .send(ExecutorServerMessage::DesktopStreamRequested {
                run_id,
                session_id,
                stream_id,
                control,
            })
            .await
            .is_err()
        {
            self.pending_streams.lock().await.remove(&stream_id);
            return Err(ApiError::Conflict(
                "the Windows executor disconnected".to_owned(),
            ));
        }
        Ok((stream_id, receiver))
    }

    pub async fn cancel_desktop_stream(&self, stream_id: Uuid) {
        self.pending_streams.lock().await.remove(&stream_id);
    }

    async fn fulfill_desktop_stream(&self, executor_id: Uuid, stream_id: Uuid, socket: WebSocket) {
        let pending = {
            let mut streams = self.pending_streams.lock().await;
            if streams
                .get(&stream_id)
                .is_some_and(|pending| pending.executor_id == executor_id)
            {
                streams.remove(&stream_id)
            } else {
                None
            }
        };
        match pending {
            Some(pending) => {
                tracing::debug!(%executor_id, %stream_id, session_id = %pending.session_id, "paired executor desktop stream");
                let _ = pending.socket.send(socket);
            }
            None => {
                tracing::warn!(%executor_id, %stream_id, "rejected unexpected executor desktop stream");
            }
        }
    }
}

pub async fn connect(
    State(state): State<AppState>,
    Path(executor_id): Path<Uuid>,
    Extension(principal): Extension<ExecutorPrincipal>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    if principal.executor_id != executor_id {
        return Err(ApiError::Unauthorized(
            "executor credential does not match the requested executor".to_owned(),
        ));
    }
    Ok(upgrade.on_upgrade(move |socket| run_socket(socket, state, principal)))
}

pub async fn connect_desktop_stream(
    State(state): State<AppState>,
    Path((executor_id, stream_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    if principal.executor_id != executor_id {
        return Err(ApiError::Unauthorized(
            "executor credential does not match the requested executor".to_owned(),
        ));
    }
    Ok(upgrade.on_upgrade(move |socket| async move {
        state
            .executor_hub
            .fulfill_desktop_stream(executor_id, stream_id, socket)
            .await;
    }))
}

async fn run_socket(mut socket: WebSocket, state: AppState, principal: ExecutorPrincipal) {
    tracing::info!(
        executor_id = %principal.executor_id,
        credential_id = %principal.credential_id,
        "executor WebSocket connected"
    );
    if send(
        &mut socket,
        &ExecutorServerMessage::Hello {
            schema_version: SCHEMA_VERSION,
            executor_id: principal.executor_id,
            heartbeat_interval_seconds: 20,
        },
    )
    .await
    .is_err()
    {
        return;
    }

    let (connection_id, mut commands) = state.executor_hub.register(principal.executor_id).await;
    let mut current_lease: Option<RunLease> = None;
    let mut dispatch = tokio::time::interval(Duration::from_secs(2));
    dispatch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                if send(&mut socket, &command).await.is_err() { break; }
            }
            _ = dispatch.tick() => {
                if current_lease.is_none() {
                    match db::recover_external_run(&state.pool, principal.executor_id, state.lease_seconds).await {
                        Ok(Some(lease)) => {
                            if send(&mut socket, &ExecutorServerMessage::Lease { lease: Box::new(lease.clone()) }).await.is_err() {
                                break;
                            }
                            current_lease = Some(lease);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            if send_error(&mut socket, "lease_recovery_failed", &error.to_string(), None).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                let active = usize::from(current_lease.is_some());
                if let Err(error) = db::heartbeat_executor(&state.pool, principal.executor_id, active).await {
                    if send_error(&mut socket, "heartbeat_failed", &error.to_string(), current_lease.as_ref().map(|lease| lease.run.spec.id)).await.is_err() {
                        break;
                    }
                    continue;
                }
                if current_lease.is_none() {
                    match db::claim_external_run(&state.pool, principal.executor_id, state.lease_seconds).await {
                        Ok(Some(lease)) => {
                            if send(&mut socket, &ExecutorServerMessage::Lease { lease: Box::new(lease.clone()) }).await.is_err() {
                                break;
                            }
                            current_lease = Some(lease);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            if send_error(&mut socket, "claim_failed", &error.to_string(), None).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
            incoming = socket.next() => {
                let Some(incoming) = incoming else { break };
                match incoming {
                    Ok(Message::Text(text)) => {
                        match serde_json::from_str::<ExecutorClientMessage>(&text) {
                            Ok(message) => {
                                let finished = handle_message(&state, principal.executor_id, &mut socket, message).await;
                                match finished {
                                    Ok(Some(run_id)) => {
                                        if current_lease.as_ref().is_some_and(|lease| lease.run.spec.id == run_id) {
                                            current_lease = None;
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(error) => {
                                        if send_error(&mut socket, "operation_failed", &error.to_string(), None).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                if send_error(&mut socket, "invalid_message", &error.to_string(), None).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Ok(Message::Ping(payload)) => {
                        if socket.send(Message::Pong(payload)).await.is_err() { break; }
                    }
                    Ok(Message::Pong(_)) => {}
                    Ok(Message::Close(_)) => break,
                    Ok(Message::Binary(_)) => {
                        if send_error(&mut socket, "invalid_message", "binary messages are reserved for GUI streams", None).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(?error, executor_id = %principal.executor_id, "executor WebSocket failed");
                        break;
                    }
                }
            }
        }
    }
    state
        .executor_hub
        .unregister(principal.executor_id, connection_id)
        .await;
    tracing::info!(executor_id = %principal.executor_id, "executor WebSocket disconnected");
}

async fn handle_message(
    state: &AppState,
    executor_id: Uuid,
    socket: &mut WebSocket,
    message: ExecutorClientMessage,
) -> Result<Option<Uuid>, ApiError> {
    let (operation, run_id, finished) = match message {
        ExecutorClientMessage::Heartbeat { heartbeat } => {
            ensure_compatible(heartbeat.protocol_version)
                .map_err(|error| ApiError::Unprocessable(error.to_string()))?;
            db::heartbeat_executor(&state.pool, executor_id, heartbeat.active_run_ids.len())
                .await?;
            ("heartbeat", None, false)
        }
        ExecutorClientMessage::LeaseHeartbeat {
            run_id,
            lease_token,
        } => {
            db::renew_lease(
                &state.pool,
                run_id,
                executor_id,
                lease_token,
                state.lease_seconds,
            )
            .await?;
            ("lease_heartbeat", Some(run_id), false)
        }
        ExecutorClientMessage::Event { run_id, request } => {
            db::append_leased_event(
                &state.pool,
                run_id,
                executor_id,
                request.lease_token,
                request.source_event_id,
                request.kind,
                request.payload,
            )
            .await?;
            ("event", Some(run_id), false)
        }
        ExecutorClientMessage::Complete { run_id, request } => {
            db::complete_leased_run(
                &state.pool,
                run_id,
                executor_id,
                request.lease_token,
                request.result,
                request.result_snapshot_manifest_id,
                request.result_diff_summary,
            )
            .await?;
            if let Err(error) = desktop::end_external_sessions(&state.pool, run_id).await {
                tracing::warn!(?error, %run_id, "failed to close completed executor desktop sessions");
            }
            ("complete", Some(run_id), true)
        }
        ExecutorClientMessage::Fail { run_id, request } => {
            db::fail_leased_run(
                &state.pool,
                run_id,
                executor_id,
                request.lease_token,
                request.error,
            )
            .await?;
            if let Err(error) = desktop::end_external_sessions(&state.pool, run_id).await {
                tracing::warn!(?error, %run_id, "failed to close failed executor desktop sessions");
            }
            ("fail", Some(run_id), true)
        }
    };
    send(
        socket,
        &ExecutorServerMessage::Ack {
            operation: operation.to_owned(),
            run_id,
        },
    )
    .await
    .map_err(|error| ApiError::Internal(error.into()))?;
    Ok(if finished { run_id } else { None })
}

async fn send(socket: &mut WebSocket, message: &ExecutorServerMessage) -> Result<(), axum::Error> {
    let encoded = serde_json::to_string(message).expect("executor message is serializable");
    socket.send(Message::Text(encoded.into())).await
}

async fn send_error(
    socket: &mut WebSocket,
    code: &str,
    message: &str,
    run_id: Option<Uuid>,
) -> Result<(), axum::Error> {
    send(
        socket,
        &ExecutorServerMessage::Error {
            code: code.to_owned(),
            message: message.to_owned(),
            run_id,
        },
    )
    .await
}
