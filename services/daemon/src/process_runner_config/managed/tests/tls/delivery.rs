//! Actual authenticated loopback TLS. The signed peer is synthetic; these
//! transport cases establish neither native acceptance nor physical retirement.
use super::*;
use cowork_runner_protocol::{
    ManagedResultDeliveryReceipt, ManagedResultDeliveryRequest, ManagedResultRetirementQuery,
    ManagedResultRetirementReceipt, ManagedResultRetirementState,
};
use cowork_runtime::ManagedResultDeliveryResponse;

fn request(peer: &Peer) -> Signed<ManagedResultDeliveryRequest> {
    let grant = &peer.fixture.source.grant.payload;
    let issued_at = now().unwrap();
    peer.fixture
        .control
        .sign(
            "managed_result_delivery_request",
            ManagedResultDeliveryRequest {
                format_version: 1,
                request_id: uuid::Uuid::new_v4().to_string(),
                actor_id: "installation-owner:tls-fixture".into(),
                authorization_sha256: "b".repeat(64),
                runtime_instance_sha256: "9".repeat(64),
                execution_plan_id: grant.execution_plan_id.clone(),
                execution_plan_sha256: grant.execution_plan_sha256.clone(),
                run_id: grant.run_id.clone(),
                process_attempt_id: grant.process_attempt_id.clone(),
                launch_envelope_sha256: envelope_hash(&peer.fixture.request),
                execution_receipt_sha256: "c".repeat(64),
                output_attestation_sha256: "d".repeat(64),
                manifest_sha256: peer.fixture.source.manifest.sha256.clone(),
                recipient_version_id: uuid::Uuid::new_v4().to_string(),
                runner: grant.runner.clone(),
                target: grant.target.clone(),
                issued_at,
                accept_before: issued_at + 30000,
            },
        )
        .unwrap()
}

pub(super) async fn handle_history(peer: &Peer, path: &str, bytes: &[u8]) -> Response {
    let request: Signed<ManagedResultDeliveryRequest> = serde_json::from_slice(bytes).unwrap();
    request
        .verify(
            "managed_result_delivery_request",
            &peer.fixture.binding.control_public_key,
        )
        .unwrap();
    request
        .payload
        .validate(
            &peer.fixture.binding,
            &request.payload.target,
            &request.payload.runtime_instance_sha256,
            request.payload.issued_at,
        )
        .unwrap();
    assert_eq!(
        path,
        format!(
            "/v2/managed/results/{}/delivery/history",
            request.payload.process_attempt_id
        )
    );
    peer.delivery_history_reads.fetch_add(1, Ordering::SeqCst);
    if peer.mode == Mode::DeliveryHistoryMissing {
        return reqwest::StatusCode::NOT_FOUND.into_response();
    }
    if peer.mode == Mode::DeliveryExpired {
        return reply(
            reqwest::StatusCode::CONFLICT,
            serde_json::json!({"error":"managed_result_delivery_confirmation_expired"}),
        );
    }
    let Some(original) = peer.accepted_delivery.lock().unwrap().clone() else {
        return reqwest::StatusCode::NOT_FOUND.into_response();
    };
    assert_eq!(original.payload.request, request);
    let mut value = original.payload;
    if peer.mode == Mode::DeliveryWrongRequest {
        let mut changed = value.request.payload;
        changed.recipient_version_id = uuid::Uuid::new_v4().to_string();
        value.request = peer
            .fixture
            .control
            .sign("managed_result_delivery_request", changed)
            .unwrap();
    }
    let signer = if peer.mode == Mode::DeliveryWrongSigner {
        SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap()
    } else {
        peer.fixture.runner.clone()
    };
    reply(
        reqwest::StatusCode::OK,
        signer
            .sign("managed_result_delivery_receipt", value)
            .unwrap(),
    )
}

pub(super) async fn handle_delivery(peer: &Peer, path: &str, bytes: &[u8]) -> Response {
    let request: Signed<ManagedResultDeliveryRequest> = serde_json::from_slice(bytes).unwrap();
    request
        .verify(
            "managed_result_delivery_request",
            &peer.fixture.binding.control_public_key,
        )
        .unwrap();
    request
        .payload
        .validate(
            &peer.fixture.binding,
            &peer.fixture.source.grant.payload.target,
            &"9".repeat(64),
            request.payload.issued_at,
        )
        .unwrap();
    assert_eq!(
        path,
        format!(
            "/v2/managed/results/{}/delivery",
            request.payload.process_attempt_id
        )
    );
    peer.deliveries.fetch_add(1, Ordering::SeqCst);
    match peer.mode {
        Mode::DeliveryUnauthorized => return reqwest::StatusCode::UNAUTHORIZED.into_response(),
        Mode::DeliveryForbidden => return reqwest::StatusCode::FORBIDDEN.into_response(),
        Mode::DeliveryUnknownConflict => {
            return reply(
                reqwest::StatusCode::CONFLICT,
                serde_json::json!({"error":"unrecognized_delivery_state"}),
            );
        }
        Mode::DeliveryExpired => {
            return reply(
                reqwest::StatusCode::CONFLICT,
                serde_json::json!({"error":"managed_result_delivery_confirmation_expired"}),
            );
        }
        Mode::DeliveryExtraError => {
            return reply(
                reqwest::StatusCode::CONFLICT,
                serde_json::json!({"error":"managed_result_delivery_confirmation_expired", "extra":true}),
            );
        }
        Mode::DeliveryMalformedError => {
            return (reqwest::StatusCode::CONFLICT, "invalid-json").into_response();
        }
        Mode::DeliveryOversizedStream => {
            return Response::new(Body::from_stream(futures_util::stream::iter(
                (0..5).map(|_| Ok::<_, std::io::Error>(vec![b' '; 4096])),
            )));
        }
        _ => {}
    }
    let stored = peer.accepted_delivery.lock().unwrap().clone();
    let accepted = if let Some(stored) = stored {
        assert_eq!(stored.payload.request, request);
        stored
    } else {
        // Synthetic historical acceptance at the original issue time. This
        // tests client history validation without changing the machine clock.
        let value = peer
            .fixture
            .runner
            .sign(
                "managed_result_delivery_receipt",
                ManagedResultDeliveryReceipt {
                    format_version: 1,
                    accepted_at: request.payload.issued_at,
                    request: request.clone(),
                },
            )
            .unwrap();
        *peer.accepted_delivery.lock().unwrap() = Some(value.clone());
        value
    };
    if peer.mode == Mode::DeliveryLostReply {
        return lost_reply();
    }
    let mut value = accepted.payload;
    if peer.mode == Mode::DeliveryWrongRequest {
        let mut changed = value.request.payload;
        changed.recipient_version_id = uuid::Uuid::new_v4().to_string();
        value.request = peer
            .fixture
            .control
            .sign("managed_result_delivery_request", changed)
            .unwrap();
    }
    let signer = if peer.mode == Mode::DeliveryWrongSigner {
        SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap()
    } else {
        peer.fixture.runner.clone()
    };
    reply(
        reqwest::StatusCode::OK,
        signer
            .sign("managed_result_delivery_receipt", value)
            .unwrap(),
    )
}

fn query(
    peer: &Peer,
    accepted: &Signed<ManagedResultDeliveryReceipt>,
) -> Signed<ManagedResultRetirementQuery> {
    let original = &accepted.payload.request.payload;
    let issued_at = now().unwrap();
    peer.fixture
        .control
        .sign(
            "managed_result_retirement_query",
            ManagedResultRetirementQuery {
                format_version: 1,
                request_id: uuid::Uuid::new_v4().to_string(),
                delivery_receipt_sha256: canonical_hash(accepted),
                process_attempt_id: original.process_attempt_id.clone(),
                runner: original.runner.clone(),
                target: original.target.clone(),
                issued_at,
                expires_at: issued_at + 30000,
            },
        )
        .unwrap()
}

pub(super) async fn handle_retirement(peer: &Peer, path: &str, bytes: &[u8]) -> Response {
    let query: Signed<ManagedResultRetirementQuery> = serde_json::from_slice(bytes).unwrap();
    query
        .verify(
            "managed_result_retirement_query",
            &peer.fixture.binding.control_public_key,
        )
        .unwrap();
    let accepted = peer.accepted_delivery.lock().unwrap().clone().unwrap();
    query
        .payload
        .validate(&accepted, &peer.fixture.binding, now().unwrap())
        .unwrap();
    assert_eq!(
        path,
        format!(
            "/v2/managed/results/{}/retirement",
            query.payload.process_attempt_id
        )
    );
    peer.retirements.fetch_add(1, Ordering::SeqCst);
    let original = &accepted.payload.request.payload;
    let value = ManagedResultRetirementReceipt {
        format_version: 1,
        delivery_receipt_sha256: if peer.mode == Mode::RetirementWrongDelivery {
            "0".repeat(64)
        } else {
            canonical_hash(&accepted)
        },
        process_attempt_id: original.process_attempt_id.clone(),
        run_id: original.run_id.clone(),
        runtime_instance_sha256: original.runtime_instance_sha256.clone(),
        revision: 1,
        state: ManagedResultRetirementState::Pending,
        recorded_at: now().unwrap(),
        physical_completion_sha256: "a".repeat(64),
        retirement_evidence_sha256: None,
    };
    reply(
        reqwest::StatusCode::OK,
        peer.fixture
            .runner
            .sign("managed_result_retirement_receipt", value)
            .unwrap(),
    )
}

fn no_dispatch(peer: &Peer) {
    for counter in [
        &peer.begins,
        &peer.starts,
        &peer.chunks,
        &peer.polls,
        &peer.source_reads,
        &peer.result_reads,
        &peer.result_contexts,
    ] {
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn tls_delivery_retains_original_historical_receipt_without_dispatch() {
    let server = server(Mode::Success).await;
    let mut old = request(&server.peer).payload;
    old.issued_at -= 60000;
    old.accept_before -= 60000;
    let original = server
        .peer
        .fixture
        .control
        .sign("managed_result_delivery_request", old)
        .unwrap();
    let ManagedResultDeliveryResponse::Accepted(first) = server
        .access
        .deliver_managed_result(original.clone())
        .await
        .unwrap()
    else {
        panic!("expected original accepted history")
    };
    let ManagedResultDeliveryResponse::Accepted(second) = server
        .access
        .deliver_managed_result(original.clone())
        .await
        .unwrap()
    else {
        panic!("expected same accepted history")
    };
    assert_eq!(first, second);
    assert_eq!(first.payload.request, original);
    assert!(first.payload.request.payload.accept_before < now().unwrap());
    assert_eq!(server.peer.deliveries.load(Ordering::SeqCst), 2);
    no_dispatch(&server.peer);
}

#[tokio::test]
async fn tls_delivery_only_exact_expiry_is_classified_as_unaccepted() {
    for mode in [
        Mode::DeliveryExpired,
        Mode::DeliveryUnknownConflict,
        Mode::DeliveryExtraError,
        Mode::DeliveryMalformedError,
    ] {
        let server = server(mode).await;
        let result = server
            .access
            .deliver_managed_result(request(&server.peer))
            .await;
        if mode == Mode::DeliveryExpired {
            assert!(matches!(
                result,
                Ok(ManagedResultDeliveryResponse::ConfirmationExpired)
            ));
        } else {
            assert!(matches!(result, Err(Error::UncertainEffect)), "{mode:?}");
        }
        assert_eq!(server.peer.deliveries.load(Ordering::SeqCst), 1);
        assert!(server.peer.accepted_delivery.lock().unwrap().is_none());
        no_dispatch(&server.peer);
    }
}

#[tokio::test]
async fn tls_delivery_post_send_denials_and_invalid_acknowledgments_remain_uncertain() {
    for mode in [
        Mode::DeliveryUnauthorized,
        Mode::DeliveryForbidden,
        Mode::DeliveryWrongSigner,
        Mode::DeliveryWrongRequest,
    ] {
        let server = server(mode).await;
        assert!(
            matches!(
                server
                    .access
                    .deliver_managed_result(request(&server.peer))
                    .await,
                Err(Error::UncertainEffect)
            ),
            "{mode:?}"
        );
        assert_eq!(server.peer.deliveries.load(Ordering::SeqCst), 1);
        no_dispatch(&server.peer);
    }
}

#[tokio::test]
async fn tls_delivery_lost_reply_and_unbounded_stream_never_retry_the_post() {
    for mode in [Mode::DeliveryLostReply, Mode::DeliveryOversizedStream] {
        let server = server(mode).await;
        assert!(
            matches!(
                server
                    .access
                    .deliver_managed_result(request(&server.peer))
                    .await,
                Err(Error::UncertainEffect)
            ),
            "{mode:?}"
        );
        assert_eq!(server.peer.deliveries.load(Ordering::SeqCst), 1);
        if mode == Mode::DeliveryLostReply {
            assert!(server.peer.accepted_delivery.lock().unwrap().is_some());
        }
        no_dispatch(&server.peer);
    }
}

#[tokio::test]
async fn tls_delivery_rejects_dot_path_before_contacting_peer() {
    let server = server(Mode::Success).await;
    for attempt in [".", ".."] {
        let mut value = request(&server.peer).payload;
        value.process_attempt_id = attempt.into();
        let signed = server
            .peer
            .fixture
            .control
            .sign("managed_result_delivery_request", value)
            .unwrap();
        assert!(matches!(
            server.access.deliver_managed_result(signed).await,
            Err(Error::Forbidden)
        ));
    }
    assert_eq!(server.peer.deliveries.load(Ordering::SeqCst), 0);
    no_dispatch(&server.peer);
}

#[tokio::test]
async fn tls_retirement_query_is_separate_and_does_not_repeat_delivery() {
    let server = server(Mode::Success).await;
    let ManagedResultDeliveryResponse::Accepted(accepted) = server
        .access
        .deliver_managed_result(request(&server.peer))
        .await
        .unwrap()
    else {
        panic!("expected acceptance")
    };
    let status = server
        .access
        .observe_managed_retirement(query(&server.peer, &accepted))
        .await
        .unwrap();
    status
        .payload
        .validate(&accepted, &server.peer.fixture.binding, &"a".repeat(64))
        .unwrap();
    assert_eq!(status.payload.state, ManagedResultRetirementState::Pending);
    assert_eq!(server.peer.deliveries.load(Ordering::SeqCst), 1);
    assert_eq!(server.peer.retirements.load(Ordering::SeqCst), 1);
    no_dispatch(&server.peer);
}

#[tokio::test]
async fn tls_retirement_rejects_history_for_another_delivery() {
    let server = server(Mode::RetirementWrongDelivery).await;
    let ManagedResultDeliveryResponse::Accepted(accepted) = server
        .access
        .deliver_managed_result(request(&server.peer))
        .await
        .unwrap()
    else {
        panic!("expected acceptance")
    };
    assert!(matches!(
        server
            .access
            .observe_managed_retirement(query(&server.peer, &accepted))
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(server.peer.deliveries.load(Ordering::SeqCst), 1);
    assert_eq!(server.peer.retirements.load(Ordering::SeqCst), 1);
    no_dispatch(&server.peer);
}

#[tokio::test]
async fn tls_delivery_history_reads_only_the_existing_original_ack_without_admission() {
    let server = server(Mode::Success).await;
    let mut value = request(&server.peer).payload;
    value.issued_at -= 60000;
    value.accept_before -= 60000;
    let original = server
        .peer
        .fixture
        .control
        .sign("managed_result_delivery_request", value)
        .unwrap();
    let accepted = server
        .peer
        .fixture
        .runner
        .sign(
            "managed_result_delivery_receipt",
            ManagedResultDeliveryReceipt {
                format_version: 1,
                accepted_at: original.payload.issued_at,
                request: original.clone(),
            },
        )
        .unwrap();
    *server.peer.accepted_delivery.lock().unwrap() = Some(accepted.clone());
    assert_eq!(
        server
            .access
            .read_managed_delivery_history(original)
            .await
            .unwrap(),
        accepted
    );
    assert_eq!(server.peer.deliveries.load(Ordering::SeqCst), 0);
    assert_eq!(server.peer.delivery_history_reads.load(Ordering::SeqCst), 1);
    assert_eq!(server.peer.retirements.load(Ordering::SeqCst), 0);
    no_dispatch(&server.peer);
}

#[tokio::test]
async fn tls_delivery_history_errors_never_first_accept_or_classify_expiry() {
    for mode in [
        Mode::DeliveryHistoryMissing,
        Mode::DeliveryExpired,
        Mode::DeliveryWrongSigner,
        Mode::DeliveryWrongRequest,
    ] {
        let server = server(mode).await;
        let original = request(&server.peer);
        let accepted = server
            .peer
            .fixture
            .runner
            .sign(
                "managed_result_delivery_receipt",
                ManagedResultDeliveryReceipt {
                    format_version: 1,
                    accepted_at: original.payload.issued_at,
                    request: original.clone(),
                },
            )
            .unwrap();
        *server.peer.accepted_delivery.lock().unwrap() = Some(accepted.clone());
        assert!(
            matches!(
                server.access.read_managed_delivery_history(original).await,
                Err(Error::UncertainEffect)
            ),
            "{mode:?}"
        );
        assert_eq!(
            server.peer.accepted_delivery.lock().unwrap().as_ref(),
            Some(&accepted)
        );
        assert_eq!(server.peer.deliveries.load(Ordering::SeqCst), 0);
        assert_eq!(server.peer.delivery_history_reads.load(Ordering::SeqCst), 1);
        no_dispatch(&server.peer);
    }
}
