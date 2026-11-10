//! Own authenticated agent connections and reverse RFB channels. No desktop owner adoption.
use axum::extract::ws::{Message, WebSocket};
use cowork_domain::{Error, Result};
use cowork_storage::{AndroidAgentPrincipal, AndroidDesktopAccess, AndroidHubOwner, Store};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use uuid::Uuid;

type PendingDesktopStream = (Arc<AndroidDesktopAccess>, oneshot::Sender<WebSocket>);

struct Lifetime {
    state: StdMutex<(bool, usize)>,
    changed: Notify,
}
pub(crate) struct ConnectionGuard(Arc<Lifetime>);
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.1 = state.1.saturating_sub(1);
        }
        self.0.changed.notify_one();
    }
}
pub struct DeviceHub {
    pub(crate) store: Store,
    pub(crate) node: Uuid,
    pub(crate) boot: Uuid,
    transport: Option<crate::device_hub_transport::Transport>,
    server: Mutex<Option<tokio::task::JoinHandle<Result<()>>>>,
    shutdown_serial: Mutex<()>,
    agents: Mutex<HashMap<Uuid, (AndroidHubOwner, mpsc::Sender<Value>)>>,
    streams: Mutex<HashMap<Uuid, PendingDesktopStream>>,
    pub(crate) stop: tokio_util::sync::CancellationToken,
    lifetime: Arc<Lifetime>,
}
impl DeviceHub {
    pub(crate) async fn configured(
        store: Store,
        web: Option<&std::path::Path>,
    ) -> Result<Arc<Self>> {
        let mut transport = crate::device_hub_transport::configured(web).await?;
        let listener = transport.as_mut().and_then(|t| t.take_listener());
        let node = std::env::var("COWORK_DEVICE_HUB_NODE_ID")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|s| {
                Uuid::parse_str(&s)
                    .map_err(|_| Error::Validation("Invalid DeviceHub node identity".into()))
            })
            .transpose()?
            .unwrap_or_else(Uuid::new_v4);
        if node.is_nil() {
            return Err(Error::Validation("Invalid DeviceHub node identity".into()));
        }
        let hub = Arc::new(Self {
            store,
            node,
            boot: Uuid::new_v4(),
            transport,
            server: Mutex::new(None),
            shutdown_serial: Mutex::new(()),
            agents: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
            stop: tokio_util::sync::CancellationToken::new(),
            lifetime: Arc::new(Lifetime {
                state: StdMutex::new((false, 0)),
                changed: Notify::new(),
            }),
        });
        if let Some(listener) = listener {
            let acceptor = crate::device_hub_transport::acceptor(
                hub.transport.as_ref().ok_or(Error::Conflict)?,
            );
            let running = hub.clone();
            *hub.server.lock().await = Some(tokio::spawn(async move {
                let result =
                    crate::device_hub_transport::serve(running.clone(), listener, acceptor).await;
                if result.is_err() {
                    running.stop.cancel();
                }
                result
            }));
        }
        Ok(hub)
    }
    pub(crate) fn connection(&self) -> Result<ConnectionGuard> {
        let mut state = self.lifetime.state.lock().map_err(|_| Error::Conflict)?;
        if state.0 {
            return Err(Error::Conflict);
        }
        state.1 += 1;
        Ok(ConnectionGuard(self.lifetime.clone()))
    }
    pub async fn shutdown(&self) {
        let _shutdown = self.shutdown_serial.lock().await;
        if let Ok(mut state) = self.lifetime.state.lock() {
            state.0 = true;
        }
        self.stop.cancel();
        if let Some(server) = self.server.lock().await.take() {
            let _ = server.await;
        }
        loop {
            let changed = self.lifetime.changed.notified();
            if self.lifetime.state.lock().is_ok_and(|v| v.1 == 0) {
                break;
            }
            changed.await;
        }
        let owners: Vec<_> = self.agents.lock().await.drain().map(|(_, v)| v.0).collect();
        for owner in owners {
            let _ = self.store.android_hub_release(&owner).await;
        }
        self.streams.lock().await.clear();
    }
    pub(crate) async fn install(
        &self,
        a: &AndroidAgentPrincipal,
    ) -> Result<(AndroidHubOwner, mpsc::Receiver<Value>)> {
        let id = a.executor_id;
        let mut map = self.agents.lock().await;
        if self.stop.is_cancelled() || map.contains_key(&id) {
            return Err(Error::Conflict);
        }
        let owner = self
            .store
            .android_hub_claim(
                a,
                self.node,
                self.boot,
                self.transport.as_ref().map(|v| v.origin.as_str()),
            )
            .await?;
        let (tx, rx) = mpsc::channel(32);
        map.insert(id, (owner.clone(), tx));
        Ok((owner, rx))
    }
    pub(crate) async fn remove(&self, owner: &AndroidHubOwner) {
        let mut map = self.agents.lock().await;
        if map.get(&owner.executor).is_some_and(|v| v.0 == *owner) {
            map.remove(&owner.executor);
        }
        drop(map);
        let _ = self.store.android_hub_release(owner).await;
    }
    pub(crate) fn stopped(&self) -> tokio_util::sync::CancellationToken {
        self.stop.clone()
    }
    pub(crate) async fn receive_stream(
        &self,
        store: &cowork_storage::Store,
        a: &AndroidAgentPrincipal,
        id: Uuid,
        socket: WebSocket,
    ) -> Result<()> {
        let (owner, access) = store.android_hub_agent_stream(a, id).await?;
        if owner.node != self.node || owner.boot != self.boot {
            let token = crate::device_hub_transport::secret()?;
            store
                .android_hub_forward(&owner, &access, Some(a), Some(id), &token)
                .await?;
            let peer = self
                .transport
                .as_ref()
                .ok_or(Error::Conflict)?
                .connect(&owner, "agent", &token)
                .await?;
            let mut socket = socket;
            let result =
                crate::device_hub_transport::bridge(self, &owner, &access, &mut socket, peer).await;
            let _ = tokio::time::timeout(Duration::from_secs(2), socket.close()).await;
            return result;
        }
        self.receive_local(a, &owner, id, socket).await
    }
    pub(crate) async fn receive_local(
        &self,
        a: &AndroidAgentPrincipal,
        owner: &AndroidHubOwner,
        id: Uuid,
        socket: WebSocket,
    ) -> Result<()> {
        if owner.node != self.node || owner.boot != self.boot {
            return Err(Error::Conflict);
        }
        let (actual, _) = self.store.android_hub_agent_stream(a, id).await?;
        if actual != *owner {
            return Err(Error::Conflict);
        }
        let mut map = self.streams.lock().await;
        let (access, _) = map.get(&id).ok_or(Error::Forbidden)?;
        self.store.android_desktop_agent_current(a, access).await?;
        let (_, send) = map.remove(&id).ok_or(Error::Conflict)?;
        self.store.android_hub_stream_remove(owner, id).await?;
        send.send(socket).map_err(|_| Error::Conflict)
    }
    pub(crate) async fn relay(
        &self,
        store: &cowork_storage::Store,
        access: Arc<AndroidDesktopAccess>,
        mut client: WebSocket,
    ) -> Result<()> {
        let result = self.relay_inner(store, &access, &mut client).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), client.close()).await;
        let _ = store.android_desktop_stream_finished(&access).await;
        result
    }
    async fn relay_inner(
        &self,
        store: &cowork_storage::Store,
        access: &Arc<AndroidDesktopAccess>,
        client: &mut WebSocket,
    ) -> Result<()> {
        let owner = store.android_hub_owner(access).await?;
        if owner.node == self.node && owner.boot == self.boot {
            return self.relay_local(&owner, access, client).await;
        }
        let token = crate::device_hub_transport::secret()?;
        store
            .android_hub_forward(&owner, access, None, None, &token)
            .await?;
        let peer = self
            .transport
            .as_ref()
            .ok_or(Error::Conflict)?
            .connect(&owner, "desktop", &token)
            .await?;
        crate::device_hub_transport::bridge(self, &owner, access, client, peer).await
    }
    pub(crate) async fn relay_local(
        &self,
        owner: &AndroidHubOwner,
        access: &Arc<AndroidDesktopAccess>,
        client: &mut WebSocket,
    ) -> Result<()> {
        let store = &self.store;
        if owner.node != self.node || owner.boot != self.boot {
            return Err(Error::Conflict);
        }
        store.android_desktop_access_current(access).await?;
        store.android_hub_current(owner).await?;
        let stream = Uuid::new_v4();
        store
            .android_hub_stream_register(owner, access, stream)
            .await?;
        let (tx, rx) = oneshot::channel();
        self.streams
            .lock()
            .await
            .insert(stream, (access.clone(), tx));
        let sent = if let Some((actual, sender)) = self
            .agents
            .lock()
            .await
            .get(&access.executor)
            .filter(|v| v.0 == *owner)
        {
            let _ = actual;
            sender.try_send(json!({"type":"desktop_stream_requested","run_id":access.run,"session_id":access.desktop,"stream_id":stream,"control":access.control})).is_ok()
        } else {
            false
        };
        if !sent {
            self.streams.lock().await.remove(&stream);
            let _ = store.android_hub_stream_remove(owner, stream).await;
            return Err(Error::Conflict);
        }
        let stop = self.stop.clone();
        let remote = tokio::select! { _ = stop.cancelled() => Err(Error::Conflict), v = tokio::time::timeout(Duration::from_secs(120), rx) => v.map_err(|_| Error::Conflict).and_then(|v| v.map_err(|_| Error::Conflict)) };
        self.streams.lock().await.remove(&stream);
        let _ = store.android_hub_stream_remove(owner, stream).await;
        let mut remote = remote?;
        let mut timer = tokio::time::interval(Duration::from_secs(2));
        let mut result = Ok(());
        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = timer.tick() => { if let Err(e) = store.android_desktop_access_current(access).await { result = Err(e); break; }
                if let Err(e)=store.android_hub_current(owner).await {result=Err(e);break;} },
                v = client.next() => match v {
                    Some(Ok(Message::Binary(v))) if v.len() <= 16*1024*1024 => { if !send(&mut remote, Message::Binary(v)).await { break; } },
                    Some(Ok(Message::Ping(v))) => { if !send(client, Message::Pong(v)).await { break; } },
                    Some(Ok(Message::Pong(_))) => {},
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => break,
                },
                v = remote.next() => match v {
                    Some(Ok(Message::Binary(v))) if v.len() <= 16*1024*1024 => { if !send(client, Message::Binary(v)).await { break; } },
                    Some(Ok(Message::Ping(v))) => { if !send(&mut remote, Message::Pong(v)).await { break; } },
                    Some(Ok(Message::Pong(_))) => {},
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => break,
                }
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), remote.close()).await;
        result
    }
}
pub(crate) async fn send(socket: &mut WebSocket, message: Message) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(5), socket.send(message)).await,
        Ok(Ok(()))
    )
}
