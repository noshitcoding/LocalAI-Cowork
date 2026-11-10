//! Actual authenticated TLS with a public test-only CA. The peer is synthetic:
//! no native backend, qualification, guest or physical termination is inferred.
use super::*;
use axum::{
    Router,
    body::Body,
    extract::State,
    response::{IntoResponse, Response},
};
use cowork_runner_protocol::ManagedInputTicket;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tower::ServiceExt;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Success,
    WrongReady,
    WrongTicket,
    WrongMissing,
    WrongAck,
    LostBeginReply,
    LostStartReply,
    HangStart,
    RevokeSource,
    WrongReceipt,
    ResultForeignReceipt,
    ResultForeignInstance,
    ResultInvalidSignature,
    ResultExpiredAttestation,
    ResultCorruptChunk,
    ResultOversizedChunk,
    ResultWrongType,
    ResultUnavailable,
    ResultExpired,
    ResultRetired,
    ResultLostReply,
    ContextWrongNonce,
    ContextWrongSigner,
    ContextWrongReceipt,
    ContextExpired,
    ContextLostReply,
    ContextUnsupported,
    DeliveryUnauthorized,
    DeliveryForbidden,
    DeliveryUnknownConflict,
    DeliveryExpired,
    DeliveryExtraError,
    DeliveryMalformedError,
    DeliveryOversizedStream,
    DeliveryWrongSigner,
    DeliveryWrongRequest,
    DeliveryLostReply,
    DeliveryHistoryMissing,
    RetirementWrongDelivery,
}

struct Peer {
    fixture: Fixture,
    mode: Mode,
    token: String,
    ticket: Mutex<Option<Signed<ManagedInputTicket>>>,
    begins: AtomicUsize,
    starts: AtomicUsize,
    chunks: AtomicUsize,
    polls: AtomicUsize,
    source_reads: AtomicUsize,
    revoked: AtomicBool,
    start_called: tokio::sync::Notify,
    result_reads: AtomicUsize,
    result_contexts: AtomicUsize,
    deliveries: AtomicUsize,
    delivery_history_reads: AtomicUsize,
    retirements: AtomicUsize,
    accepted_delivery: Mutex<Option<Signed<cowork_runner_protocol::ManagedResultDeliveryReceipt>>>,
}

struct Server {
    peer: Arc<Peer>,
    access: Arc<HttpsRunner>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Source(Arc<Peer>);
#[async_trait::async_trait]
impl cowork_runtime::ManagedInputAccess for Source {
    async fn manifest(&self) -> Result<ManagedInputManifest> {
        if self.0.revoked.load(Ordering::SeqCst) {
            return Err(Error::Forbidden);
        }
        Ok(self.0.fixture.source.clone())
    }
    async fn chunk(&self, input: ReadWorkspaceTransferChunk) -> Result<cowork_broker::BinaryFile> {
        self.0.source_reads.fetch_add(1, Ordering::SeqCst);
        if self.0.revoked.load(Ordering::SeqCst) {
            return Err(Error::Forbidden);
        }
        validate_chunk(&self.0.fixture.source.manifest, &input, b"input")?;
        Ok(cowork_broker::BinaryFile {
            metadata: cowork_contracts::DownloadMetadata {
                file_name: "input.txt".into(),
                size: 5,
                sha256: hash(b"input"),
            },
            bytes: b"input".to_vec(),
        })
    }
}

async fn server(mode: Mode) -> Server {
    let fixture = fixture();
    let peer = Arc::new(Peer {
        fixture: fixture.clone(),
        mode,
        token: "test-only-transport-credential-abcdefghijklmnopqrstuvwxyz".into(),
        ticket: Mutex::new(None),
        begins: AtomicUsize::new(0),
        starts: AtomicUsize::new(0),
        chunks: AtomicUsize::new(0),
        polls: AtomicUsize::new(0),
        source_reads: AtomicUsize::new(0),
        revoked: AtomicBool::new(false),
        start_called: tokio::sync::Notify::new(),
        result_reads: AtomicUsize::new(0),
        result_contexts: AtomicUsize::new(0),
        deliveries: AtomicUsize::new(0),
        delivery_history_reads: AtomicUsize::new(0),
        retirements: AtomicUsize::new(0),
        accepted_delivery: Mutex::new(None),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![rustls::pki_types::CertificateDer::from(
            include_bytes!("../test-tls/server.der").to_vec(),
        )],
        rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
            include_bytes!("../test-tls/server-key.der").to_vec(),
        )),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    let router = Router::new().fallback(handle).with_state(peer.clone());
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted=listener.accept()=> {
                    let Ok((stream,_))=accepted else { break; };
                    let acceptor=acceptor.clone();let router=router.clone();
                    connections.spawn(async move {
                        let Ok(tls)=acceptor.accept(stream).await else { return; };
                        let _=hyper::server::conn::http1::Builder::new().serve_connection(hyper_util::rt::TokioIo::new(tls),
                            hyper::service::service_fn(move|request|router.clone().oneshot(request))).await;
                    });
                },
                _=connections.join_next(),if !connections.is_empty()=>{},
            }
        }
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .tls_built_in_root_certs(false)
        .add_root_certificate(
            reqwest::Certificate::from_pem(include_bytes!("../test-tls/root-ca.pem")).unwrap(),
        )
        .build()
        .unwrap();
    let access = Arc::new(HttpsRunner {
        client,
        origin: reqwest::Url::parse(&format!("https://{address}/")).unwrap(),
        token: peer.token.clone(),
        binding: fixture.binding,
    });
    Server { peer, access, task }
}

fn reply<T: Serialize>(status: reqwest::StatusCode, value: T) -> Response {
    (status, axum::Json(value)).into_response()
}
fn lost_reply() -> Response {
    // Headers may arrive; the original body is lost. The client must never
    // infer nonstart or issue a second POST because of this read failure.
    Response::new(Body::from_stream(futures_util::stream::once(async {
        Err::<Vec<u8>, _>(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "synthetic lost reply",
        ))
    })))
}
fn receipt(peer: &Peer, completed: bool) -> Signed<Receipt> {
    let mut request = peer.fixture.request.payload.clone();
    if peer.mode == Mode::WrongReceipt {
        request.process_attempt_id.push_str("-foreign");
    }
    let state = if completed {
        ExecutionState::Completed {
            result: cowork_contracts::ProcessResult {
                termination: cowork_contracts::ProcessTermination::Exited,
                process_id: Some(1),
                exit_code: Some(0),
                signal: None,
                stdout: cowork_contracts::ProcessCapture {
                    bytes: vec![],
                    sha256: hash(b""),
                    total_bytes: 0,
                    truncated: false,
                },
                stderr: cowork_contracts::ProcessCapture {
                    bytes: vec![],
                    sha256: hash(b""),
                    total_bytes: 0,
                    truncated: false,
                },
                duration_ms: 1,
                output_complete: true,
            },
        }
    } else {
        ExecutionState::Admitted
    };
    peer.fixture
        .runner
        .sign(
            "receipt",
            Receipt {
                request,
                request_envelope_sha256: envelope_hash(&peer.fixture.request),
                state,
                revision: if completed { 3 } else { 1 },
                updated_at: now().unwrap(),
            },
        )
        .unwrap()
}

async fn handle(State(peer): State<Arc<Peer>>, request: axum::extract::Request) -> Response {
    if request
        .headers()
        .get(reqwest::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some(&format!("Bearer {}", peer.token))
    {
        return reqwest::StatusCode::UNAUTHORIZED.into_response();
    }
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let bytes = axum::body::to_bytes(request.into_body(), MANAGED_CHUNK_BODY_BYTES)
        .await
        .unwrap();
    if method == reqwest::Method::POST && path.starts_with("/v2/managed/results/") {
        if path.ends_with("/delivery/history") {
            return delivery::handle_history(&peer, &path, &bytes).await;
        }
        if path.ends_with("/delivery") {
            return delivery::handle_delivery(&peer, &path, &bytes).await;
        }
        if path.ends_with("/retirement") {
            return delivery::handle_retirement(&peer, &path, &bytes).await;
        }
        if path.ends_with("/context") {
            return results::context::handle_context(&peer, &path, &bytes).await;
        }
        return results::handle_result(&peer, &path, &bytes).await;
    }
    if path == "/v2/managed/ready" && method == reqwest::Method::POST {
        let input: ManagedReadyRequest = serde_json::from_slice(&bytes).unwrap();
        let at = now().unwrap();
        let value = ManagedReadiness {
            format_version: 1,
            request_id: if peer.mode == Mode::WrongReady {
                uuid::Uuid::new_v4().to_string()
            } else {
                input.request_id.clone()
            },
            request_sha256: canonical_hash(&input),
            runner: peer.fixture.source.grant.payload.runner.clone(),
            environment: input.environment,
            target: input.target,
            runtime_instance_sha256: "9".repeat(64),
            observed_at: at,
            expires_at: at + 15000,
        };
        return reply(
            reqwest::StatusCode::OK,
            peer.fixture
                .runner
                .sign("managed_readiness", value)
                .unwrap(),
        );
    }
    if path == "/v2/managed/inputs" && method == reqwest::Method::POST {
        peer.begins.fetch_add(1, Ordering::SeqCst);
        let input: ManagedBeginRequest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(input.launch, peer.fixture.request);
        assert_eq!(input.grant, peer.fixture.source.grant);
        assert_eq!(input.manifest, peer.fixture.source.manifest);
        if peer.mode == Mode::LostBeginReply {
            return lost_reply();
        }
        let value = ManagedInputTicket {
            format_version: 1,
            input_id: uuid::Uuid::new_v4().to_string(),
            launch_envelope_sha256: if peer.mode == Mode::WrongTicket {
                "0".repeat(64)
            } else {
                envelope_hash(&input.launch)
            },
            grant_envelope_sha256: canonical_hash(&input.grant),
            manifest_sha256: input.manifest.sha256.clone(),
            readiness_sha256: canonical_hash(&input.readiness),
            runtime_instance_sha256: input.readiness.payload.runtime_instance_sha256.clone(),
            expires_at: input
                .launch
                .payload
                .expires_at
                .min(input.grant.payload.expires_at),
        };
        let ticket = peer
            .fixture
            .runner
            .sign("managed_input_ticket", value)
            .unwrap();
        *peer.ticket.lock().unwrap() = Some(ticket.clone());
        return reply(
            reqwest::StatusCode::CREATED,
            ManagedBeginResponse::Input { ticket },
        );
    }
    if method == reqwest::Method::POST && path.ends_with("/missing") {
        let operation: ManagedInputOperation = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            Some(&operation.ticket),
            peer.ticket.lock().unwrap().as_ref()
        );
        let mut missing = chunk();
        if peer.mode == Mode::WrongMissing {
            missing.path = "original-secret.txt".into();
        }
        return reply(
            reqwest::StatusCode::OK,
            ManagedMissingResponse {
                ticket_sha256: canonical_hash(&operation.ticket),
                missing: vec![missing],
            },
        );
    }
    if method == reqwest::Method::POST && path.ends_with("/chunks") {
        peer.chunks.fetch_add(1, Ordering::SeqCst);
        let input: ManagedChunkRequest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(Some(&input.ticket), peer.ticket.lock().unwrap().as_ref());
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(input.bytes_base64)
                .unwrap(),
            b"input"
        );
        if peer.mode == Mode::RevokeSource {
            peer.revoked.store(true, Ordering::SeqCst);
        }
        return reply(
            reqwest::StatusCode::OK,
            ManagedChunkAck {
                receipt_sha256: "8".repeat(64),
                path: input.chunk.path,
                offset: input.chunk.offset,
                size: 5,
                sha256: if peer.mode == Mode::WrongAck {
                    hash(b"other")
                } else {
                    hash(b"input")
                },
            },
        );
    }
    if method == reqwest::Method::POST && path.ends_with("/start") {
        peer.starts.fetch_add(1, Ordering::SeqCst);
        peer.start_called.notify_one();
        let operation: ManagedInputOperation = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            Some(&operation.ticket),
            peer.ticket.lock().unwrap().as_ref()
        );
        if peer.mode == Mode::LostStartReply {
            return lost_reply();
        }
        if peer.mode == Mode::HangStart {
            std::future::pending::<()>().await;
        }
        return reply(reqwest::StatusCode::ACCEPTED, receipt(&peer, false));
    }
    if method == reqwest::Method::GET && path.starts_with("/v2/receipts/") {
        assert_eq!(
            path,
            format!(
                "/v2/receipts/{}",
                peer.fixture.request.payload.process_attempt_id
            )
        );
        peer.polls.fetch_add(1, Ordering::SeqCst);
        return reply(reqwest::StatusCode::OK, receipt(&peer, true));
    }
    panic!("Unexpected managed client route: {method} {path}")
}

async fn launch(server: &Server, stop: CancellationToken) -> Result<Signed<Receipt>> {
    server
        .access
        .managed_launch(
            server.peer.fixture.request.clone(),
            Arc::new(Source(server.peer.clone())),
            stop,
        )
        .await
}

#[tokio::test]
async fn tls_managed_input_reaches_only_its_original_receipt_once() {
    let server = server(Mode::Success).await;
    server
        .access
        .managed_input_ready(
            &server.peer.fixture.request.payload.environment,
            &server.peer.fixture.source.grant.payload.target,
        )
        .await
        .unwrap();
    let receipt = launch(&server, CancellationToken::new()).await.unwrap();
    assert!(matches!(
        receipt.payload.state,
        ExecutionState::Completed { .. }
    ));
    assert_eq!(server.peer.begins.load(Ordering::SeqCst), 1);
    assert_eq!(server.peer.chunks.load(Ordering::SeqCst), 1);
    assert_eq!(server.peer.starts.load(Ordering::SeqCst), 1);
    assert_eq!(server.peer.polls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn tls_manipulated_ready_ticket_missing_ack_or_receipt_never_succeed() {
    for mode in [
        Mode::WrongReady,
        Mode::WrongTicket,
        Mode::WrongMissing,
        Mode::WrongAck,
        Mode::WrongReceipt,
        Mode::RevokeSource,
    ] {
        let server = server(mode).await;
        assert!(
            launch(&server, CancellationToken::new()).await.is_err(),
            "{mode:?}"
        );
        if mode != Mode::WrongReceipt {
            assert_eq!(server.peer.starts.load(Ordering::SeqCst), 0, "{mode:?}");
        }
        if mode == Mode::WrongMissing {
            assert_eq!(server.peer.source_reads.load(Ordering::SeqCst), 0);
        }
        if mode == Mode::WrongReady {
            assert_eq!(server.peer.begins.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn tls_lost_begin_or_start_reply_is_uncertain_without_post_retry() {
    for mode in [Mode::LostBeginReply, Mode::LostStartReply] {
        let server = server(mode).await;
        assert!(matches!(
            launch(&server, CancellationToken::new()).await,
            Err(Error::UncertainEffect)
        ));
        assert_eq!(server.peer.begins.load(Ordering::SeqCst), 1);
        assert_eq!(
            server.peer.starts.load(Ordering::SeqCst),
            usize::from(mode == Mode::LostStartReply)
        );
        assert_eq!(server.peer.polls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn tls_stop_interrupts_a_pending_start_reply_and_does_not_resend() {
    let server = server(Mode::HangStart).await;
    let stop = CancellationToken::new();
    let access = server.access.clone();
    let peer = server.peer.clone();
    let task_stop = stop.clone();
    let task = tokio::spawn(async move {
        access
            .managed_launch(
                peer.fixture.request.clone(),
                Arc::new(Source(peer)),
                task_stop,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), server.peer.start_called.notified())
        .await
        .unwrap();
    stop.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap(),
        Err(Error::UncertainEffect)
    ));
    assert_eq!(server.peer.begins.load(Ordering::SeqCst), 1);
    assert_eq!(server.peer.starts.load(Ordering::SeqCst), 1);
    assert_eq!(server.peer.polls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn tls_untrusted_ca_is_rejected_before_application_authority_is_sent() {
    let server = server(Mode::Success).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .tls_built_in_root_certs(false)
        .retry(reqwest::retry::never())
        .build()
        .unwrap();
    let response = client
        .post(server.access.origin.join(MANAGED_READY_PATH).unwrap())
        .send()
        .await;
    assert!(response.is_err());
    assert_eq!(server.peer.begins.load(Ordering::SeqCst), 0);
}

mod delivery;
mod results;
