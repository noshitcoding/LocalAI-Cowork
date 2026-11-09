//! Actual authenticated loopback TLS; the result/context peer remains synthetic.
use super::*;
use cowork_runner_protocol::{ManagedResultContextRequest, ManagedResultReadContext};

fn context_request(peer: &Peer) -> Signed<ManagedResultContextRequest> {
    let old = read_request(peer, ManagedResultReadOperation::Manifest).payload;
    peer.fixture
        .control
        .sign(
            "managed_result_context_request",
            ManagedResultContextRequest {
                format_version: 1,
                request_id: uuid::Uuid::new_v4().to_string(),
                runtime_instance_sha256: old.runtime_instance_sha256,
                process_attempt_id: old.process_attempt_id,
                run_id: old.run_id,
                launch_envelope_sha256: old.launch_envelope_sha256,
                execution_receipt_sha256: old.execution_receipt_sha256,
                runner: old.runner,
                target: old.target,
                issued_at: old.issued_at,
                expires_at: old.expires_at,
            },
        )
        .unwrap()
}
pub(in super::super) async fn handle_context(peer: &Peer, path: &str, bytes: &[u8]) -> Response {
    let request: Signed<ManagedResultContextRequest> = serde_json::from_slice(bytes).unwrap();
    request
        .verify(
            "managed_result_context_request",
            &peer.fixture.binding.control_public_key,
        )
        .unwrap();
    request
        .payload
        .validate(
            &peer.fixture.binding,
            &peer.fixture.source.grant.payload.target,
            &"9".repeat(64),
            now().unwrap(),
        )
        .unwrap();
    assert_eq!(
        path,
        format!(
            "/v2/managed/results/{}/context",
            request.payload.process_attempt_id
        )
    );
    peer.result_contexts.fetch_add(1, Ordering::SeqCst);
    if peer.mode == Mode::ContextLostReply {
        return lost_reply();
    }
    if peer.mode == Mode::ContextUnsupported {
        return reqwest::StatusCode::NOT_FOUND.into_response();
    }
    let mut context = ManagedResultReadContext {
        format_version: 1,
        expires_at: request.payload.expires_at,
        observed_at: now().unwrap(),
        request: request.payload,
        fence_sha256: "c".repeat(64),
    };
    match peer.mode {
        Mode::ContextWrongNonce => context.request.request_id = uuid::Uuid::new_v4().to_string(),
        Mode::ContextWrongReceipt => context.request.execution_receipt_sha256 = "0".repeat(64),
        Mode::ContextExpired => context.expires_at = context.observed_at,
        _ => {}
    }
    let signer = if peer.mode == Mode::ContextWrongSigner {
        SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap()
    } else {
        peer.fixture.runner.clone()
    };
    reply(
        reqwest::StatusCode::OK,
        signer.sign("managed_result_context", context).unwrap(),
    )
}

#[tokio::test]
async fn tls_context_then_v2_read_binds_original_without_dispatch() {
    let server = server(Mode::Success).await;
    let original = context_request(&server.peer);
    let context = server
        .access
        .managed_result_context(original.clone())
        .await
        .unwrap();
    context
        .payload
        .validate(
            &original.payload,
            &server.peer.fixture.binding,
            now().unwrap(),
        )
        .unwrap();
    let mut read = read_request(&server.peer, ManagedResultReadOperation::Manifest).payload;
    read.format_version = 2;
    read.expires_at = read.expires_at.min(context.payload.expires_at);
    read.context = Some(context);
    let signed = server
        .peer
        .fixture
        .control
        .sign("managed_result_read", read)
        .unwrap();
    server.access.managed_result_manifest(signed).await.unwrap();
    assert_eq!(server.peer.result_contexts.load(Ordering::SeqCst), 1);
    assert_eq!(server.peer.result_reads.load(Ordering::SeqCst), 1);
    no_dispatch(&server.peer);
}
#[tokio::test]
async fn tls_invalid_contexts_are_rejected_before_any_content_post() {
    for mode in [
        Mode::ContextWrongNonce,
        Mode::ContextWrongSigner,
        Mode::ContextWrongReceipt,
        Mode::ContextExpired,
    ] {
        let server = server(mode).await;
        assert!(
            matches!(
                server
                    .access
                    .managed_result_context(context_request(&server.peer))
                    .await,
                Err(Error::Forbidden)
            ),
            "{mode:?}"
        );
        assert_eq!(server.peer.result_contexts.load(Ordering::SeqCst), 1);
        assert_eq!(server.peer.result_reads.load(Ordering::SeqCst), 0);
        no_dispatch(&server.peer);
    }
}
#[tokio::test]
async fn tls_lost_or_unsupported_context_has_no_retry_or_old_read_fallback() {
    for mode in [Mode::ContextLostReply, Mode::ContextUnsupported] {
        let server = server(mode).await;
        assert!(
            server
                .access
                .managed_result_context(context_request(&server.peer))
                .await
                .is_err()
        );
        assert_eq!(server.peer.result_contexts.load(Ordering::SeqCst), 1);
        assert_eq!(server.peer.result_reads.load(Ordering::SeqCst), 0);
        no_dispatch(&server.peer);
    }
}
#[tokio::test]
async fn tls_context_rejects_invalid_signed_path_before_network() {
    let server = server(Mode::Success).await;
    for invalid in ["../other", ".", "..", ""] {
        let mut request = context_request(&server.peer).payload;
        request.process_attempt_id = invalid.into();
        let signed = server
            .peer
            .fixture
            .control
            .sign("managed_result_context_request", request)
            .unwrap();
        assert!(matches!(
            server.access.managed_result_context(signed).await,
            Err(Error::Forbidden)
        ));
    }
    assert_eq!(server.peer.result_contexts.load(Ordering::SeqCst), 0);
    assert_eq!(server.peer.result_reads.load(Ordering::SeqCst), 0);
    no_dispatch(&server.peer);
}
