//! Experimental, default-off private API connection routing.
//! DB-time leases and one-use epochs are fencing proofs. Notifications and
//! configured peers never turn an unknown command outcome into retry authority.
mod config;
mod controls;
pub(crate) mod directory;
pub(crate) mod linux;
pub(crate) mod relay;
pub(crate) mod tickets;
mod transport;

use anyhow::{ensure, Result};
pub use config::ClusterConfig;
use directory::Owner;
use serde::Serialize;
use sqlx::{Connection, PgConnection, PgPool};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{watch, Mutex, Semaphore},
    task::JoinSet,
    time::Instant,
};
use uuid::Uuid;

const LOCK_KEY: i64 = 0x4357_4b41_5049_0001;
pub(crate) const LEASE_SECONDS: u64 = 8;
const DB_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct ApiCluster {
    inner: Arc<Inner>,
}
#[derive(Clone)]
pub(crate) struct WeakCluster {
    inner: Weak<Inner>,
}
impl WeakCluster {
    pub(crate) fn upgrade(&self) -> Option<ApiCluster> {
        self.inner.upgrade().map(|inner| ApiCluster { inner })
    }
}

struct Inner {
    config: ClusterConfig,
    pool: PgPool,
    process_epoch: Uuid,
    certificate_sha256: Vec<u8>,
    tls: transport::Tls,
    connection: Mutex<Option<PgConnection>>,
    backend_pid: i32,
    confirmed_until: Mutex<Instant>,
    active: AtomicBool,
    lost: watch::Sender<bool>,
}

#[derive(Serialize)]
pub struct ApiClusterStatus {
    pub mode: &'static str,
    pub multiple_api_replicas_supported: bool,
    pub ownership_healthy: bool,
    pub executor_connection_routing: &'static str,
    pub desktop_connection_routing: &'static str,
    pub configured_peers: usize,
}

impl ApiCluster {
    pub async fn acquire(pool: PgPool, config: ClusterConfig) -> Result<Self> {
        config.validate()?;
        let tls = transport::Tls::new(&config)?;
        let mut connection = tokio::time::timeout(
            Duration::from_secs(10),
            PgConnection::connect_with(pool.connect_options().as_ref()),
        )
        .await??;
        let shared: bool = tokio::time::timeout(
            DB_TIMEOUT,
            sqlx::query_scalar("SELECT pg_try_advisory_lock_shared($1)")
                .bind(LOCK_KEY)
                .fetch_one(&mut connection),
        )
        .await??;
        ensure!(shared, "another singleton API process owns this database");
        let backend_pid: i32 = tokio::time::timeout(
            DB_TIMEOUT,
            sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut connection),
        )
        .await??;
        let started = Instant::now();
        let (lost, _) = watch::channel(false);
        let cluster = Self {
            inner: Arc::new(Inner {
                certificate_sha256: tls.own_sha256.clone(),
                tls,
                config,
                pool,
                process_epoch: Uuid::new_v4(),
                connection: Mutex::new(Some(connection)),
                backend_pid,
                confirmed_until: Mutex::new(started + Duration::from_secs(LEASE_SECONDS)),
                active: AtomicBool::new(true),
                lost,
            }),
        };
        tokio::time::timeout(DB_TIMEOUT, directory::register_node(&cluster)).await??;
        cluster.ensure_active().await?;
        let weak = cluster.downgrade();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(cluster) = weak.upgrade() else {
                    return;
                };
                if !cluster.inner.active.load(Ordering::Acquire) {
                    return;
                }
                if cluster.renew().await.is_err() {
                    cluster.lose();
                    return;
                }
            }
        });
        Ok(cluster)
    }

    fn lose(&self) {
        self.inner.active.store(false, Ordering::Release);
        self.inner.lost.send_replace(true);
    }
    pub fn begin_draining(&self) {
        self.lose();
    }
    pub async fn wait_until_lost(&self) {
        let mut lost = self.inner.lost.subscribe();
        loop {
            if *lost.borrow_and_update() {
                return;
            }
            if lost.changed().await.is_err() {
                return;
            }
        }
    }

    pub async fn ensure_active(&self) -> Result<()> {
        ensure!(
            self.inner.active.load(Ordering::Acquire),
            "API cluster ownership is no longer active"
        );
        if *self.inner.confirmed_until.lock().await <= Instant::now() {
            self.lose();
            anyhow::bail!("last confirmed API node lease expired");
        }
        match tokio::time::timeout(
            DB_TIMEOUT,
            directory::verify_node(
                &self.inner.pool,
                self.inner.config.group_id,
                self.inner.config.node_id,
                self.inner.process_epoch,
            ),
        )
        .await
        {
            Ok(Ok(())) => {}
            _ => {
                self.lose();
                anyhow::bail!("API cluster DB ownership unavailable");
            }
        }
        if !self.inner.active.load(Ordering::Acquire)
            || *self.inner.confirmed_until.lock().await <= Instant::now()
        {
            self.lose();
            anyhow::bail!("last confirmed API node lease expired");
        }
        Ok(())
    }

    async fn renew(&self) -> Result<()> {
        let started = Instant::now();
        ensure!(
            self.inner.active.load(Ordering::Acquire),
            "API cluster is draining"
        );
        ensure!(
            *self.inner.confirmed_until.lock().await > started,
            "API cluster local deadline expired"
        );
        let mut connection = tokio::time::timeout(DB_TIMEOUT, self.inner.connection.lock()).await?;
        let owns=match connection.as_mut(){Some(connection)=>tokio::time::timeout(DB_TIMEOUT,sqlx::query_scalar::<_,bool>("SELECT pg_backend_pid()=$1 AND EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND pid=pg_backend_pid() AND classid::bigint=$2 AND objid::bigint=$3 AND objsubid=1 AND mode='ShareLock' AND granted)")
            .bind(self.inner.backend_pid).bind(LOCK_KEY>>32).bind(LOCK_KEY&0xffff_ffff).fetch_one(connection)).await??,None=>false};
        ensure!(owns, "API cluster shared session lock was lost");
        drop(connection);
        tokio::time::timeout(DB_TIMEOUT, directory::renew_node(self)).await??;
        ensure!(
            *self.inner.confirmed_until.lock().await > Instant::now(),
            "renewal answered after previous API lease deadline"
        );
        ensure!(
            self.inner.active.load(Ordering::Acquire),
            "API cluster began draining during renewal"
        );
        *self.inner.confirmed_until.lock().await = started + Duration::from_secs(LEASE_SECONDS);
        Ok(())
    }

    pub fn status(&self) -> ApiClusterStatus {
        ApiClusterStatus {
            mode: "experimental_api_cluster",
            multiple_api_replicas_supported: false,
            ownership_healthy: self.inner.active.load(Ordering::Acquire),
            executor_connection_routing: "durable_credential_connection_epochs",
            desktop_connection_routing: "allowlisted_private_mtls",
            configured_peers: self.inner.config.peers.len(),
        }
    }
    pub(crate) fn downgrade(&self) -> WeakCluster {
        WeakCluster {
            inner: Arc::downgrade(&self.inner),
        }
    }
    pub(crate) fn local_owner(&self) -> Owner {
        Owner {
            group_id: self.inner.config.group_id,
            node_id: self.inner.config.node_id,
            process_epoch: self.inner.process_epoch,
            executor_id: None,
            connection_epoch: None,
            credential_id: None,
        }
    }
    pub(crate) fn pool(&self) -> &PgPool {
        &self.inner.pool
    }

    pub async fn start_peer_server(
        &self,
        hub: crate::executor_ws::ExecutorHub,
    ) -> Result<PeerServer> {
        self.ensure_active().await?;
        let listener = TcpListener::bind(self.inner.config.listen_addr).await?;
        Ok(PeerServer {
            listener,
            cluster: self.clone(),
            hub,
        })
    }

    pub(crate) async fn prepare_stream(
        &self,
        session: Uuid,
        run: Uuid,
        user: Uuid,
        auth_session: Uuid,
        control: bool,
        external: Option<Uuid>,
    ) -> Result<tickets::Pending> {
        tickets::prepare(self, session, run, user, auth_session, control, external).await
    }

    pub(crate) async fn stop_linux_desktop(
        &self,
        session: Uuid,
        run: Uuid,
        user: Uuid,
        auth: Uuid,
        runner: &crate::desktop::RunnerControl,
    ) -> Result<()> {
        controls::stop(self, session, run, user, auth, runner).await
    }

    fn request(
        &self,
        pending: &tickets::Pending,
        role: transport::Role,
        principal: Option<&crate::auth::ExecutorPrincipal>,
    ) -> transport::Request {
        transport::Request {
            protocol: 1,
            group_id: self.inner.config.group_id,
            source_node_id: self.inner.config.node_id,
            source_process_epoch: self.inner.process_epoch,
            target_node_id: pending.owner.node_id,
            target_process_epoch: pending.owner.process_epoch,
            stream_id: pending.stream_id,
            role,
            executor_id: principal.map(|p| p.executor_id),
            credential_id: principal.map(|p| p.credential_id),
        }
    }

    pub(crate) async fn open_user_stream(
        &self,
        pending: &tickets::Pending,
    ) -> Result<transport::PeerSocket> {
        tickets::authorise(self, pending).await?;
        transport::open(self, &self.request(pending, transport::Role::User, None)).await
    }

    pub(crate) async fn callback_pending(
        &self,
        stream: Uuid,
        principal: &crate::auth::ExecutorPrincipal,
    ) -> Result<tickets::Pending> {
        let pending = tickets::load(self, stream).await?;
        relay::callback_matches(&pending, principal)?;
        tickets::authorise(self, &pending).await?;
        Ok(pending)
    }

    pub(crate) async fn open_callback(
        &self,
        pending: &tickets::Pending,
        principal: &crate::auth::ExecutorPrincipal,
    ) -> Result<transport::PeerSocket> {
        transport::open(
            self,
            &self.request(pending, transport::Role::ExecutorCallback, Some(principal)),
        )
        .await
    }
}

pub struct PeerServer {
    listener: TcpListener,
    cluster: ApiCluster,
    hub: crate::executor_ws::ExecutorHub,
}
impl PeerServer {
    pub async fn run(self, runner: Option<Arc<crate::desktop::RunnerControl>>) -> Result<()> {
        let mut peers = JoinSet::new();
        let slots = Arc::new(Semaphore::new(64));
        let result=async{loop{tokio::select!{biased;
            _=self.cluster.wait_until_lost()=>return Ok(()),
            Some(outcome)=peers.join_next()=>{if let Err(error)=outcome{return Err(error.into());}},
            accepted=self.listener.accept()=>{
                let(socket,_)=accepted?;let Ok(slot)=slots.clone().try_acquire_owned() else{drop(socket);continue};
                let cluster=self.cluster.clone();let hub=self.hub.clone();let runner=runner.clone();
                peers.spawn(async move{let _slot=slot;let _=handle_peer(cluster,hub,runner,socket).await;});
            },
        }}}.await;
        self.cluster.begin_draining();
        peers.abort_all();
        while peers.join_next().await.is_some() {}
        let _ = tokio::time::timeout(DB_TIMEOUT, directory::release_node(&self.cluster)).await;
        result
    }
}

async fn handle_peer(
    cluster: ApiCluster,
    hub: crate::executor_ws::ExecutorHub,
    runner: Option<Arc<crate::desktop::RunnerControl>>,
    socket: tokio::net::TcpStream,
) -> Result<()> {
    let tls = tokio::time::timeout(
        Duration::from_secs(3),
        cluster.inner.tls.acceptor.accept(socket),
    )
    .await??;
    let certificate = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .ok_or_else(|| anyhow::anyhow!("private client certificate missing"))?;
    use sha2::{Digest, Sha256};
    let fingerprint = hex::encode(Sha256::digest(certificate.as_ref()));
    let mut socket: transport::PeerSocket = Box::pin(tls);
    let request: transport::Request =
        tokio::time::timeout(Duration::from_secs(3), transport::read_initial(&mut socket))
            .await??;
    transport::identity(&cluster.inner.config, &fingerprint, &request)?;
    ensure!(
        request.target_process_epoch == cluster.inner.process_epoch,
        "private target API process epoch changed"
    );
    directory::verify_node(
        cluster.pool(),
        request.group_id,
        request.source_node_id,
        request.source_process_epoch,
    )
    .await?;
    if matches!(request.role, transport::Role::DesktopStop) {
        return controls::handle(&cluster, runner.as_deref(), socket, &request).await;
    }
    let pending = tickets::load(&cluster, request.stream_id).await?;
    ensure!(
        pending.owner.node_id == cluster.inner.config.node_id
            && pending.owner.process_epoch == cluster.inner.process_epoch,
        "desktop connection owner changed"
    );
    match request.role {
        transport::Role::User => {
            ensure!(
                request.executor_id.is_none() && request.credential_id.is_none(),
                "private user bridge cannot supply executor identity"
            );
            tickets::claim_user(
                &cluster,
                &pending,
                request.source_node_id,
                request.source_process_epoch,
            )
            .await?;
            tickets::claim_command(&cluster, &pending).await?;
            if pending.owner.executor_id.is_some() {
                let receiver = hub.request_cluster_desktop_stream(pending.clone()).await?;
                transport::acknowledge(&mut socket).await?;
                let result = tickets::guard(&cluster, &pending, async {
                    let executor =
                        tokio::time::timeout(Duration::from_secs(300), receiver).await??;
                    relay::owner(socket, executor).await
                })
                .await;
                hub.cancel_cluster_desktop_stream(pending.stream_id).await;
                tickets::cancel(&cluster, pending.stream_id).await;
                result
            } else {
                let runner =
                    runner.ok_or_else(|| anyhow::anyhow!("desktop owner has no local runner"))?;
                transport::acknowledge(&mut socket).await?;
                let result = tickets::guard(
                    &cluster,
                    &pending,
                    relay::runner(
                        socket,
                        &runner,
                        cluster.pool(),
                        pending.session_id,
                        pending.control,
                    ),
                )
                .await;
                tickets::cancel(&cluster, pending.stream_id).await;
                result
            }
        }
        transport::Role::ExecutorCallback => {
            let principal = crate::auth::ExecutorPrincipal {
                executor_id: request
                    .executor_id
                    .ok_or_else(|| anyhow::anyhow!("private callback executor missing"))?,
                credential_id: request
                    .credential_id
                    .ok_or_else(|| anyhow::anyhow!("private callback credential missing"))?,
            };
            tickets::claim_callback(&cluster, &pending, &principal).await?;
            transport::acknowledge(&mut socket).await?;
            hub.fulfill_cluster_desktop_stream(&pending, socket).await
        }
        transport::Role::DesktopStop => {
            unreachable!("desktop controls were dispatched before RFB slots")
        }
    }
}
