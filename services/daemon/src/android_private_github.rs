//! Network-none Git guest and original actor-bound HTTPS broker. No host/Team `gh` adoption.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_storage::android_private_github_runtime::GithubAdmission;
use sha2::Digest;
use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use uuid::Uuid;

fn invalid() -> Error {
    Error::Forbidden
}
fn uncertain() -> Error {
    Error::UncertainEffect
}
struct ReceiptState {
    closed: bool,
    jobs: Vec<tokio::task::JoinHandle<()>>,
}
pub(super) struct Receipts {
    state: std::sync::Mutex<ReceiptState>,
    slots: Arc<Semaphore>,
    serial: Mutex<()>,
}
impl Default for Receipts {
    fn default() -> Self {
        Self {
            state: std::sync::Mutex::new(ReceiptState {
                closed: false,
                jobs: Vec::new(),
            }),
            slots: Arc::new(Semaphore::new(8)),
            serial: Mutex::new(()),
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
        state.jobs.retain(|j| !j.is_finished());
        let (tx, rx) = oneshot::channel();
        state.jobs.push(tokio::spawn(async move {
            let _slot = slot;
            let value = original.await;
            if value.is_err() {
                tracing::warn!("Original private GitHub receive unresolved; replay is forbidden");
            }
            let _ = tx.send(value);
        }));
        Ok(rx)
    }
    pub(super) async fn drain(&self) {
        let _serial = self.serial.lock().await;
        let jobs = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.closed = true;
            self.slots.close();
            std::mem::take(&mut state.jobs)
        };
        for job in jobs {
            let _ = job.await;
        }
    }
}
fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
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
        IpAddr::V6(v) => v.to_ipv4_mapped().map_or_else(
            || {
                let s = v.segments();
                s[0] & 0xe000 == 0x2000
                    && s[0] != 0x2002
                    && !(s[0] == 0x2001 && s[1] <= 0x1ff)
                    && !(s[0] == 0x2001 && s[1] == 0xdb8)
            },
            |v| public(IpAddr::V4(v)),
        ),
    }
}
struct Request {
    client: reqwest::Client,
    url: reqwest::Url,
    method: reqwest::Method,
    body: Zeroizing<Vec<u8>>,
    addresses: Vec<SocketAddr>,
    api: bool,
}
async fn prepare(permit: &GithubAdmission, wire: &Value) -> Result<Request> {
    cowork_storage::android_private_github_runtime::network_allowed(
        &permit.binding,
        &permit.action,
        wire,
    )?;
    let u =
        reqwest::Url::parse(wire["url"].as_str().ok_or_else(invalid)?).map_err(|_| invalid())?;
    if u.scheme() != "https"
        || u.port_or_known_default() != Some(443)
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
    {
        return Err(Error::Forbidden);
    }
    let body = Zeroizing::new(
        STANDARD
            .decode(wire["body_base64"].as_str().ok_or_else(invalid)?)
            .map_err(|_| invalid())?,
    );
    if body.len() > 8 * 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    let host = u.host_str().ok_or_else(invalid)?.to_owned();
    let addresses = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host.as_str(), 443)),
    )
    .await
    .map_err(|_| uncertain())?
    .map_err(|_| uncertain())?
    .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.len() > 64 || addresses.iter().any(|a| !public(a.ip())) {
        return Err(Error::Forbidden);
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .resolve_to_addrs(&host, &addresses)
        .build()
        .map_err(|_| invalid())?;
    Ok(Request {
        client,
        url: u,
        method: reqwest::Method::from_bytes(
            wire["method"].as_str().ok_or_else(invalid)?.as_bytes(),
        )
        .map_err(|_| invalid())?,
        body,
        addresses,
        api: wire["channel"] == "api",
    })
}
async fn fetch(request: Request, token: Zeroizing<String>) -> Result<Value> {
    let remote_effect = request.api && request.method != reqwest::Method::GET
        || request.url.path().ends_with("/git-receive-pack");
    let mut headers = reqwest::header::HeaderMap::new();
    let credentials = Zeroizing::new(format!("x-access-token:{}", token.as_str()));
    let auth = Zeroizing::new(if request.api {
        format!("Bearer {}", token.as_str())
    } else {
        format!("Basic {}", STANDARD.encode(credentials.as_bytes()))
    });
    let mut value = reqwest::header::HeaderValue::from_str(&auth).map_err(|_| invalid())?;
    value.set_sensitive(true);
    headers.insert(reqwest::header::AUTHORIZATION, value);
    headers.insert(
        reqwest::header::ACCEPT_ENCODING,
        reqwest::header::HeaderValue::from_static("identity"),
    );
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static("LocalAI-Cowork-V2-private-github"),
    );
    if request.api {
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "x-github-api-version",
            reqwest::header::HeaderValue::from_static("2022-11-28"),
        );
        if !request.body.is_empty() {
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("application/json"),
            );
        }
    } else if request.method == reqwest::Method::POST {
        let value = if request.url.path().ends_with("git-receive-pack") {
            "application/x-git-receive-pack-request"
        } else {
            "application/x-git-upload-pack-request"
        };
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static(value),
        );
    }
    let response = request
        .client
        .request(request.method, request.url.clone())
        .headers(headers)
        .body(request.body.to_vec())
        .send()
        .await;
    let mut response = match response {
        Ok(v) => v,
        Err(e) => {
            return Ok(
                json!({"format":1,"uncertain":true,"phase":"send","timeout":e.is_timeout(),"connection_error":e.is_connect()}),
            );
        }
    };
    let original_headers: Vec<Value> = response
        .headers()
        .iter()
        .map(|(k, v)| json!([k.as_str(), STANDARD.encode(v.as_bytes())]))
        .collect();
    let mut result = json!({"format":1,"status":response.status().as_u16(),"connected_peer":response.remote_addr(),"original_headers":original_headers});
    if response
        .remote_addr()
        .is_none_or(|a| !request.addresses.contains(&a))
        || response.status().as_u16() == 101
    {
        result["uncertain"] = json!(true);
        result["phase"] = json!("peer_or_upgrade");
        return Ok(result);
    }
    let mut body = Zeroizing::new(Vec::new());
    let mut failure = None;
    let mut observed = 0usize;
    loop {
        let part = match response.chunk().await {
            Ok(Some(v)) => v,
            Ok(None) => break,
            Err(_) => {
                failure = Some("receive_transport");
                break;
            }
        };
        observed = observed.saturating_add(part.len());
        let available = (8 * 1024 * 1024usize - body.len()).min(part.len());
        body.extend_from_slice(&part[..available]);
        if available != part.len() {
            failure = Some("receive_budget");
            break;
        }
    }
    result["body_base64"] = json!(STANDARD.encode(body.as_slice()));
    result["body_sha256"] = json!(format!("{:x}", sha2::Sha256::digest(body.as_slice())));
    result["observed_body_bytes"] = json!(observed);
    result["content_type"] = json!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
    );
    if let Some(phase) = failure {
        result["uncertain"] = json!(true);
        result["phase"] = json!(phase);
        result["body_prefix_only"] = json!(true)
    }
    if remote_effect && (response.status().is_server_error() || response.status().as_u16() == 408) {
        result["uncertain"] = json!(true);
        result["phase"] = json!("remote_effect_outcome");
    }
    // Redirects are received as original errors; no credential-bearing follow-up is issued.
    Ok(result)
}
// The existing codec permits 32 MiB of JSON and one framing newline.
const CONTROL_LIMIT: usize = 32 * 1024 * 1024 + 1;
#[derive(Default)]
struct OriginalControl {
    bytes: Zeroizing<Vec<u8>>,
    observed: u64,
    digest: sha2::Sha256,
    first_frame_complete: bool,
}
impl OriginalControl {
    fn append(&mut self, bytes: &[u8]) {
        self.observed = self.observed.saturating_add(bytes.len() as u64);
        self.digest.update(bytes);
        let keep = bytes
            .len()
            .min(CONTROL_LIMIT.saturating_sub(self.bytes.len()));
        self.bytes.extend_from_slice(&bytes[..keep]);
    }
    fn sha256(&self) -> String {
        format!("{:x}", self.digest.clone().finalize())
    }
}
async fn read_frame(
    read: &mut BufReader<tokio::process::ChildStdout>,
    original: &mut OriginalControl,
) -> Result<Value> {
    loop {
        let available = read.fill_buf().await.map_err(|_| uncertain())?;
        if available.is_empty() {
            return Err(uncertain());
        }
        let end = available.iter().position(|b| *b == b'\n');
        let count = end.map_or(available.len(), |n| n + 1);
        original.append(&available[..count]);
        read.consume(count);
        original.first_frame_complete = end.is_some();
        if original.observed > CONTROL_LIMIT as u64 {
            return Err(Error::BudgetExceeded);
        }
        if original.first_frame_complete {
            return serde_json::from_slice(&original.bytes).map_err(|_| uncertain());
        }
    }
}
async fn drain_control(
    read: &mut BufReader<tokio::process::ChildStdout>,
    original: &mut OriginalControl,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut chunk = [0u8; 8192];
    loop {
        let count = tokio::time::timeout_at(deadline, read.read(&mut chunk))
            .await
            .map_err(|_| uncertain())?
            .map_err(|_| uncertain())?;
        if count == 0 {
            return Ok(());
        }
        original.append(&chunk[..count]);
        if original.observed > CONTROL_LIMIT as u64 {
            return Err(Error::BudgetExceeded);
        }
    }
}
async fn write_frame(write: &mut tokio::process::ChildStdin, value: &Value) -> Result<()> {
    let raw = Zeroizing::new(serde_json::to_vec(value).map_err(|_| invalid())?);
    if raw.len() > 12 * 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    write.write_all(&raw).await.map_err(|_| uncertain())?;
    write.write_all(b"\n").await.map_err(|_| uncertain())?;
    write.flush().await.map_err(|_| uncertain())
}
impl AndroidDeviceServices {
    pub(super) async fn private_github_tool(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        stop: &CancellationToken,
    ) -> Result<bool> {
        use cowork_storage::android_private_mcp_runtime::PrivateMcpGate;
        match self
            .store
            .android_private_github_gate(lease.run, lease.lease, self.keys.as_ref(), self.key_id)
            .await?
        {
            PrivateMcpGate::Waiting(_) => return Ok(false),
            PrivateMcpGate::Denied(_) => return Ok(true),
            PrivateMcpGate::Ready => (),
        }
        let runner = self.private_runner.as_ref().ok_or_else(invalid)?;
        let permit = self
            .store
            .android_private_github_admit(lease.run, lease.lease, self.keys.as_ref(), self.key_id)
            .await?;
        let files = self
            .store
            .android_private_runner_files(lease.run, lease.lease, self.keys.as_ref())
            .await?;
        let bootstrap = json!({"format":1,"mode":"github","files":files,"session_id":permit.session,"action_sequence":permit.sequence,"action":permit.action,"binding":permit.binding,"git_files":permit.git_files});
        let mut container = self
            .private_mcp_wait(
                lease.run,
                lease.lease,
                None,
                stop,
                runner.start(lease.run, lease.lease, &permit.invocation, &bootstrap, stop),
            )
            .await?;
        let nonce = container.nonce;
        let attached = tokio::time::timeout(
            Duration::from_secs(30),
            self.store.android_private_github_attach(
                lease.run,
                lease.lease,
                permit.session,
                permit.sequence,
                nonce,
                self.keys.as_ref(),
            ),
        )
        .await
        .map_err(|_| uncertain())
        .and_then(|v| v);
        let mut write = container.stdin.take();
        let mut read = container.stdout.take().map(BufReader::new);
        // The observed frame belongs to this original invocation, outside every cancellable waiter.
        let mut control = OriginalControl::default();
        let mut outcome = match (attached, write.as_mut(), read.as_mut()) {
            (Ok(()), Some(write), Some(read)) => self.private_mcp_wait(lease.run,lease.lease,None,stop,async{
                self.store.android_private_github_validate(lease.run,lease.lease,permit.session,permit.sequence,nonce,self.keys.as_ref()).await?;
                write_frame(write,&json!({"kind":"go","nonce":nonce,"session_id":permit.session,"action_sequence":permit.sequence})).await?;
                self.private_github_exchange(lease,&permit,nonce,write,read,&mut control).await
            }).await,
            (Err(error), _, _) => Err(error),
            _ => Err(uncertain()),
        };
        let retained = if let Ok(received) = &outcome {
            tokio::time::timeout(
                Duration::from_secs(30),
                self.store.android_private_program_retain_received(
                    lease.run,
                    lease.lease,
                    permit.sequence,
                    &permit.request_sha256,
                    nonce,
                    received,
                    self.keys.as_ref(),
                    self.key_id,
                ),
            )
            .await
            .map_err(|_| uncertain())
            .and_then(|v| v)
        } else {
            Ok(())
        };
        let failed = outcome.is_err() || retained.is_err();
        if !failed {
            // The complete result already has its own original loop receipt.
            control = OriginalControl::default();
        }
        // Closing this pipe grants no further action; drain original bytes before positive retirement.
        drop(write);
        let drained = if let Some(read) = read.as_mut() {
            drain_control(read, &mut control).await
        } else {
            Err(uncertain())
        };
        if !failed && (drained.is_err() || !control.bytes.is_empty()) {
            outcome = Err(uncertain());
        }
        let observation = if !control.bytes.is_empty() {
            tokio::time::timeout(
                Duration::from_secs(30),
                self.store.android_private_github_control_received(
                    lease.run,
                    lease.lease,
                    permit.session,
                    permit.sequence,
                    nonce,
                    &permit.request_sha256,
                    &control.bytes,
                    control.observed,
                    &control.sha256(),
                    control.first_frame_complete,
                    self.keys.as_ref(),
                    self.key_id,
                ),
            )
            .await
            .map_err(|_| uncertain())
            .and_then(|v| v)
        } else {
            Ok(())
        };
        // Do not let a failed receive, authority lookup or SQL write skip original physical cleanup.
        let finished = container.finish().await;
        let witness = finished?;
        retained?;
        observation?;
        let received = outcome?;
        self.store
            .android_private_github_completed(
                lease.run,
                lease.lease,
                &permit,
                nonce,
                &received,
                &witness,
                self.keys.as_ref(),
                self.key_id,
            )
            .await?;
        Ok(true)
    }
    async fn private_github_exchange(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        permit: &GithubAdmission,
        nonce: Uuid,
        write: &mut tokio::process::ChildStdin,
        read: &mut BufReader<tokio::process::ChildStdout>,
        control: &mut OriginalControl,
    ) -> Result<Value> {
        let mut requests = BTreeSet::new();
        let mut total = 0usize;
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(90), read_frame(read, control))
                .await
                .map_err(|_| uncertain())??;
            if frame["nonce"] != nonce.to_string()
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
            let request = Uuid::parse_str(frame["request_id"].as_str().ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
            if !requests.insert(request) || requests.len() > 32 {
                return Err(Error::BudgetExceeded);
            }
            let wire = if frame["network"]["channel"] == "api" {
                if frame["network"].as_object().is_none_or(|v| v.len() != 2)
                    || !frame["network"]["preflight"].is_boolean()
                {
                    return Err(Error::Forbidden);
                }
                cowork_storage::android_private_github_runtime::api_request(
                    &permit.binding,
                    &permit.action,
                    frame["network"]["preflight"] == true,
                )?
            } else {
                frame["network"].clone()
            };
            let prepared = prepare(permit, &wire).await?;
            let slot = self.private_github_receipts.reserve()?;
            self.store
                .android_private_github_network_admit(
                    lease.run,
                    lease.lease,
                    permit.session,
                    permit.sequence,
                    nonce,
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
            let session = permit.session;
            let sequence = permit.sequence;
            let original = wire.clone();
            let receiver = self.private_github_receipts.observe(slot, async move {
                let validation = tokio::time::timeout(
                    Duration::from_secs(10),
                    store.android_private_github_network_validate(
                        run,
                        lease_id,
                        session,
                        sequence,
                        nonce,
                        request,
                        &original,
                        keys.as_ref(),
                    ),
                )
                .await;
                let response = match validation {
                    Ok(Ok(token)) => fetch(prepared, token).await?,
                    _ => json!({"format":1,"not_sent":true,"phase":"original_dispatch_rejected"}),
                };
                tokio::time::timeout(
                    Duration::from_secs(30),
                    store.android_private_github_network_received(
                        run,
                        lease_id,
                        session,
                        sequence,
                        nonce,
                        request,
                        &original,
                        &response,
                        keys.as_ref(),
                        key_id,
                    ),
                )
                .await
                .map_err(|_| uncertain())??;
                Ok(response)
            })?;
            let response = receiver.await.map_err(|_| uncertain())??;
            total =
                total.saturating_add(serde_json::to_vec(&response).map_err(|_| invalid())?.len());
            if total > 64 * 1024 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            if response["not_sent"] == true {
                return Err(Error::Forbidden);
            }
            if response["uncertain"] == true {
                return Err(Error::UncertainEffect);
            }
            self.store
                .android_private_github_validate(
                    lease.run,
                    lease.lease,
                    permit.session,
                    permit.sequence,
                    nonce,
                    self.keys.as_ref(),
                )
                .await?;
            write_frame(write,&json!({"kind":"network","nonce":nonce,"session_id":session,"action_sequence":sequence,"request_id":request,"response":response})).await?;
            // Only discard a frame after its original request/response is durably retained and relayed.
            *control = OriginalControl::default();
        }
    }
}
