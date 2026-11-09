//! Private Chromium's original pipe is its only network broker. No team/owner browser resources.
use super::private_runner::Container;
use super::*;
use cowork_storage::android_private_browser::{BrowserAdmission, url};
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use uuid::Uuid;

struct ReceiptState {
    closed: bool,
    jobs: Vec<tokio::task::JoinHandle<()>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real partial HTTP receive; this fixture deliberately bypasses public-DNS preparation.
    // It qualifies observer lifetime/one original send, not actor SQL authority or a Chromium guest.
    #[tokio::test]
    async fn dropped_parent_keeps_original_partial_receive_without_resending() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (started, receiving) = oneshot::channel();
        let (finish, finished) = oneshot::channel();
        let sends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = sends.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                assert_eq!(stream.read(&mut b).await.unwrap(), 1);
                headers.push(b[0]);
                assert!(headers.len() < 8192);
            }
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabc")
                .await
                .unwrap();
            stream.flush().await.unwrap();
            let _ = started.send(());
            finished.await.unwrap();
            // Deliberate EOF: the original body is partial, so its outcome remains uncertain.
        });
        let request = Request {
            client: reqwest::Client::builder()
                .no_proxy()
                .retry(reqwest::retry::never())
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
            url: reqwest::Url::parse(&format!("http://{address}/original")).unwrap(),
            method: reqwest::Method::GET,
            headers: reqwest::header::HeaderMap::new(),
            body: Zeroizing::new(Vec::new()),
            addresses: vec![address],
        };
        let receipts = Receipts::default();
        let retained = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let original = retained.clone();
        let waiter = receipts
            .observe(receipts.reserve().unwrap(), async move {
                let received = fetch(request).await?;
                original.lock().unwrap().push(received.clone());
                Ok(received)
            })
            .unwrap();
        receiving.await.unwrap();
        drop(waiter);
        finish.send(()).unwrap();
        receipts.drain().await;
        server.await.unwrap();
        let retained = retained.lock().unwrap();
        assert_eq!(sends.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0]["uncertain"], true);
        assert_eq!(retained[0]["body_prefix_only"], true);
        assert_eq!(retained[0]["body_base64"], STANDARD.encode(b"abc"));
        assert_eq!(retained[0]["observed_body_bytes"], 3);
        assert!(receipts.reserve().is_err());
    }
}
/// These original receive jobs belong to the service, never to the awaiting browser future.
pub(super) struct Receipts {
    state: std::sync::Mutex<ReceiptState>,
    slots: Arc<Semaphore>,
    drain_serial: Mutex<()>,
}
impl Default for Receipts {
    fn default() -> Self {
        Self {
            state: std::sync::Mutex::new(ReceiptState {
                closed: false,
                jobs: Vec::new(),
            }),
            slots: Arc::new(Semaphore::new(8)),
            drain_serial: Mutex::new(()),
        }
    }
}
impl Receipts {
    fn reserve(&self) -> Result<OwnedSemaphorePermit> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return Err(Error::Forbidden);
        }
        self.slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::BudgetExceeded)
    }
    fn observe(
        &self,
        slot: OwnedSemaphorePermit,
        original: impl std::future::Future<Output = Result<Value>> + Send + 'static,
    ) -> Result<oneshot::Receiver<Result<Value>>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return Err(Error::Forbidden);
        }
        state.jobs.retain(|job| !job.is_finished());
        let (sender, receiver) = oneshot::channel();
        state.jobs.push(tokio::spawn(async move {
            let _slot = slot;
            let received = original.await;
            if received.is_err() {
                tracing::warn!(
                    "Original private browser receive remains unresolved; no replay is permitted"
                );
            }
            // A dropped waiter cannot drop the original request or its persisted receive.
            let _ = sender.send(received);
        }));
        Ok(receiver)
    }
    pub(super) async fn drain(&self) {
        let _serial = self.drain_serial.lock().await;
        let jobs = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.closed = true;
            self.slots.close();
            std::mem::take(&mut state.jobs)
        };
        for job in jobs {
            if job.await.is_err() {
                tracing::warn!(
                    "Original private browser receive observer ended without a confirmed receipt"
                );
            }
        }
    }
}

pub(super) struct BrowserSession {
    container: Container,
    write: tokio::process::ChildStdin,
    read: BufReader<tokio::process::ChildStdout>,
}
fn uncertain() -> Error {
    Error::UncertainEffect
}
fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || a == 169 && b == 254
                || a == 172 && (16..=31).contains(&b)
                || a == 192 && (b == 168 || b == 0 || b == 2 || b == 88 && c == 99)
                || a == 100 && (64..=127).contains(&b)
                || a == 198 && (b == 18 || b == 19 || b == 51 && c == 100)
                || a == 203 && b == 0 && c == 113)
        }
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or_else(
            || {
                let s = ip.segments();
                s[0] & 0xe000 == 0x2000
                    && s[0] != 0x2002
                    && !(s[0] == 0x2001 && s[1] <= 0x1ff)
                    && !(s[0] == 0x2001 && s[1] == 0xdb8)
            },
            |v| public(IpAddr::V4(v)),
        ),
    }
}
async fn read_line(read: &mut BufReader<tokio::process::ChildStdout>) -> Result<Value> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0];
        if read.read(&mut byte).await.map_err(|_| uncertain())? == 0 {
            return Err(uncertain());
        }
        if byte[0] == b'\n' {
            break;
        }
        if bytes.len() >= 24 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        bytes.push(byte[0]);
    }
    serde_json::from_slice(&bytes).map_err(|_| uncertain())
}
async fn write_frame(write: &mut tokio::process::ChildStdin, value: &Value) -> Result<()> {
    let raw = Zeroizing::new(serde_json::to_vec(value).map_err(|_| invalid())?);
    if raw.len() > 6 * 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    write.write_all(&raw).await.map_err(|_| uncertain())?;
    write.write_all(b"\n").await.map_err(|_| uncertain())?;
    write.flush().await.map_err(|_| uncertain())
}
struct Request {
    client: reqwest::Client,
    url: reqwest::Url,
    method: reqwest::Method,
    headers: reqwest::header::HeaderMap,
    body: Zeroizing<Vec<u8>>,
    addresses: Vec<SocketAddr>,
}
async fn prepare(action: &Value, network: &Value) -> Result<Request> {
    let u = url(network["url"].as_str().ok_or_else(invalid)?)?;
    if !action["allowed_origins"]
        .as_array()
        .is_some_and(|v| v.contains(&json!(u.origin().ascii_serialization())))
        || !action["allowed_methods"]
            .as_array()
            .is_some_and(|v| v.contains(&network["method"]))
    {
        return Err(Error::Forbidden);
    }
    let method =
        reqwest::Method::from_bytes(network["method"].as_str().ok_or_else(invalid)?.as_bytes())
            .map_err(|_| invalid())?;
    let values = network["headers"]
        .as_object()
        .filter(|v| v.len() <= 64)
        .ok_or_else(invalid)?;
    let mut headers = reqwest::header::HeaderMap::new();
    let mut size = 0usize;
    for (name, value) in values {
        let name =
            reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid())?;
        let value = value.as_str().ok_or_else(invalid)?;
        size = size.saturating_add(name.as_str().len() + value.len());
        if size > 32768 {
            return Err(Error::BudgetExceeded);
        }
        if matches!(
            name.as_str(),
            "authorization"
                | "proxy-authorization"
                | "proxy-connection"
                | "upgrade"
                | "te"
                | "trailer"
        ) {
            return Err(Error::Forbidden);
        }
        if name == reqwest::header::COOKIE && u.scheme() != "https" {
            return Err(Error::Forbidden);
        }
        if matches!(name.as_str(), "origin" | "referer") {
            let origin = url(value)?.origin().ascii_serialization();
            if !action["allowed_origins"]
                .as_array()
                .is_some_and(|v| v.contains(&json!(origin)))
            {
                return Err(Error::Forbidden);
            }
        }
        // Chromium framing/security headers cannot select a host, proxy or transport.
        if matches!(
            name.as_str(),
            "cookie"
                | "accept"
                | "accept-language"
                | "content-type"
                | "origin"
                | "referer"
                | "user-agent"
                | "cache-control"
                | "pragma"
                | "if-none-match"
                | "if-modified-since"
        ) {
            let mut v = reqwest::header::HeaderValue::from_str(value).map_err(|_| invalid())?;
            v.set_sensitive(name == reqwest::header::COOKIE);
            headers.insert(name, v);
        }
    }
    // Request identity encoding: fulfill transports the original response bytes once.
    headers.insert(
        reqwest::header::ACCEPT_ENCODING,
        reqwest::header::HeaderValue::from_static("identity"),
    );
    let encoded = network["body_base64"]
        .as_str()
        .filter(|s| s.len() <= 700000)
        .ok_or_else(invalid)?;
    let body = Zeroizing::new(STANDARD.decode(encoded).map_err(|_| invalid())?);
    if body.len() > 512 * 1024
        || matches!(method, reqwest::Method::GET | reqwest::Method::HEAD) && !body.is_empty()
    {
        return Err(Error::Forbidden);
    }
    let host = u.host_str().ok_or_else(invalid)?.to_owned();
    let port = u.port_or_known_default().ok_or_else(invalid)?;
    let addresses = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await
    .map_err(|_| uncertain())?
    .map_err(|_| uncertain())?
    .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.len() > 64 || addresses.iter().any(|v| !public(v.ip())) {
        return Err(Error::Forbidden);
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .resolve_to_addrs(&host, &addresses)
        .build()
        .map_err(|_| invalid())?;
    Ok(Request {
        client,
        url: u,
        method,
        headers,
        body,
        addresses,
    })
}
async fn fetch(request: Request) -> Result<Value> {
    let response = request
        .client
        .request(request.method, request.url.clone())
        .headers(request.headers)
        .body(request.body.to_vec())
        .send()
        .await;
    let mut response = match response {
        Ok(v) => v,
        Err(e) => {
            return Ok(
                json!({"format":1,"url":request.url.as_str(),"uncertain":true,"phase":"send","timeout":e.is_timeout(),"connection_error":e.is_connect()}),
            );
        }
    };
    let peer = response.remote_addr();
    let original_headers: Vec<Value> = response
        .headers()
        .iter()
        .map(|(k, v)| json!([k.as_str(), STANDARD.encode(v.as_bytes())]))
        .collect();
    let mut evidence = json!({"format":1,"url":request.url.as_str(),"status":response.status().as_u16(),"connected_peer":peer,"original_headers":original_headers});
    if response
        .remote_addr()
        .is_none_or(|p| !request.addresses.contains(&p))
        || response.status().as_u16() == 101
    {
        evidence["uncertain"] = json!(true);
        evidence["phase"] = json!("peer_or_upgrade");
        return Ok(evidence);
    }
    let status = response.status().as_u16();
    let mut headers = BTreeMap::new();
    let mut header_size = 0usize;
    for (name, value) in response.headers() {
        if matches!(
            name.as_str(),
            "connection"
                | "transfer-encoding"
                | "content-length"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "upgrade"
                | "alt-svc"
                | "set-cookie"
        ) {
            continue;
        }
        let Ok(v) = value.to_str() else {
            evidence["uncertain"] = json!(true);
            evidence["phase"] = json!("header_encoding");
            return Ok(evidence);
        };
        header_size = header_size.saturating_add(name.as_str().len() + v.len());
        if header_size > 32768 {
            evidence["uncertain"] = json!(true);
            evidence["phase"] = json!("header_budget");
            return Ok(evidence);
        }
        headers.insert(name.as_str().to_owned(), v.to_owned());
    }
    // Original Set-Cookie remains scoped to this response URL inside Chromium, never a daemon jar.
    let cookies = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().map(str::to_owned).map_err(|_| uncertain()))
        .collect::<Result<Vec<_>>>();
    let cookies = match cookies {
        Ok(v) => v,
        Err(_) => {
            evidence["uncertain"] = json!(true);
            evidence["phase"] = json!("cookie_encoding");
            return Ok(evidence);
        }
    };
    if cookies.iter().map(String::len).sum::<usize>() > 32768 {
        evidence["uncertain"] = json!(true);
        evidence["phase"] = json!("cookie_budget");
        return Ok(evidence);
    }
    if response
        .content_length()
        .is_some_and(|n| n > 4 * 1024 * 1024)
    {
        evidence["uncertain"] = json!(true);
        evidence["phase"] = json!("body_length_budget");
        return Ok(evidence);
    }
    let mut body = Zeroizing::new(Vec::new());
    let mut failure = None;
    let mut observed_bytes = 0usize;
    loop {
        let part = match response.chunk().await {
            Ok(Some(v)) => v,
            Ok(None) => break,
            Err(_) => {
                failure = Some("body_transport");
                break;
            }
        };
        observed_bytes = observed_bytes.saturating_add(part.len());
        if body.len().saturating_add(part.len()) > 4 * 1024 * 1024 {
            let available = (4 * 1024 * 1024 - body.len()).min(part.len());
            body.extend_from_slice(&part[..available]);
            failure = Some("body_budget");
            break;
        }
        body.extend_from_slice(&part);
    }
    evidence["status"] = json!(status);
    evidence["headers"] = json!(headers);
    evidence["set_cookie"] = json!(cookies);
    evidence["body_base64"] = json!(STANDARD.encode(body.as_slice()));
    evidence["body_sha256"] = json!(format!("{:x}", sha2::Sha256::digest(body.as_slice())));
    evidence["observed_body_bytes"] = json!(observed_bytes);
    if let Some(phase) = failure {
        evidence["uncertain"] = json!(true);
        evidence["phase"] = json!(phase);
        evidence["body_prefix_only"] = json!(true);
    }
    Ok(evidence)
}
impl AndroidDeviceServices {
    pub(super) async fn private_browser_tool(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        stop: &CancellationToken,
        sessions: &mut BTreeMap<Uuid, BrowserSession>,
    ) -> Result<bool> {
        use cowork_storage::android_private_mcp_runtime::PrivateMcpGate;
        match self
            .store
            .android_private_browser_gate(lease.run, lease.lease, self.keys.as_ref(), self.key_id)
            .await?
        {
            PrivateMcpGate::Waiting(_) => return Ok(false),
            PrivateMcpGate::Denied(_) => return Ok(true),
            PrivateMcpGate::Ready => {}
        }
        let runner = self.private_runner.as_ref().ok_or(Error::Forbidden)?;
        let permit = self
            .store
            .android_private_browser_admit(lease.run, lease.lease, self.keys.as_ref(), self.key_id)
            .await?;
        if permit.new_session {
            let files = self
                .store
                .android_private_runner_files(lease.run, lease.lease, self.keys.as_ref())
                .await?;
            let bootstrap =
                json!({"format":1,"mode":"browser","files":files,"session_id":permit.session});
            let mut container = self
                .private_mcp_wait(
                    lease.run,
                    lease.lease,
                    None,
                    stop,
                    runner.start(lease.run, lease.lease, &permit.invocation, &bootstrap, stop),
                )
                .await?;
            self.store
                .android_private_browser_attach(
                    lease.run,
                    lease.lease,
                    permit.session,
                    permit.sequence,
                    container.nonce,
                    self.keys.as_ref(),
                )
                .await?;
            let write = container.stdin.take().ok_or_else(uncertain)?;
            let read = BufReader::new(container.stdout.take().ok_or_else(uncertain)?);
            sessions.insert(
                permit.session,
                BrowserSession {
                    container,
                    write,
                    read,
                },
            );
        }
        let session = sessions.get_mut(&permit.session).ok_or_else(uncertain)?;
        let received = self
            .private_mcp_wait(
                lease.run,
                lease.lease,
                None,
                stop,
                self.private_browser_action(lease, session, &permit),
            )
            .await?;
        self.store
            .android_private_program_retain_received(
                lease.run,
                lease.lease,
                permit.sequence,
                &permit.request_sha256,
                session.container.nonce,
                &received,
                self.keys.as_ref(),
                self.key_id,
            )
            .await?;
        self.store
            .android_private_browser_applied(
                lease.run,
                lease.lease,
                permit.session,
                permit.sequence,
                session.container.nonce,
                &received,
                self.keys.as_ref(),
                self.key_id,
            )
            .await?;
        // A deterministic read starts an isolated short-lived context. Close
        // that original guest before the next node can reuse browser capacity.
        if permit.action["operation"] == "close"
            || (permit.new_session
                && matches!(
                    permit.action["operation"].as_str(),
                    Some("fetch_text" | "search")
                ))
        {
            let mut session = sessions.remove(&permit.session).ok_or_else(uncertain)?;
            let witness = session.container.finish().await?;
            self.store
                .android_private_browser_closed(
                    lease.run,
                    lease.lease,
                    permit.session,
                    session.container.nonce,
                    &witness,
                    self.keys.as_ref(),
                    self.key_id,
                )
                .await?;
        }
        Ok(true)
    }
    async fn private_browser_action(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        session: &mut BrowserSession,
        permit: &BrowserAdmission,
    ) -> Result<Value> {
        self.store
            .android_private_browser_validate(
                lease.run,
                lease.lease,
                permit.session,
                permit.sequence,
                session.container.nonce,
                self.keys.as_ref(),
            )
            .await?;
        write_frame(&mut session.write,&json!({"kind":"action","nonce":session.container.nonce,"session_id":permit.session,"sequence":permit.sequence,"action":permit.action})).await?;
        let mut requests = BTreeSet::new();
        let mut response_bytes = 0usize;
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(75), read_line(&mut session.read))
                .await
                .map_err(|_| uncertain())??;
            if frame["nonce"] != session.container.nonce.to_string()
                || frame["session_id"] != permit.session.to_string()
                || frame["action_sequence"] != permit.sequence
            {
                return Err(uncertain());
            }
            if frame["kind"] == "result" {
                return Ok(frame);
            }
            if frame["kind"] != "network" {
                return Err(uncertain());
            }
            let request = Uuid::parse_str(frame["request_id"].as_str().ok_or_else(uncertain)?)
                .map_err(|_| uncertain())?;
            if !requests.insert(request) || requests.len() > 512 {
                return Err(Error::BudgetExceeded);
            }
            let prepared = match prepare(&permit.action, &frame["network"]).await {
                Ok(p) => p,
                Err(Error::Forbidden | Error::Validation(_) | Error::BudgetExceeded) => {
                    self.store
                        .android_private_browser_validate(
                            lease.run,
                            lease.lease,
                            permit.session,
                            permit.sequence,
                            session.container.nonce,
                            self.keys.as_ref(),
                        )
                        .await?;
                    write_frame(&mut session.write,&json!({"kind":"network","nonce":session.container.nonce,"sequence":permit.sequence,"request_id":request,"denied":true})).await?;
                    continue;
                }
                Err(e) => return Err(e),
            };
            let wire = json!({"url":prepared.url.as_str(),"method":prepared.method.as_str(),"headers":prepared.headers.iter().map(|(k,v)|(k.as_str().to_owned(),v.to_str().unwrap_or("").to_owned())).collect::<BTreeMap<_,_>>(),"body_base64":STANDARD.encode(prepared.body.as_slice()),"addresses":prepared.addresses});
            let slot = self.private_browser_receipts.reserve()?;
            self.store
                .android_private_browser_network_admit(
                    lease.run,
                    lease.lease,
                    permit.session,
                    permit.sequence,
                    session.container.nonce,
                    request,
                    &wire,
                    self.keys.as_ref(),
                    self.key_id,
                )
                .await?;
            let store = self.store.clone();
            let keys = self.keys.clone();
            let key_id = self.key_id;
            let run = lease.run;
            let lease_id = lease.lease;
            let session_id = permit.session;
            let sequence = permit.sequence;
            let nonce = session.container.nonce;
            let receiver = self.private_browser_receipts.observe(slot, async move {
                // This only validates the existing original admission; it cannot submit another request.
                let validation = tokio::time::timeout(Duration::from_secs(10),
                    store.android_private_browser_network_validate(run, lease_id, session_id, sequence, nonce, request, &wire, keys.as_ref())
                ).await;
                let response = match validation {
                    Ok(Ok(())) => fetch(prepared).await?,
                    Ok(Err(_)) => json!({"format":1,"not_sent":true,"phase":"original_dispatch_rejected"}),
                    Err(_) => json!({"format":1,"not_sent":true,"phase":"original_dispatch_validation_timeout"}),
                };
                // Exactly one bounded receipt write, also when the parent has stopped or lost authority.
                tokio::time::timeout(Duration::from_secs(30),
                    store.android_private_browser_network_received(run, lease_id, session_id, sequence, nonce, request, &wire, &response, keys.as_ref(), key_id)
                ).await.map_err(|_| uncertain())??;
                Ok(response)
            })?;
            let response = receiver.await.map_err(|_| uncertain())??;
            response_bytes = response_bytes
                .saturating_add(serde_json::to_vec(&response).map_err(|_| invalid())?.len());
            if response["not_sent"] == true {
                return Err(Error::Forbidden);
            }
            if response["uncertain"] == true {
                return Err(Error::UncertainEffect);
            }
            if response_bytes > 32 * 1024 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            self.store
                .android_private_browser_validate(
                    lease.run,
                    lease.lease,
                    permit.session,
                    permit.sequence,
                    session.container.nonce,
                    self.keys.as_ref(),
                )
                .await?;
            write_frame(&mut session.write,&json!({"kind":"network","nonce":session.container.nonce,"sequence":permit.sequence,"request_id":request,"response":response})).await?;
        }
    }
    pub(super) async fn private_browser_close(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        sessions: &mut BTreeMap<Uuid, BrowserSession>,
    ) -> Result<()> {
        for (id, mut session) in std::mem::take(sessions) {
            let witness = session.container.finish().await?;
            self.store
                .android_private_browser_closed(
                    lease.run,
                    lease.lease,
                    id,
                    session.container.nonce,
                    &witness,
                    self.keys.as_ref(),
                    self.key_id,
                )
                .await?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod pipeline_broker_tests {
    use super::*;
    #[tokio::test]
    async fn actual_http_fixture_retains_exact_bytes_and_original_peer_once() {
        // This injected test transport is deliberately not a production URL
        // grant. The public-host policy remains checked by prepare()/url().
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let expected =
            b"<title>Actor fixture</title><body>Actual retained pipeline evidence</body>";
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let size = stream.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..size]).starts_with("GET /evidence "));
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                expected.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(expected).await.unwrap();
            drop(stream);
            assert!(
                tokio::time::timeout(Duration::from_millis(200), listener.accept())
                    .await
                    .is_err(),
                "No implicit retry or duplicate network request"
            );
        });
        let response = fetch(Request {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            url: reqwest::Url::parse(&format!("http://{address}/evidence")).unwrap(),
            method: reqwest::Method::GET,
            headers: Default::default(),
            body: Zeroizing::new(vec![]),
            addresses: vec![address],
        })
        .await
        .unwrap();
        assert_eq!(response["status"], 200);
        assert_eq!(
            STANDARD
                .decode(response["body_base64"].as_str().unwrap())
                .unwrap(),
            expected
        );
        assert_ne!(response["uncertain"], true);
        assert_eq!(response["connected_peer"], json!(address));
        server.await.unwrap();
        assert!(prepare(&json!({"allowed_origins":[format!("http://{address}")],"allowed_methods":["GET"]}),&json!({"url":format!("http://{address}/evidence"),"method":"GET","headers":{},"body_base64":""})).await.is_err(),"Production actor policy never allows this fixture loopback");
    }
}
