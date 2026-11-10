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

use crate::{
    api_cluster::{
        self, directory::Owner, relay::DesktopSocket, tickets::Pending, ApiCluster, WeakCluster,
    },
    api_replica::ApiReplicaGuard,
    auth::ExecutorPrincipal,
    db, desktop,
    error::ApiError,
    AppState,
};

#[derive(Clone, Default)]
pub struct ExecutorHub {
    connections: Arc<Mutex<HashMap<Uuid, ExecutorConnection>>>,
    pending_streams: Arc<Mutex<HashMap<Uuid, PendingDesktopStream>>>,
    ownership: Option<ApiReplicaGuard>,
    cluster: Option<WeakCluster>,
    cluster_streams: Arc<Mutex<HashMap<Uuid, PendingClusterStream>>>,
}

#[derive(Clone)]
struct ExecutorConnection {
    connection_id: Uuid,
    credential_id: Uuid,
    commands: mpsc::Sender<ExecutorServerMessage>,
    owner: Option<Owner>,
    confirmed_until: tokio::time::Instant,
}

struct PendingClusterStream {
    pending: Pending,
    socket: oneshot::Sender<DesktopSocket>,
}

struct PendingDesktopStream {
    executor_id: Uuid,
    connection_id: Uuid,
    credential_id: Uuid,
    session_id: Uuid,
    expires_at: tokio::time::Instant,
    socket: oneshot::Sender<WebSocket>,
}

impl ExecutorHub {
    pub fn new_cluster(cluster: ApiCluster) -> Self {
        Self {
            cluster: Some(cluster.downgrade()),
            ..Self::default()
        }
    }

    fn cluster(&self) -> Result<Option<ApiCluster>, ApiError> {
        self.cluster
            .as_ref()
            .map(|weak| {
                weak.upgrade().ok_or_else(|| {
                    ApiError::Unavailable("API cluster ownership is no longer active".to_owned())
                })
            })
            .transpose()
    }
    pub fn new(ownership: Option<ApiReplicaGuard>) -> Self {
        Self {
            ownership,
            ..Self::default()
        }
    }

    async fn ensure_ownership(&self) -> Result<(), ApiError> {
        if let Some(cluster) = self.cluster()? {
            return cluster.ensure_active().await.map_err(|_| {
                ApiError::Unavailable("API cluster ownership is no longer active".to_owned())
            });
        }
        let ownership = self.ownership.as_ref().ok_or_else(|| {
            ApiError::Unavailable("executor streaming requires the active API process".to_owned())
        })?;
        ownership.ensure_active().await.map_err(|_| {
            ApiError::Unavailable(
                "API process lost exclusive executor/desktop connection ownership".to_owned(),
            )
        })
    }

    async fn register(
        &self,
        executor_id: Uuid,
        credential_id: Uuid,
    ) -> (Uuid, mpsc::Receiver<ExecutorServerMessage>) {
        let connection_id = Uuid::new_v4();
        let (commands, receiver) = mpsc::channel(16);
        let mut connections = self.connections.lock().await;
        let previous = connections.insert(
            executor_id,
            ExecutorConnection {
                connection_id,
                credential_id,
                commands,
                owner: None,
                confirmed_until: tokio::time::Instant::now() + Duration::from_secs(8),
            },
        );
        if let Some(previous) = previous {
            self.pending_streams.lock().await.retain(|_, pending| {
                pending.executor_id != executor_id
                    || pending.connection_id != previous.connection_id
            });
        }
        (connection_id, receiver)
    }

    async fn is_current(&self, executor_id: Uuid, connection_id: Uuid) -> bool {
        let connection = self.connections.lock().await.get(&executor_id).cloned();
        let Some(connection) = connection.filter(|c| c.connection_id == connection_id) else {
            return false;
        };
        let Some(owner) = &connection.owner else {
            return self.cluster.is_none();
        };
        let Ok(Some(cluster)) = self.cluster() else {
            return false;
        };
        connection.confirmed_until > tokio::time::Instant::now()
            && matches!(
                tokio::time::timeout(
                    Duration::from_secs(2),
                    api_cluster::directory::verify_owner(&cluster, owner)
                )
                .await,
                Ok(Ok(()))
            )
            && connection.confirmed_until > tokio::time::Instant::now()
    }

    async fn register_current(
        &self,
        executor: Uuid,
        credential: Uuid,
    ) -> Result<(Uuid, mpsc::Receiver<ExecutorServerMessage>), ApiError> {
        let Some(cluster) = self.cluster()? else {
            return Ok(self.register(executor, credential).await);
        };
        let connection_id = Uuid::new_v4();
        let (commands, receiver) = mpsc::channel(16);
        // Serialise local replacement with the DB claim so a delayed earlier
        // claim cannot overwrite a newer local socket after it commits.
        let mut connections = self.connections.lock().await;
        let started = tokio::time::Instant::now();
        let owner = tokio::time::timeout(
            Duration::from_secs(2),
            api_cluster::directory::claim_executor(&cluster, executor, credential, connection_id),
        )
        .await
        .map_err(|_| ApiError::Unavailable("executor connection ownership timed out".to_owned()))?
        .map_err(ApiError::Internal)?;
        if started + Duration::from_secs(api_cluster::LEASE_SECONDS) <= tokio::time::Instant::now()
        {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                api_cluster::directory::release_executor(&cluster, &owner),
            )
            .await;
            return Err(ApiError::Conflict(
                "executor connection claim answered after its deadline".to_owned(),
            ));
        }
        connections.insert(
            executor,
            ExecutorConnection {
                connection_id,
                credential_id: credential,
                commands,
                owner: Some(owner),
                confirmed_until: started + Duration::from_secs(8),
            },
        );
        self.pending_streams
            .lock()
            .await
            .retain(|_, p| p.executor_id != executor);
        self.cluster_streams
            .lock()
            .await
            .retain(|_, p| p.pending.owner.executor_id != Some(executor));
        Ok((connection_id, receiver))
    }

    async fn renew_current(&self, executor: Uuid, connection: Uuid) -> Result<(), ApiError> {
        let Some(cluster) = self.cluster()? else {
            return Ok(());
        };
        let started = tokio::time::Instant::now();
        let mut connections = self.connections.lock().await;
        let current = connections
            .get_mut(&executor)
            .filter(|c| c.connection_id == connection)
            .ok_or_else(|| ApiError::Conflict("executor connection was replaced".to_owned()))?;
        if current.confirmed_until <= started {
            return Err(ApiError::Conflict(
                "executor connection lease expired".to_owned(),
            ));
        }
        let owner = current
            .owner
            .as_ref()
            .ok_or_else(|| ApiError::Conflict("executor connection is unbound".to_owned()))?;
        tokio::time::timeout(
            Duration::from_secs(2),
            api_cluster::directory::renew_executor(&cluster, owner),
        )
        .await
        .map_err(|_| ApiError::Unavailable("executor connection renewal timed out".to_owned()))?
        .map_err(ApiError::Internal)?;
        if current.confirmed_until <= tokio::time::Instant::now() {
            return Err(ApiError::Conflict(
                "executor connection renewal answered after its deadline".to_owned(),
            ));
        }
        current.confirmed_until = started + Duration::from_secs(8);
        Ok(())
    }

    async fn unregister(&self, executor_id: Uuid, connection_id: Uuid) {
        let mut connections = self.connections.lock().await;
        if connections
            .get(&executor_id)
            .is_some_and(|connection| connection.connection_id == connection_id)
        {
            let removed = connections.remove(&executor_id);
            self.pending_streams.lock().await.retain(|_, pending| {
                pending.executor_id != executor_id || pending.connection_id != connection_id
            });
            self.cluster_streams.lock().await.retain(|_, p| {
                p.pending.owner.executor_id != Some(executor_id)
                    || p.pending.owner.connection_epoch != Some(connection_id)
            });
            drop(connections);
            if let (Some(owner), Ok(Some(cluster))) =
                (removed.and_then(|c| c.owner), self.cluster())
            {
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    api_cluster::directory::release_executor(&cluster, &owner),
                )
                .await;
            }
        }
    }

    pub(crate) async fn request_cluster_desktop_stream(
        &self,
        pending: Pending,
    ) -> anyhow::Result<oneshot::Receiver<DesktopSocket>> {
        self.ensure_ownership().await?;
        let executor = pending
            .owner
            .executor_id
            .ok_or_else(|| anyhow::anyhow!("private desktop executor missing"))?;
        let connection = self
            .connections
            .lock()
            .await
            .get(&executor)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("desktop executor is offline"))?;
        anyhow::ensure!(
            connection.owner.as_ref() == Some(&pending.owner)
                && self.is_current(executor, connection.connection_id).await,
            "desktop executor connection was replaced"
        );
        let stream = pending.stream_id;
        let (session, run, control) = (pending.session_id, pending.run_id, pending.control);
        let (sender, receiver) = oneshot::channel();
        let mut streams = self.cluster_streams.lock().await;
        anyhow::ensure!(
            !streams.contains_key(&stream),
            "private desktop stream already queued"
        );
        streams.insert(
            stream,
            PendingClusterStream {
                pending,
                socket: sender,
            },
        );
        drop(streams);
        if !matches!(
            tokio::time::timeout(
                Duration::from_secs(3),
                connection
                    .commands
                    .send(ExecutorServerMessage::DesktopStreamRequested {
                        run_id: run,
                        session_id: session,
                        stream_id: stream,
                        control
                    })
            )
            .await,
            Ok(Ok(()))
        ) {
            self.cancel_cluster_desktop_stream(stream).await;
            anyhow::bail!("desktop command queue unavailable; outcome must not be replayed");
        }
        Ok(receiver)
    }

    pub(crate) async fn cancel_cluster_desktop_stream(&self, stream: Uuid) {
        self.cluster_streams.lock().await.remove(&stream);
    }

    pub(crate) async fn fulfill_cluster_desktop_stream(
        &self,
        pending: &Pending,
        socket: DesktopSocket,
    ) -> anyhow::Result<()> {
        self.ensure_ownership().await?;
        let executor = pending
            .owner
            .executor_id
            .ok_or_else(|| anyhow::anyhow!("private desktop executor missing"))?;
        let connection = pending
            .owner
            .connection_epoch
            .ok_or_else(|| anyhow::anyhow!("private desktop connection missing"))?;
        anyhow::ensure!(
            self.is_current(executor, connection).await,
            "desktop connection owner changed before callback"
        );
        let mut streams = self.cluster_streams.lock().await;
        anyhow::ensure!(
            streams
                .get(&pending.stream_id)
                .is_some_and(|p| p.pending.owner == pending.owner),
            "private desktop callback has no local pending command"
        );
        let queued = streams
            .remove(&pending.stream_id)
            .ok_or_else(|| anyhow::anyhow!("private desktop callback already used"))?;
        queued
            .socket
            .send(socket)
            .map_err(|_| anyhow::anyhow!("desktop user bridge closed"))
    }

    async fn send_command(
        &self,
        socket: &mut WebSocket,
        command: &ExecutorServerMessage,
        executor: Uuid,
        connection: Uuid,
    ) -> anyhow::Result<()> {
        if let (
            Some(cluster),
            ExecutorServerMessage::DesktopStreamRequested {
                stream_id,
                run_id,
                session_id,
                control,
            },
        ) = (self.cluster()?, command)
        {
            let pending = self
                .cluster_streams
                .lock()
                .await
                .get(stream_id)
                .map(|p| p.pending.clone())
                .ok_or_else(|| anyhow::anyhow!("private desktop command was cancelled"))?;
            anyhow::ensure!(
                pending.owner.executor_id == Some(executor)
                    && pending.owner.connection_epoch == Some(connection)
                    && pending.run_id == *run_id
                    && pending.session_id == *session_id
                    && pending.control == *control,
                "private desktop command binding mismatch"
            );
            let deadline = self
                .connections
                .lock()
                .await
                .get(&executor)
                .filter(|c| {
                    c.connection_id == connection && c.owner.as_ref() == Some(&pending.owner)
                })
                .map(|c| c.confirmed_until)
                .ok_or_else(|| anyhow::anyhow!("private desktop connection was replaced"))?;
            anyhow::ensure!(
                deadline > tokio::time::Instant::now(),
                "private desktop connection deadline expired"
            );
            return tokio::select! {biased;
                _=tokio::time::sleep_until(deadline)=>Err(anyhow::anyhow!("private desktop connection deadline expired")),
                result=tokio::time::timeout(Duration::from_secs(6),api_cluster::tickets::dispatch(&cluster,&pending,socket,command))=>result?,
            };
        }
        self.fence_operation(executor, connection, async {
            send(socket, command)
                .await
                .map_err(|e| ApiError::Internal(e.into()))
        })
        .await?;
        Ok(())
    }

    async fn fence_operation<T>(
        &self,
        executor: Uuid,
        connection: Uuid,
        operation: impl std::future::Future<Output = Result<T, ApiError>>,
    ) -> Result<T, ApiError> {
        let Some(cluster) = self.cluster()? else {
            return operation.await;
        };
        let current = self
            .connections
            .lock()
            .await
            .get(&executor)
            .cloned()
            .filter(|c| c.connection_id == connection)
            .ok_or_else(|| ApiError::Conflict("executor connection was replaced".to_owned()))?;
        let owner = current
            .owner
            .as_ref()
            .ok_or_else(|| ApiError::Conflict("executor connection is unbound".to_owned()))?;
        // Incoming events/lease heartbeats must be fenced for their entire DB
        // operation, not only by a check made before a reconnect commits.
        let operation = async {
            cluster.ensure_active().await.map_err(ApiError::Internal)?;
            let mut tx = cluster.pool().begin().await?;
            api_cluster::directory::fence_owner(&mut tx, owner)
                .await
                .map_err(ApiError::Internal)?;
            let outcome = operation.await;
            tx.commit().await?;
            outcome
        };
        tokio::select! {biased;
            _=tokio::time::sleep_until(current.confirmed_until)=>Err(ApiError::Conflict("executor connection deadline expired".to_owned())),
            result=operation=>result,
        }
    }

    pub async fn request_desktop_stream(
        &self,
        executor_id: Uuid,
        run_id: Uuid,
        session_id: Uuid,
        control: bool,
    ) -> Result<(Uuid, oneshot::Receiver<WebSocket>), ApiError> {
        self.ensure_ownership().await?;
        if self.cluster.is_some() {
            return Err(ApiError::Conflict(
                "cluster desktop streaming requires a durable pending ticket".to_owned(),
            ));
        }
        let connections = self.connections.lock().await;
        let connection = connections
            .get(&executor_id)
            .cloned()
            .ok_or_else(|| ApiError::Conflict("the Windows executor is offline".to_owned()))?;
        let stream_id = Uuid::new_v4();
        let (sender, receiver) = oneshot::channel();
        self.pending_streams.lock().await.insert(
            stream_id,
            PendingDesktopStream {
                executor_id,
                connection_id: connection.connection_id,
                credential_id: connection.credential_id,
                session_id,
                expires_at: tokio::time::Instant::now() + Duration::from_secs(300),
                socket: sender,
            },
        );
        drop(connections);
        if !matches!(
            tokio::time::timeout(
                Duration::from_secs(3),
                connection
                    .commands
                    .send(ExecutorServerMessage::DesktopStreamRequested {
                        run_id,
                        session_id,
                        stream_id,
                        control,
                    })
            )
            .await,
            Ok(Ok(()))
        ) {
            self.pending_streams.lock().await.remove(&stream_id);
            return Err(ApiError::Conflict(
                "the Windows executor disconnected".to_owned(),
            ));
        }
        if !self.is_current(executor_id, connection.connection_id).await {
            self.pending_streams.lock().await.remove(&stream_id);
            return Err(ApiError::Conflict(
                "the Windows executor connection was replaced".to_owned(),
            ));
        }
        Ok((stream_id, receiver))
    }

    pub async fn cancel_desktop_stream(&self, stream_id: Uuid) {
        self.pending_streams.lock().await.remove(&stream_id);
    }

    async fn validate_pending_stream(
        &self,
        principal: &ExecutorPrincipal,
        stream_id: Uuid,
    ) -> Result<(), ApiError> {
        self.ensure_ownership().await?;
        let connections = self.connections.lock().await;
        let streams = self.pending_streams.lock().await;
        let valid = streams.get(&stream_id).is_some_and(|pending| {
            pending.executor_id == principal.executor_id
                && pending.credential_id == principal.credential_id
                && pending.expires_at > tokio::time::Instant::now()
                && connections
                    .get(&principal.executor_id)
                    .is_some_and(|connection| {
                        connection.connection_id == pending.connection_id
                            && connection.credential_id == principal.credential_id
                    })
        });
        if valid {
            Ok(())
        } else {
            Err(ApiError::Conflict("executor desktop stream request is missing, expired, or belongs to a replaced connection".to_owned()))
        }
    }

    async fn fulfill_desktop_stream(
        &self,
        principal: ExecutorPrincipal,
        stream_id: Uuid,
        socket: WebSocket,
    ) {
        let executor_id = principal.executor_id;
        if self.ensure_ownership().await.is_err() {
            return;
        }
        let pending = {
            let connections = self.connections.lock().await;
            let mut streams = self.pending_streams.lock().await;
            if streams.get(&stream_id).is_some_and(|pending| {
                pending.executor_id == executor_id
                    && pending.credential_id == principal.credential_id
                    && pending.expires_at > tokio::time::Instant::now()
                    && connections.get(&executor_id).is_some_and(|connection| {
                        connection.connection_id == pending.connection_id
                            && connection.credential_id == principal.credential_id
                    })
            }) {
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
    state.executor_hub.ensure_ownership().await?;
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
    if let Some(cluster) = state.api_cluster.clone() {
        let pending = tokio::time::timeout(
            Duration::from_secs(3),
            cluster.callback_pending(stream_id, &principal),
        )
        .await
        .map_err(|_| {
            ApiError::Unavailable("private desktop callback authorisation timed out".to_owned())
        })?
        .map_err(|_| {
            ApiError::Conflict("private desktop callback is missing, expired, or fenced".to_owned())
        })?;
        let peer = cluster
            .open_callback(&pending, &principal)
            .await
            .map_err(|_| {
                ApiError::Conflict("private desktop callback owner unavailable".to_owned())
            })?;
        let ownership = state.api_ownership();
        return Ok(upgrade.on_upgrade(move |socket| async move {
            let _ = crate::runtime_status::guard_api_future(
                ownership,
                api_cluster::tickets::guard(
                    &cluster,
                    &pending,
                    api_cluster::relay::executor(socket, peer),
                ),
            )
            .await;
        }));
    }
    state
        .executor_hub
        .validate_pending_stream(&principal, stream_id)
        .await?;
    Ok(upgrade.on_upgrade(move |socket| async move {
        if crate::auth::validate_executor_principal(&state.pool, &principal)
            .await
            .is_err()
        {
            return;
        }
        state
            .executor_hub
            .fulfill_desktop_stream(principal, stream_id, socket)
            .await;
    }))
}

async fn run_socket(socket: WebSocket, state: AppState, principal: ExecutorPrincipal) {
    if state.executor_hub.ensure_ownership().await.is_err()
        || crate::auth::validate_executor_principal(&state.pool, &principal)
            .await
            .is_err()
    {
        return;
    }
    let Ok((connection_id, commands)) = state
        .executor_hub
        .register_current(principal.executor_id, principal.credential_id)
        .await
    else {
        return;
    };
    // Cleanup executes even when the ownership guard cancels an in-flight DB
    // operation. A stale cleanup only deletes its exact connection epoch.
    let _ = crate::runtime_status::guard_api_future(state.api_ownership(), async {
        run_registered_socket(socket, &state, &principal, connection_id, commands).await;
        Ok(())
    })
    .await;
    let _ = tokio::time::timeout(
        Duration::from_secs(3),
        state
            .executor_hub
            .unregister(principal.executor_id, connection_id),
    )
    .await;
    tracing::info!(executor_id = %principal.executor_id, "executor WebSocket disconnected");
}

async fn run_registered_socket(
    mut socket: WebSocket,
    state: &AppState,
    principal: &ExecutorPrincipal,
    connection_id: Uuid,
    mut commands: mpsc::Receiver<ExecutorServerMessage>,
) {
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

    let mut current_lease: Option<RunLease> = None;
    let mut dispatch = tokio::time::interval(Duration::from_secs(2));
    dispatch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                if !state.executor_hub.is_current(principal.executor_id, connection_id).await
                    || state.executor_hub.ensure_ownership().await.is_err() { break; }
                if crate::auth::validate_executor_principal(&state.pool,principal).await.is_err() { break; }
                if state.executor_hub.send_command(&mut socket, &command,principal.executor_id,connection_id).await.is_err() { break; }
            }
            _ = dispatch.tick() => {
                if state.executor_hub.renew_current(principal.executor_id,connection_id).await.is_err(){break;}
                if !state.executor_hub.is_current(principal.executor_id, connection_id).await
                    || state.executor_hub.ensure_ownership().await.is_err() { break; }
                if crate::auth::validate_executor_principal(&state.pool,principal).await.is_err() { break; }
                if state.executor_hub.fence_operation(principal.executor_id,connection_id,
                    dispatch_runs(state,principal.executor_id,principal.credential_id,&mut current_lease,&mut socket)).await.is_err(){break;}
            }
            incoming = socket.next() => {
                let Some(incoming) = incoming else { break };
                if !state.executor_hub.is_current(principal.executor_id, connection_id).await
                    || state.executor_hub.ensure_ownership().await.is_err() { break; }
                if crate::auth::validate_executor_principal(&state.pool,principal).await.is_err() { break; }
                match incoming {
                    Ok(Message::Text(text)) => {
                        match serde_json::from_str::<ExecutorClientMessage>(&text) {
                            Ok(message) => {
                                let finished = state.executor_hub.fence_operation(principal.executor_id,connection_id,handle_message(state, principal.executor_id, &mut socket, message)).await;
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
}

async fn dispatch_runs(
    state: &AppState,
    executor_id: Uuid,
    credential_id: Uuid,
    current_lease: &mut Option<RunLease>,
    socket: &mut WebSocket,
) -> Result<(), ApiError> {
    if current_lease.is_none() {
        match db::recover_external_run(&state.pool, executor_id, credential_id, state.lease_seconds)
            .await
        {
            Ok(Some(lease)) => {
                send(
                    socket,
                    &ExecutorServerMessage::Lease {
                        lease: Box::new(lease.clone()),
                    },
                )
                .await
                .map_err(|e| ApiError::Internal(e.into()))?;
                *current_lease = Some(lease);
            }
            Ok(None) => {}
            Err(error) => send_error(socket, "lease_recovery_failed", &error.to_string(), None)
                .await
                .map_err(|e| ApiError::Internal(e.into()))?,
        }
    }
    if let Err(error) = db::heartbeat_executor(
        &state.pool,
        executor_id,
        usize::from(current_lease.is_some()),
    )
    .await
    {
        send_error(
            socket,
            "heartbeat_failed",
            &error.to_string(),
            current_lease.as_ref().map(|l| l.run.spec.id),
        )
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
        return Ok(());
    }
    if current_lease.is_none() {
        match db::claim_external_run(&state.pool, executor_id, credential_id, state.lease_seconds)
            .await
        {
            Ok(Some(lease)) => {
                send(
                    socket,
                    &ExecutorServerMessage::Lease {
                        lease: Box::new(lease.clone()),
                    },
                )
                .await
                .map_err(|e| ApiError::Internal(e.into()))?;
                *current_lease = Some(lease);
            }
            Ok(None) => {}
            Err(error) => send_error(socket, "claim_failed", &error.to_string(), None)
                .await
                .map_err(|e| ApiError::Internal(e.into()))?,
        }
    }
    Ok(())
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
            db::append_external_leased_event(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replaced_connection_is_fenced_and_its_pending_streams_are_canceled() {
        let hub = ExecutorHub::default();
        let executor_id = Uuid::new_v4();
        let credential_id = Uuid::new_v4();
        let (first, _commands) = hub.register(executor_id, credential_id).await;
        let (sender, receiver) = oneshot::channel();
        hub.pending_streams.lock().await.insert(
            Uuid::new_v4(),
            PendingDesktopStream {
                executor_id,
                connection_id: first,
                credential_id,
                session_id: Uuid::new_v4(),
                expires_at: tokio::time::Instant::now() + Duration::from_secs(300),
                socket: sender,
            },
        );
        let (second, _commands) = hub.register(executor_id, Uuid::new_v4()).await;
        assert!(!hub.is_current(executor_id, first).await);
        assert!(hub.is_current(executor_id, second).await);
        assert!(receiver.await.is_err());
        hub.unregister(executor_id, first).await;
        assert!(hub.is_current(executor_id, second).await);
    }

    #[tokio::test]
    async fn disconnect_cancels_pending_streams_for_that_connection() {
        let hub = ExecutorHub::default();
        let executor_id = Uuid::new_v4();
        let credential_id = Uuid::new_v4();
        let (connection_id, _commands) = hub.register(executor_id, credential_id).await;
        let (sender, receiver) = oneshot::channel();
        hub.pending_streams.lock().await.insert(
            Uuid::new_v4(),
            PendingDesktopStream {
                executor_id,
                connection_id,
                credential_id,
                session_id: Uuid::new_v4(),
                expires_at: tokio::time::Instant::now() + Duration::from_secs(300),
                socket: sender,
            },
        );
        hub.unregister(executor_id, connection_id).await;
        assert!(receiver.await.is_err());
        assert!(!hub.is_current(executor_id, connection_id).await);
    }

    #[tokio::test]
    async fn unguarded_process_refuses_executor_streaming() {
        let hub = ExecutorHub::default();
        assert!(matches!(
            hub.request_desktop_stream(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), false)
                .await,
            Err(ApiError::Unavailable(_))
        ));
    }
}
