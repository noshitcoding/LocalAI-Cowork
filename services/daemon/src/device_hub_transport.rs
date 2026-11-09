//! Replica transport is explicit HTTPS, authenticated by a short-lived one-use
//! database grant. It never sends a user's session or an agent's bearer token.
use crate::android_device_channel::{DeviceHub, send};
#[cfg(windows)]
use crate::private_secret_file;
use axum::{
    Router,
    extract::{Path, State, WebSocketUpgrade},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_domain::{Error, Result};
use cowork_storage::AndroidHubOwner;
use futures_util::{SinkExt, StreamExt};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use std::{
    io::Read,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::net::TcpListener;
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    },
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message as PeerMessage,
        protocol::{Role, WebSocketConfig},
    },
};
use tower::ServiceExt;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    format: u32,
    origin: String,
    listen: std::net::SocketAddr,
    certificate_file: PathBuf,
    key_file: PathBuf,
    ca_certificate_file: PathBuf,
}
pub(crate) struct Transport {
    pub origin: String,
    client: reqwest::Client,
    listener: Option<TcpListener>,
    acceptor: TlsAcceptor,
}
fn invalid() -> Error {
    Error::Validation("Invalid private DeviceHub HTTPS configuration".into())
}
fn private(path: &FsPath, limit: u64) -> Result<Zeroizing<Vec<u8>>> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(invalid());
    }
    for item in path.ancestors() {
        let m = std::fs::symlink_metadata(item).map_err(|_| invalid())?;
        if m.file_type().is_symlink() {
            return Err(invalid());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                return Err(invalid());
            }
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    let mut file = options.open(path).map_err(|_| invalid())?;
    let m = file.metadata().map_err(|_| invalid())?;
    if !m.is_file() || m.len() == 0 || m.len() > limit {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if m.mode() & 0o077 != 0 || m.nlink() != 1 || m.uid() != unsafe { libc::geteuid() } {
            return Err(invalid());
        }
    }
    #[cfg(windows)]
    private_secret_file::validate(&file).map_err(|_| invalid())?;
    let mut bytes = Zeroizing::new(Vec::new());
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() as u64 != m.len() {
        return Err(invalid());
    }
    Ok(bytes)
}
fn pem(bytes: &[u8], kind: &str) -> Result<Vec<Vec<u8>>> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let begin = format!("-----BEGIN {kind}-----");
    let end = format!("-----END {kind}-----");
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some((_, tail)) = rest.split_once(&begin) {
        let (body, next) = tail.split_once(&end).ok_or_else(invalid)?;
        let compact: Zeroizing<String> =
            Zeroizing::new(body.chars().filter(|c| !c.is_ascii_whitespace()).collect());
        blocks.push(STANDARD.decode(compact.as_bytes()).map_err(|_| invalid())?);
        rest = next;
    }
    if blocks.is_empty() || blocks.len() > 16 {
        return Err(invalid());
    }
    Ok(blocks)
}
pub(crate) fn secret() -> Result<Zeroizing<String>> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Conflict)?;
    Ok(Zeroizing::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
    ))
}
pub(crate) async fn configured(web: Option<&FsPath>) -> Result<Option<Transport>> {
    let Some(path) = std::env::var_os("COWORK_DEVICE_HUB_CONFIG").filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let c: Configuration =
        serde_json::from_slice(&private(FsPath::new(&path), 65536)?).map_err(|_| invalid())?;
    if let Some(web) = web {
        let web = std::path::absolute(web).map_err(|_| invalid())?;
        for path in [
            &c.certificate_file,
            &c.key_file,
            &c.ca_certificate_file,
            &PathBuf::from(path),
        ] {
            if path.starts_with(&web) {
                return Err(invalid());
            }
        }
    }
    let origin = if c.origin == "kubernetes_pod_dns" {
        let ip: std::net::Ipv4Addr = std::env::var("COWORK_DEVICE_HUB_POD_IP")
            .map_err(|_| invalid())?
            .parse()
            .map_err(|_| invalid())?;
        let namespace = std::env::var("COWORK_DEVICE_HUB_NAMESPACE").map_err(|_| invalid())?;
        if ip.is_unspecified()
            || ip.is_multicast()
            || ip.is_loopback()
            || namespace.is_empty()
            || namespace.len() > 63
            || namespace.starts_with('-')
            || namespace.ends_with('-')
            || !namespace
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(invalid());
        }
        format!(
            "https://{}.{}.pod.cluster.local:{}/",
            ip.to_string().replace('.', "-"),
            namespace,
            c.listen.port()
        )
    } else {
        c.origin.clone()
    };
    let url = reqwest::Url::parse(&origin).map_err(|_| invalid())?;
    if c.format != 1
        || c.listen.port() == 0
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port_or_known_default() != Some(c.listen.port())
    {
        return Err(invalid());
    }
    let ca = private(&c.ca_certificate_file, 1024 * 1024)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .tls_built_in_root_certs(false)
        .connect_timeout(Duration::from_secs(5))
        .add_root_certificate(reqwest::Certificate::from_pem(&ca).map_err(|_| invalid())?)
        .build()
        .map_err(|_| invalid())?;
    let certs = pem(&private(&c.certificate_file, 1024 * 1024)?, "CERTIFICATE")?
        .into_iter()
        .map(CertificateDer::from)
        .collect();
    let mut keys = pem(&private(&c.key_file, 65536)?, "PRIVATE KEY")?;
    if keys.len() != 1 {
        return Err(invalid());
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| invalid())?
        .with_no_client_auth()
        .with_single_cert(
            certs,
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(keys.remove(0))),
        )
        .map_err(|_| invalid())?;
    Ok(Some(Transport {
        origin: url.to_string(),
        client,
        listener: Some(TcpListener::bind(c.listen).await.map_err(|_| invalid())?),
        acceptor: TlsAcceptor::from(Arc::new(tls)),
    }))
}
impl Transport {
    pub(crate) fn take_listener(&mut self) -> Option<TcpListener> {
        self.listener.take()
    }
    pub(crate) async fn connect(
        &self,
        owner: &AndroidHubOwner,
        kind: &str,
        token: &str,
    ) -> Result<WebSocketStream<reqwest::Upgraded>> {
        let origin = owner.origin.as_ref().ok_or(Error::Conflict)?;
        let url = reqwest::Url::parse(origin).map_err(|_| Error::Conflict)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(kind, "desktop" | "agent")
        {
            return Err(Error::Conflict);
        }
        let mut random = [0u8; 16];
        use ring::rand::SecureRandom;
        ring::rand::SystemRandom::new()
            .fill(&mut random)
            .map_err(|_| Error::Conflict)?;
        let key = STANDARD.encode(random);
        let mut url = url
            .join(&format!("internal/device-hub/{kind}"))
            .map_err(|_| Error::Conflict)?;
        url.query_pairs_mut().append_pair("ticket", token);
        // A single handshake attempt. A failed or ambiguous consume requires a
        // newly authorized desktop session/ticket, never a replay or failover.
        let response = tokio::time::timeout(
            Duration::from_secs(10),
            self.client
                .get(url)
                .version(reqwest::Version::HTTP_11)
                .header("Connection", "Upgrade")
                .header("Upgrade", "websocket")
                .header("Sec-WebSocket-Version", "13")
                .header("Sec-WebSocket-Key", &key)
                .send(),
        )
        .await
        .map_err(|_| Error::Conflict)?
        .map_err(|_| Error::Conflict)?;
        let expected = STANDARD.encode(
            ring::digest::digest(
                &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
                format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes(),
            )
            .as_ref(),
        );
        if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS
            || response
                .headers()
                .get("Sec-WebSocket-Accept")
                .and_then(|v| v.to_str().ok())
                != Some(expected.as_str())
            || !response
                .headers()
                .get("Upgrade")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        {
            return Err(Error::Conflict);
        }
        let upgraded = response.upgrade().await.map_err(|_| Error::Conflict)?;
        Ok(WebSocketStream::from_raw_socket(
            upgraded,
            Role::Client,
            Some(
                WebSocketConfig::default()
                    .max_message_size(Some(16 * 1024 * 1024))
                    .max_frame_size(Some(16 * 1024 * 1024)),
            ),
        )
        .await)
    }
}
#[derive(Deserialize)]
struct Ticket {
    ticket: String,
}
async fn forwarded(
    State(hub): State<Arc<DeviceHub>>,
    Path(kind): Path<String>,
    axum::extract::Query(ticket): axum::extract::Query<Ticket>,
    ws: WebSocketUpgrade,
) -> Response {
    let token = Zeroizing::new(ticket.ticket);
    let grant = match hub
        .store
        .android_hub_take_forward(hub.node, hub.boot, &token, &kind)
        .await
    {
        Ok(v) => v,
        Err(_) => return axum::http::StatusCode::NOT_FOUND.into_response(),
    };
    let guard = match hub.connection() {
        Ok(v) => v,
        Err(_) => return axum::http::StatusCode::CONFLICT.into_response(),
    };
    ws.max_message_size(16 * 1024 * 1024)
        .on_upgrade(move |mut socket| async move {
            let _guard = guard;
            if kind == "desktop" {
                let access = Arc::new(grant.access);
                let _ = hub.relay_local(&grant.owner, &access, &mut socket).await;
                let _ = hub.store.android_desktop_stream_finished(&access).await;
            } else if let (Some(a), Some(id)) = (grant.agent, grant.stream) {
                let _ = hub.receive_local(&a, &grant.owner, id, socket).await;
                return;
            }
            let _ = tokio::time::timeout(Duration::from_secs(2), socket.close()).await;
        })
        .into_response()
}
pub(crate) async fn serve(
    hub: Arc<DeviceHub>,
    listener: TcpListener,
    acceptor: TlsAcceptor,
) -> Result<()> {
    let app = Router::new()
        .route("/internal/device-hub/{kind}", get(forwarded))
        .with_state(hub.clone());
    let mut jobs = tokio::task::JoinSet::new();
    let slots = Arc::new(tokio::sync::Semaphore::new(128));
    loop {
        tokio::select! {
            _=hub.stop.cancelled()=>break,
            Some(_)=jobs.join_next(),if !jobs.is_empty()=>{},
            accepted=listener.accept()=>{let (socket,_)=accepted.map_err(cowork_domain::storage_error)?;let Ok(permit)=slots.clone().try_acquire_owned()else{continue};let acceptor=acceptor.clone();let app=app.clone();let stop=hub.stop.clone();
                jobs.spawn(async move{let _permit=permit;let Ok(Ok(socket))=tokio::time::timeout(Duration::from_secs(5),acceptor.accept(socket)).await else{return};let service=hyper::service::service_fn(move|request|{let app=app.clone();async move{app.oneshot(request).await}});tokio::select!{_=stop.cancelled()=>{},_=tokio::time::timeout(Duration::from_secs(15),hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).with_upgrades())=>{}}});
            }
        }
    }
    while jobs.join_next().await.is_some() {}
    Ok(())
}
pub(crate) fn acceptor(t: &Transport) -> TlsAcceptor {
    t.acceptor.clone()
}
pub(crate) async fn bridge(
    hub: &DeviceHub,
    owner: &AndroidHubOwner,
    access: &cowork_storage::AndroidDesktopAccess,
    local: &mut axum::extract::ws::WebSocket,
    mut peer: WebSocketStream<reqwest::Upgraded>,
) -> Result<()> {
    use axum::extract::ws::Message;
    let mut timer = tokio::time::interval(Duration::from_secs(2));
    let result = loop {
        tokio::select! {
            _=hub.stop.cancelled()=>break Ok(()),
            _=timer.tick()=>{if let Err(e)=hub.store.android_desktop_access_current(access).await{break Err(e)}
                if let Err(e)=hub.store.android_hub_current(owner).await{break Err(e)}},
            v=local.next()=>match v {Some(Ok(Message::Binary(v))) if v.len()<=16*1024*1024=>{if !matches!(tokio::time::timeout(Duration::from_secs(5),peer.send(PeerMessage::Binary(v))).await,Ok(Ok(()))){break Err(Error::Conflict)}},Some(Ok(Message::Ping(v)))=>{if !send(local,Message::Pong(v)).await{break Ok(())}},Some(Ok(Message::Pong(_)))=>{},_=>break Ok(())},
            v=peer.next()=>match v {Some(Ok(PeerMessage::Binary(v))) if v.len()<=16*1024*1024=>{if !send(local,Message::Binary(v)).await{break Ok(())}},Some(Ok(PeerMessage::Ping(v)))=>{if !matches!(tokio::time::timeout(Duration::from_secs(5),peer.send(PeerMessage::Pong(v))).await,Ok(Ok(()))){break Ok(())}},Some(Ok(PeerMessage::Pong(_)))=>{},_=>break Ok(())}
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), peer.close(None)).await;
    result
}
