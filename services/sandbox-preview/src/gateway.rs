//! A separate-origin preview proxy. Its bearer only validates previews; sandbox
//! application requests receive neither app cookies nor runner credentials.
use crate::config::{Config, dns_label};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{
        Request, State,
        ws::{
            Message as BrowserMessage, WebSocket, WebSocketUpgrade,
            rejection::WebSocketUpgradeRejection,
        },
    },
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use cowork_runner_protocol::{SandboxPreviewGrant, Signed};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message as GuestMessage, client::IntoClientRequest, protocol::WebSocketConfig},
};

const COOKIE: &str = "__Host-cowork-preview";
const MAX_REQUEST: usize = 8 * 1024 * 1024;
const MAX_RESPONSE: usize = 32 * 1024 * 1024;
const BOOTSTRAP_SCRIPT: &str = r#"(async()=>{const ticket=location.hash.slice(1);history.replaceState(null,'',location.pathname);if(!ticket||ticket.length>3500){document.body.textContent='Preview ticket missing or invalid.';return;}const r=await fetch('/_cowork/bootstrap',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({ticket}),credentials:'same-origin',redirect:'error'});if(r.ok)location.replace('/');else document.body.textContent='Preview access expired or unavailable.';})().catch(()=>{document.body.textContent='Preview access unavailable.';});"#;

pub struct Gateway {
    pub config: Config,
    validator_token: String,
    client: reqwest::Client,
    #[cfg(test)]
    test_upstream: Option<std::net::SocketAddr>,
}
impl Gateway {
    pub fn new(
        config: Config,
        validator_token: String,
        client: reqwest::Client,
    ) -> anyhow::Result<Arc<Self>> {
        config.validate()?;
        Ok(Arc::new(Self {
            config,
            validator_token,
            client,
            #[cfg(test)]
            test_upstream: None,
        }))
    }
    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/healthz", get(|| async { StatusCode::NO_CONTENT }))
            .route("/_cowork/bootstrap", get(bootstrap).post(establish))
            .fallback(proxy)
            .with_state(self)
    }
    fn host(&self, headers: &HeaderMap) -> Result<String, StatusCode> {
        if headers.get_all(header::HOST).iter().count() != 1 {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let value = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let value = value
            .strip_suffix(":443")
            .unwrap_or(value)
            .to_ascii_lowercase();
        let suffix = format!(".{}", self.config.preview_domain);
        let prefix = value
            .strip_suffix(&suffix)
            .ok_or(StatusCode::UNAUTHORIZED)?;
        if !dns_label(prefix) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(value)
    }
    fn ticket(
        &self,
        raw: &str,
        host: &str,
        now: i64,
    ) -> Result<Signed<SandboxPreviewGrant>, StatusCode> {
        if raw.is_empty() || raw.len() > 3500 {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let grant: Signed<SandboxPreviewGrant> =
            serde_json::from_slice(&bytes).map_err(|_| StatusCode::UNAUTHORIZED)?;
        grant
            .verify("sandbox_preview", &self.config.runner_public_key)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let p = &grant.payload;
        let service = p
            .service_dns
            .strip_suffix(&self.config.service_suffix)
            .ok_or(StatusCode::UNAUTHORIZED)?;
        if p.runner_id != self.config.runner_id
            || p.runner_generation != self.config.runner_generation
            || p.namespace != self.config.namespace
            || !dns_label(service)
            || p.preview_host != host
            || !self.config.allowed_ports.contains(&p.port)
            || p.issued_at > now
            || p.expires_at <= now
            || p.expires_at
                .checked_sub(p.issued_at)
                .is_none_or(|ttl| ttl <= 0 || ttl > self.config.max_ticket_ms)
            || [
                &p.preview_id,
                &p.project_id,
                &p.workspace_resource_id,
                &p.sandbox_uid,
                &p.pod_uid,
            ]
            .iter()
            .any(|v| v.is_empty() || v.len() > 128)
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(grant)
    }
    async fn live(&self, grant: &Signed<SandboxPreviewGrant>) -> Result<(), StatusCode> {
        let response = self
            .client
            .post(format!(
                "{}/v2/sandbox-previews/validate",
                self.config.runner_url.trim_end_matches('/')
            ))
            .bearer_auth(&self.validator_token)
            .json(grant)
            .send()
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        if response.status() != StatusCode::NO_CONTENT {
            return Err(StatusCode::UNAUTHORIZED);
        }
        if grant.payload.expires_at <= now() {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(())
    }
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
fn deny(status: StatusCode) -> Response {
    secure((status, "preview_access_unavailable").into_response(), None)
}
fn secure(mut response: Response, csp: Option<String>) -> Response {
    let headers = response.headers_mut();
    for (key, value) in [
        ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-resource-policy", "same-origin"),
    ] {
        headers.insert(key, HeaderValue::from_static(value));
    }
    let policy=csp.unwrap_or_else(||"sandbox allow-scripts allow-forms allow-same-origin; default-src 'self' data: blob:; script-src 'self' 'unsafe-inline' 'unsafe-eval' blob:; style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; form-action 'self'; base-uri 'none'; object-src 'none'; worker-src 'none'".into());
    headers.insert(
        "content-security-policy",
        HeaderValue::from_str(&policy).unwrap(),
    );
    response
}
async fn bootstrap(State(gateway): State<Arc<Gateway>>, headers: HeaderMap, uri: Uri) -> Response {
    if uri.query().is_some() {
        return deny(StatusCode::BAD_REQUEST);
    }
    if gateway.host(&headers).is_err() {
        return deny(StatusCode::UNAUTHORIZED);
    }
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>LocalAI Cowork preview</title><body>Opening preview…<script>{BOOTSTRAP_SCRIPT}</script></body>"
    );
    let hash = STANDARD.encode(Sha256::digest(BOOTSTRAP_SCRIPT.as_bytes()));
    let mut response = html.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    secure(
        response,
        Some(format!(
            "default-src 'none'; script-src 'sha256-{hash}'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"
        )),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    ticket: String,
}
async fn establish(State(gateway): State<Arc<Gateway>>, request: Request) -> Response {
    let host = match gateway.host(request.headers()) {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    if !same_origin(request.headers(), &host, true) {
        return deny(StatusCode::FORBIDDEN);
    }
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| !v.starts_with("application/json"))
    {
        return deny(StatusCode::BAD_REQUEST);
    }
    let body = match to_bytes(request.into_body(), 8192).await {
        Ok(v) => v,
        Err(_) => return deny(StatusCode::PAYLOAD_TOO_LARGE),
    };
    let input: Bootstrap = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    let grant = match gateway.ticket(&input.ticket, &host, now()) {
        Ok(v) => v,
        Err(status) => return deny(status),
    };
    if let Err(status) = gateway.live(&grant).await {
        return deny(status);
    }
    let seconds = ((grant.payload.expires_at - now()) / 1000).clamp(1, 60);
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{COOKIE}={}; Path=/; Max-Age={seconds}; HttpOnly; Secure; SameSite=Strict",
            input.ticket
        ))
        .unwrap(),
    );
    secure(response, None)
}
fn cookie(headers: &HeaderMap) -> Result<String, StatusCode> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE) {
        let raw = value.to_str().map_err(|_| StatusCode::UNAUTHORIZED)?;
        if raw.len() > 8192 {
            return Err(StatusCode::UNAUTHORIZED);
        }
        for item in raw.split(';') {
            if let Some(value) = item.trim().strip_prefix(&format!("{COOKIE}=")) {
                if found.is_some() {
                    return Err(StatusCode::UNAUTHORIZED);
                }
                found = Some(value.to_owned());
            }
        }
    }
    found.ok_or(StatusCode::UNAUTHORIZED)
}
fn same_origin(headers: &HeaderMap, host: &str, required: bool) -> bool {
    let expected = format!("https://{host}");
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if origin.is_some_and(|v| v != expected) || required && origin.is_none() {
        return false;
    }
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok())
        && !matches!(site, "same-origin" | "none")
    {
        return false;
    }
    true
}
fn redirect_location(
    raw: &str,
    upstream: &reqwest::Url,
    preview_host: &str,
) -> Result<String, StatusCode> {
    let location = upstream.join(raw).map_err(|_| StatusCode::BAD_GATEWAY)?;
    if location.origin() != upstream.origin()
        || !location.username().is_empty()
        || location.password().is_some()
    {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let mut target = format!("https://{preview_host}{}", location.path());
    if let Some(query) = location.query() {
        target.push('?');
        target.push_str(query)
    }
    if let Some(fragment) = location.fragment() {
        target.push('#');
        target.push_str(fragment)
    }
    Ok(target)
}
async fn proxy(
    State(gateway): State<Arc<Gateway>>,
    websocket: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    request: Request,
) -> Response {
    let host = match gateway.host(request.headers()) {
        Ok(v) => v,
        Err(s) => return deny(s),
    };
    if !same_origin(request.headers(), &host, false) {
        return deny(StatusCode::FORBIDDEN);
    }
    let raw = match cookie(request.headers()) {
        Ok(v) => v,
        Err(s) => return deny(s),
    };
    let grant = match gateway.ticket(&raw, &host, now()) {
        Ok(v) => v,
        Err(s) => return deny(s),
    };
    if request.uri().path().starts_with("/_cowork/") {
        return deny(StatusCode::NOT_FOUND);
    }
    if !matches!(
        request.method().as_str(),
        "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS"
    ) {
        return deny(StatusCode::METHOD_NOT_ALLOWED);
    }
    if request.headers().contains_key(header::UPGRADE) {
        if let Err(status) = gateway.live(&grant).await {
            return deny(status);
        }
        let upgrade = match websocket {
            Ok(v) => v,
            Err(_) => return deny(StatusCode::BAD_REQUEST),
        };
        return websocket_proxy(gateway, grant, request, upgrade).await;
    }
    let upstream = match reqwest::Url::parse(&format!(
        "http://{}:{}{}",
        grant.payload.service_dns,
        grant.payload.port,
        request
            .uri()
            .path_and_query()
            .map(|v| v.as_str())
            .unwrap_or("/")
    )) {
        Ok(v) => v,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    if upstream.host_str() != Some(grant.payload.service_dns.as_str())
        || upstream.port() != Some(grant.payload.port)
    {
        return deny(StatusCode::UNAUTHORIZED);
    }
    let mut outgoing = gateway
        .client
        .request(request.method().clone(), upstream.clone());
    // Deliberate allowlist: Cookie, Authorization, forwarding and proxy headers
    // from the browser can never reach the guest application.
    for name in [
        "accept",
        "accept-language",
        "content-type",
        "range",
        "if-none-match",
        "if-modified-since",
        "cache-control",
    ] {
        if let Some(value) = request.headers().get(name) {
            outgoing = outgoing.header(name, value);
        }
    }
    let body = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        to_bytes(request.into_body(), MAX_REQUEST),
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(_)) => return deny(StatusCode::PAYLOAD_TOO_LARGE),
        Err(_) => return deny(StatusCode::REQUEST_TIMEOUT),
    };
    if let Err(status) = gateway.live(&grant).await {
        return deny(status);
    }
    let response = match outgoing.body(body).send().await {
        Ok(v) => v,
        Err(_) => return deny(StatusCode::BAD_GATEWAY),
    };
    if response
        .content_length()
        .is_some_and(|v| v > MAX_RESPONSE as u64)
    {
        return deny(StatusCode::BAD_GATEWAY);
    }
    let status = response.status();
    let mut headers = HeaderMap::new();
    for name in [
        "content-type",
        "content-encoding",
        "etag",
        "last-modified",
        "accept-ranges",
        "content-range",
    ] {
        if let Some(value) = response.headers().get(name) {
            headers.insert(name, value.clone());
        }
    }
    if let Some(location) = response.headers().get(header::LOCATION) {
        let value = match location
            .to_str()
            .ok()
            .and_then(|v| redirect_location(v, &upstream, &host).ok())
        {
            Some(v) => v,
            None => return deny(StatusCode::BAD_GATEWAY),
        };
        headers.insert(header::LOCATION, HeaderValue::from_str(&value).unwrap());
    }
    let mut total = 0usize;
    let stream = response
        .bytes_stream()
        .map(move |chunk| -> std::io::Result<_> {
            let bytes = chunk.map_err(std::io::Error::other)?;
            total = total.saturating_add(bytes.len());
            if total > MAX_RESPONSE {
                return Err(std::io::Error::other("preview_response_limit"));
            }
            Ok(bytes)
        });
    let mut output = Response::new(Body::from_stream(stream));
    *output.status_mut() = status;
    *output.headers_mut() = headers;
    let csp = format!(
        "sandbox allow-scripts allow-forms allow-same-origin; default-src 'self' data: blob:; script-src 'self' 'unsafe-inline' 'unsafe-eval' blob:; style-src 'self' 'unsafe-inline'; connect-src 'self' wss://{host}; frame-ancestors 'none'; form-action 'self'; base-uri 'none'; object-src 'none'; worker-src 'none'"
    );
    secure(output, Some(csp))
}

async fn websocket_proxy(
    gateway: Arc<Gateway>,
    grant: Signed<SandboxPreviewGrant>,
    request: Request,
    mut upgrade: WebSocketUpgrade,
) -> Response {
    let target = format!(
        "ws://{}:{}{}",
        grant.payload.service_dns,
        grant.payload.port,
        request
            .uri()
            .path_and_query()
            .map(|v| v.as_str())
            .unwrap_or("/")
    );
    let mut outgoing = match target.into_client_request() {
        Ok(v) => v,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    let protocols = request
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let offered: Vec<String> = protocols
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect();
    if offered.len() > 8
        || offered.iter().any(|v| {
            v.len() > 128
                || !v
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        })
    {
        return deny(StatusCode::BAD_REQUEST);
    }
    if !offered.is_empty() {
        outgoing.headers_mut().insert(
            "sec-websocket-protocol",
            HeaderValue::from_str(&offered.join(", ")).unwrap(),
        );
    }
    // This request was built from the signed target. Browser cookies, bearer
    // headers and forwarding headers are never copied into the handshake.
    let address = (grant.payload.service_dns.as_str(), grant.payload.port);
    #[cfg(test)]
    let connect = async {
        if let Some(address) = gateway.test_upstream {
            tokio::net::TcpStream::connect(address).await
        } else {
            tokio::net::TcpStream::connect(address).await
        }
    };
    #[cfg(not(test))]
    let connect = tokio::net::TcpStream::connect(address);
    let stream = match tokio::time::timeout(std::time::Duration::from_secs(5), connect).await {
        Ok(Ok(v)) => v,
        _ => return deny(StatusCode::BAD_GATEWAY),
    };
    let limits = WebSocketConfig::default()
        .max_message_size(Some(MAX_REQUEST))
        .max_frame_size(Some(MAX_REQUEST))
        .max_write_buffer_size(MAX_REQUEST * 2);
    let (guest, response) = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio_tungstenite::client_async_with_config(outgoing, stream, Some(limits)),
    )
    .await
    {
        Ok(Ok(v)) => v,
        _ => return deny(StatusCode::BAD_GATEWAY),
    };
    if let Some(protocol) = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
    {
        if !offered.iter().any(|v| v == protocol) {
            return deny(StatusCode::BAD_GATEWAY);
        }
        upgrade = upgrade.protocols([protocol.to_owned()]);
    }
    if grant.payload.expires_at <= now() {
        return deny(StatusCode::UNAUTHORIZED);
    }
    let response = upgrade
        .max_message_size(MAX_REQUEST)
        .max_frame_size(MAX_REQUEST)
        .max_write_buffer_size(MAX_REQUEST * 2)
        .on_upgrade(move |browser| bridge(gateway, grant, browser, guest));
    secure(response, None)
}
async fn bridge(
    gateway: Arc<Gateway>,
    grant: Signed<SandboxPreviewGrant>,
    mut browser: WebSocket,
    mut guest: WebSocketStream<tokio::net::TcpStream>,
) {
    let duration =
        std::time::Duration::from_millis((grant.payload.expires_at - now()).max(0) as u64);
    let _=tokio::time::timeout(duration,async{
        let mut validation=tokio::time::interval(std::time::Duration::from_secs(2));
        loop{tokio::select!{
            message=browser.recv()=>{
                let message=match message{Some(Ok(v))=>v,_=>break};
                let message=match message{BrowserMessage::Text(v)=>GuestMessage::Text(v.to_string().into()),BrowserMessage::Binary(v)=>GuestMessage::Binary(v),BrowserMessage::Ping(v)=>GuestMessage::Ping(v),BrowserMessage::Pong(v)=>GuestMessage::Pong(v),BrowserMessage::Close(_)=>break};
                if !matches!(tokio::time::timeout(std::time::Duration::from_secs(5),guest.send(message)).await,Ok(Ok(()))){break}
            },
            message=guest.next()=>{
                let message=match message{Some(Ok(v))=>v,_=>break};
                let message=match message{GuestMessage::Text(v)=>BrowserMessage::Text(v.to_string().into()),GuestMessage::Binary(v)=>BrowserMessage::Binary(v),GuestMessage::Ping(v)=>BrowserMessage::Ping(v),GuestMessage::Pong(v)=>BrowserMessage::Pong(v),GuestMessage::Close(_)=>break,GuestMessage::Frame(_)=>continue};
                if !matches!(tokio::time::timeout(std::time::Duration::from_secs(5),browser.send(message)).await,Ok(Ok(()))){break}
            },
            _=validation.tick()=>{if gateway.live(&grant).await.is_err(){break}}
        }}
    }).await;
    // Dropping both sockets closes them even if a peer ignores close frames.
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
