//! Actual loopback TLS with a synthetic output signer. No native service claim.
use super::*;
pub(super) mod context;
use cowork_runner_protocol::{
    ManagedOutputAttestation, ManagedResultManifest, ManagedResultReadOperation,
    ManagedResultReadRequest,
};

fn original_receipt(peer: &Peer) -> Signed<Receipt> {
    let mut value = receipt(peer, true).payload;
    value.updated_at = peer.fixture.request.payload.issued_at;
    peer.fixture.runner.sign("receipt", value).unwrap()
}
fn output(peer: &Peer, request: &ManagedResultReadRequest) -> ManagedResultManifest {
    let at = now().unwrap();
    let manifest = peer.fixture.source.manifest.clone();
    let mut attestation = ManagedOutputAttestation {
        format_version: 1,
        runtime_instance_sha256: request.runtime_instance_sha256.clone(),
        execution_plan_id: request.execution_plan_id.clone(),
        execution_plan_sha256: request.execution_plan_sha256.clone(),
        process_attempt_id: request.process_attempt_id.clone(),
        run_id: request.run_id.clone(),
        launch_envelope_sha256: request.launch_envelope_sha256.clone(),
        input_manifest_sha256: peer.fixture.source.manifest.sha256.clone(),
        output_manifest_sha256: manifest.sha256.clone(),
        execution_receipt_sha256: canonical_hash(&original_receipt(peer)),
        physical_record_sha256: "a".repeat(64),
        runner: request.runner.clone(),
        target: request.target.clone(),
        produced_at: at,
        retained_until: at + 60000,
    };
    match peer.mode {
        Mode::ResultForeignReceipt => attestation.execution_receipt_sha256 = "0".repeat(64),
        Mode::ResultForeignInstance => attestation.runtime_instance_sha256 = "8".repeat(64),
        Mode::ResultExpiredAttestation => {
            attestation.produced_at = at - 60000;
            attestation.retained_until = at - 1;
        }
        _ => {}
    }
    let signer = if peer.mode == Mode::ResultInvalidSignature {
        SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap()
    } else {
        peer.fixture.runner.clone()
    };
    ManagedResultManifest {
        attestation: signer.sign("managed_output", attestation).unwrap(),
        manifest,
    }
}
pub(super) async fn handle_result(peer: &Peer, path: &str, bytes: &[u8]) -> Response {
    let request: Signed<ManagedResultReadRequest> = serde_json::from_slice(bytes).unwrap();
    request
        .verify(
            "managed_result_read",
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
            "/v2/managed/results/{}/read",
            request.payload.process_attempt_id
        )
    );
    peer.result_reads.fetch_add(1, Ordering::SeqCst);
    match peer.mode {
        Mode::ResultUnavailable => return reqwest::StatusCode::CONFLICT.into_response(),
        Mode::ResultExpired => return reqwest::StatusCode::GONE.into_response(),
        Mode::ResultRetired => return reqwest::StatusCode::FORBIDDEN.into_response(),
        Mode::ResultLostReply => return lost_reply(),
        _ => {}
    }
    match &request.payload.operation {
        ManagedResultReadOperation::Manifest => {
            reply(reqwest::StatusCode::OK, output(peer, &request.payload))
        }
        ManagedResultReadOperation::Chunk { chunk: input, .. } => {
            assert_eq!(input.path, chunk().path);
            let bytes = match peer.mode {
                Mode::ResultCorruptChunk => b"other".to_vec(),
                Mode::ResultOversizedChunk => b"input-more".to_vec(),
                _ => b"input".to_vec(),
            };
            (
                [(
                    reqwest::header::CONTENT_TYPE,
                    if peer.mode == Mode::ResultWrongType {
                        "application/json"
                    } else {
                        "application/octet-stream"
                    },
                )],
                bytes,
            )
                .into_response()
        }
    }
}
fn read_request(
    peer: &Peer,
    operation: ManagedResultReadOperation,
) -> Signed<ManagedResultReadRequest> {
    let g = &peer.fixture.source.grant.payload;
    let at = now().unwrap();
    peer.fixture
        .control
        .sign(
            "managed_result_read",
            ManagedResultReadRequest {
                format_version: 1,
                runtime_instance_sha256: "9".repeat(64),
                request_id: uuid::Uuid::new_v4().to_string(),
                actor_id: "installation-owner:test-controller".into(),
                authorization_sha256: "b".repeat(64),
                execution_plan_id: g.execution_plan_id.clone(),
                execution_plan_sha256: g.execution_plan_sha256.clone(),
                process_attempt_id: g.process_attempt_id.clone(),
                run_id: g.run_id.clone(),
                launch_envelope_sha256: envelope_hash(&peer.fixture.request),
                execution_receipt_sha256: canonical_hash(&original_receipt(peer)),
                runner: g.runner.clone(),
                target: g.target.clone(),
                operation,
                issued_at: at,
                expires_at: at + 30000,
                context: None,
            },
        )
        .unwrap()
}
fn read_chunk_request(peer: &Peer) -> Signed<ManagedResultReadRequest> {
    let manifest_request = read_request(peer, ManagedResultReadOperation::Manifest);
    let result = output(peer, &manifest_request.payload);
    read_request(
        peer,
        ManagedResultReadOperation::Chunk {
            attestation_sha256: canonical_hash(&result.attestation),
            manifest_sha256: result.manifest.sha256,
            chunk: chunk(),
        },
    )
}
fn no_dispatch(peer: &Peer) {
    assert_eq!(peer.begins.load(Ordering::SeqCst), 0);
    assert_eq!(peer.starts.load(Ordering::SeqCst), 0);
    assert_eq!(peer.chunks.load(Ordering::SeqCst), 0);
    assert_eq!(peer.polls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn tls_result_read_uses_fresh_signed_request_and_original_receipt_without_dispatch() {
    let server = server(Mode::Success).await;
    let f = &server.peer.fixture;
    let ready_request = ManagedReadyRequest {
        format_version: 1,
        request_id: uuid::Uuid::new_v4().to_string(),
        environment: f.request.payload.environment.clone(),
        target: f.source.grant.payload.target.clone(),
    };
    let ready = server
        .access
        .managed_result_readiness(ready_request.clone())
        .await
        .unwrap();
    ready
        .payload
        .validate(&ready_request, &f.binding, now().unwrap())
        .unwrap();
    let request = read_request(&server.peer, ManagedResultReadOperation::Manifest);
    let result = server
        .access
        .managed_result_manifest(request)
        .await
        .unwrap();
    result
        .attestation
        .payload
        .validate(
            &f.source.grant.payload,
            &f.request,
            &original_receipt(&server.peer),
            &result.manifest,
            &ready.payload.runtime_instance_sha256,
            now().unwrap(),
        )
        .unwrap();
    let bytes = server
        .access
        .managed_result_chunk(read_chunk_request(&server.peer), 5)
        .await
        .unwrap();
    assert_eq!(bytes, b"input");
    assert_eq!(server.peer.result_reads.load(Ordering::SeqCst), 2);
    no_dispatch(&server.peer);
}

#[tokio::test]
async fn tls_result_foreign_receipt_instance_or_signer_is_rejected() {
    for mode in [
        Mode::ResultForeignReceipt,
        Mode::ResultForeignInstance,
        Mode::ResultInvalidSignature,
        Mode::ResultExpiredAttestation,
    ] {
        let server = server(mode).await;
        assert!(
            matches!(
                server
                    .access
                    .managed_result_manifest(read_request(
                        &server.peer,
                        ManagedResultReadOperation::Manifest
                    ))
                    .await,
                Err(Error::Forbidden)
            ),
            "{mode:?}"
        );
        assert_eq!(server.peer.result_reads.load(Ordering::SeqCst), 1);
        no_dispatch(&server.peer);
    }
}

#[tokio::test]
async fn tls_result_corrupt_oversized_or_wrong_type_bytes_are_rejected() {
    for mode in [
        Mode::ResultCorruptChunk,
        Mode::ResultOversizedChunk,
        Mode::ResultWrongType,
    ] {
        let server = server(mode).await;
        assert!(
            matches!(
                server
                    .access
                    .managed_result_chunk(read_chunk_request(&server.peer), 5)
                    .await,
                Err(Error::FileConflict)
            ),
            "{mode:?}"
        );
        assert_eq!(server.peer.result_reads.load(Ordering::SeqCst), 1);
        no_dispatch(&server.peer);
    }
}

#[tokio::test]
async fn tls_result_unavailable_expired_revoked_and_lost_reply_do_not_retry() {
    for mode in [
        Mode::ResultUnavailable,
        Mode::ResultExpired,
        Mode::ResultRetired,
        Mode::ResultLostReply,
    ] {
        let server = server(mode).await;
        let result = server
            .access
            .managed_result_manifest(read_request(
                &server.peer,
                ManagedResultReadOperation::Manifest,
            ))
            .await;
        match mode {
            Mode::ResultUnavailable => assert!(matches!(result, Err(Error::Validation(_)))),
            Mode::ResultExpired => assert!(matches!(result, Err(Error::NotFound))),
            Mode::ResultRetired => assert!(matches!(result, Err(Error::Forbidden))),
            Mode::ResultLostReply => assert!(matches!(result, Err(Error::UncertainEffect))),
            _ => unreachable!(),
        }
        assert_eq!(server.peer.result_reads.load(Ordering::SeqCst), 1);
        no_dispatch(&server.peer);
    }
}

#[tokio::test]
async fn tls_result_wrong_purpose_or_expired_local_request_is_not_sent() {
    let server = server(Mode::Success).await;
    let valid = read_request(&server.peer, ManagedResultReadOperation::Manifest);
    let wrong = server
        .peer
        .fixture
        .control
        .sign("launch", valid.payload.clone())
        .unwrap();
    assert!(matches!(
        server.access.managed_result_manifest(wrong).await,
        Err(Error::Forbidden)
    ));
    let mut expired = valid.payload;
    expired.issued_at -= 60000;
    expired.expires_at -= 60000;
    assert!(matches!(
        server
            .access
            .managed_result_manifest(
                server
                    .peer
                    .fixture
                    .control
                    .sign("managed_result_read", expired)
                    .unwrap()
            )
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(server.peer.result_reads.load(Ordering::SeqCst), 0);
    no_dispatch(&server.peer);
}
