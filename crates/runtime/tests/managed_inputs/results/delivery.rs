//! Actual SQL/native Ready custody with a deliberately synthetic signed peer.
//! No real remote volume cleanup or installed App acceptance is inferred.
use super::*;
use cowork_runner_protocol::{
    ManagedResultDeliveryReceipt, ManagedResultDeliveryRequest, ManagedResultRetirementQuery,
    ManagedResultRetirementReceipt, ManagedResultRetirementState,
};
use cowork_runtime::ManagedResultDeliveryResponse;
use cowork_storage::{ManagedResultDelivery, ManagedResultDeliveryState, ManagedResultVersion};

impl ResultRunner {
    pub(super) async fn fixture_delivery_history(
        &self,
        request: Signed<ManagedResultDeliveryRequest>,
    ) -> cowork_domain::Result<Signed<ManagedResultDeliveryReceipt>> {
        request
            .verify(
                "managed_result_delivery_request",
                &self.delivery_binding.control_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        self.delivery_history_requests
            .lock()
            .unwrap()
            .push(request.clone());
        if self.delivery_mode.load(Ordering::SeqCst) == 6 {
            return Err(Error::NotFound);
        }
        let accepted = self
            .delivery_accepted
            .lock()
            .unwrap()
            .clone()
            .ok_or(Error::NotFound)?;
        if accepted.payload.request != request {
            return Err(Error::Forbidden);
        }
        if self.delivery_mode.load(Ordering::SeqCst) == 5 {
            return SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap())
                .unwrap()
                .sign("managed_result_delivery_receipt", accepted.payload)
                .map_err(|_| Error::Forbidden);
        }
        Ok(accepted)
    }
    pub(super) async fn fixture_delivery(
        &self,
        request: Signed<ManagedResultDeliveryRequest>,
    ) -> cowork_domain::Result<ManagedResultDeliveryResponse> {
        request
            .verify(
                "managed_result_delivery_request",
                &self.input.grant.payload.runner.control_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        self.delivery_requests.lock().unwrap().push(request.clone());
        let mode = self.delivery_mode.load(Ordering::SeqCst);
        let previous = self.delivery_accepted.lock().unwrap().clone();
        if let Some(previous) = previous {
            if previous.payload.request != request {
                return Err(Error::UncertainEffect);
            }
            return Ok(ManagedResultDeliveryResponse::Accepted(Box::new(previous)));
        }
        if now() >= request.payload.accept_before {
            return Ok(ManagedResultDeliveryResponse::ConfirmationExpired);
        }
        request
            .payload
            .validate(
                &self.delivery_binding,
                &self.input.grant.payload.target,
                &self.result().attestation.payload.runtime_instance_sha256,
                now(),
            )
            .map_err(|_| Error::Forbidden)?;
        if mode == 4 {
            return Err(Error::UncertainEffect);
        }
        let receipt = self
            .inner
            .key
            .sign(
                "managed_result_delivery_receipt",
                ManagedResultDeliveryReceipt {
                    format_version: 1,
                    request,
                    accepted_at: now(),
                },
            )
            .map_err(|_| Error::Forbidden)?;
        *self.delivery_accepted.lock().unwrap() = Some(receipt.clone());
        if mode == 1 {
            return Err(Error::UncertainEffect);
        }
        if mode == 2 {
            self.store.cancel(&self.launch.payload.run_id).await?;
        }
        if mode == 3 {
            let other = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
            return Ok(ManagedResultDeliveryResponse::Accepted(
                other
                    .sign("managed_result_delivery_receipt", receipt.payload)
                    .unwrap()
                    .into(),
            ));
        }
        Ok(ManagedResultDeliveryResponse::Accepted(Box::new(receipt)))
    }
    pub(super) async fn fixture_retirement(
        &self,
        request: Signed<ManagedResultRetirementQuery>,
    ) -> cowork_domain::Result<Signed<ManagedResultRetirementReceipt>> {
        request
            .verify(
                "managed_result_retirement_query",
                &self.input.grant.payload.runner.control_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        let accepted = self
            .delivery_accepted
            .lock()
            .unwrap()
            .clone()
            .ok_or(Error::NotFound)?;
        request
            .payload
            .validate(&accepted, &self.delivery_binding, now())
            .map_err(|_| Error::Forbidden)?;
        self.retirement_queries.fetch_add(1, Ordering::SeqCst);
        let revision = self.retirement_state.load(Ordering::SeqCst) as u64;
        let state = match revision {
            1 => ManagedResultRetirementState::Pending,
            3 => ManagedResultRetirementState::Unconfirmed,
            _ => ManagedResultRetirementState::Retired,
        };
        let record = ManagedResultRetirementReceipt {
            format_version: 1,
            delivery_receipt_sha256: canonical_hash(&accepted),
            process_attempt_id: request.payload.process_attempt_id,
            run_id: self.launch.payload.run_id.clone(),
            runtime_instance_sha256: accepted
                .payload
                .request
                .payload
                .runtime_instance_sha256
                .clone(),
            revision,
            state,
            recorded_at: now().max(accepted.payload.accepted_at),
            physical_completion_sha256: self.result().attestation.payload.physical_record_sha256,
            retirement_evidence_sha256: (state == ManagedResultRetirementState::Retired)
                .then(|| "d".repeat(64)),
        };
        self.inner
            .key
            .sign("managed_result_retirement_receipt", record)
            .map_err(|_| Error::Forbidden)
    }
}

async fn local_version(f: &Fixture) -> ManagedResultVersion {
    let text: String = sqlx::query_scalar("SELECT payload FROM owner_managed_result_versions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    serde_json::from_str(&text).unwrap()
}
async fn ready(f: &Fixture, runtime: &Runtime, peer: &ResultRunner, run: &RunRecord) -> String {
    runtime
        .retain_managed_result_version(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
        .await
        .unwrap()
}
async fn prepare(
    f: &Fixture,
    runtime: &Runtime,
    peer: &ResultRunner,
    run: &RunRecord,
) -> ManagedResultDelivery {
    runtime
        .prepare_owner_managed_result_delivery(
            f.order(),
            &run.id,
            &peer.launch.payload.process_attempt_id,
        )
        .await
        .unwrap()
}
async fn confirm(
    f: &Fixture,
    runtime: &Runtime,
    peer: &ResultRunner,
    run: &RunRecord,
    intent: &ManagedResultDelivery,
) -> cowork_domain::Result<ManagedResultDelivery> {
    runtime
        .confirm_owner_managed_result_delivery(
            f.order(),
            &run.id,
            &peer.launch.payload.process_attempt_id,
            &intent.id,
            &canonical_hash(&intent.request),
        )
        .await
}
async fn history(
    f: &Fixture,
    runtime: &Runtime,
    peer: &ResultRunner,
    run: &RunRecord,
) -> ManagedResultDelivery {
    runtime
        .owner_managed_result_delivery(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn delivery_requires_actual_ready_and_preparation_has_no_remote_effect() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    assert!(
        runtime
            .prepare_owner_managed_result_delivery(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .is_err()
    );
    let id = ready(&f, &runtime, &peer, &run).await;
    let before = local_version(&f).await;
    let run_before = f.runtime.store.stored_run(&run.id).await.unwrap();
    let first = prepare(&f, &runtime, &peer, &run).await;
    assert_eq!(first.state, ManagedResultDeliveryState::Prepared);
    assert_eq!(first.result_version_id, id);
    assert_eq!(first, prepare(&f, &runtime, &peer, &run).await);
    assert!(peer.delivery_requests.lock().unwrap().is_empty());
    assert_eq!(
        runtime
            .refresh_owner_managed_result_retirement(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .unwrap(),
        first
    );
    assert_eq!(peer.retirement_queries.load(Ordering::SeqCst), 0);
    assert_eq!(local_version(&f).await, before);
    assert_eq!(
        canonical_hash(&f.runtime.store.stored_run(&run.id).await.unwrap()),
        canonical_hash(&run_before)
    );
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn delivery_lost_reply_retries_only_original_request_and_retains_original_acceptance() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    ready(&f, &runtime, &peer, &run).await;
    let before = local_version(&f).await;
    let intent = prepare(&f, &runtime, &peer, &run).await;
    peer.delivery_mode.store(1, Ordering::SeqCst);
    assert!(matches!(
        confirm(&f, &runtime, &peer, &run, &intent).await,
        Err(Error::UncertainEffect)
    ));
    let committed = history(&f, &runtime, &peer, &run).await;
    assert_eq!(committed.state, ManagedResultDeliveryState::Committed);
    assert_eq!(committed.request, intent.request);
    assert_eq!(prepare(&f, &runtime, &peer, &run).await, committed);
    peer.delivery_mode.store(0, Ordering::SeqCst);
    let accepted = confirm(&f, &runtime, &peer, &run, &intent).await.unwrap();
    assert_eq!(accepted.state, ManagedResultDeliveryState::Accepted);
    assert_eq!(
        accepted.accepted,
        peer.delivery_accepted.lock().unwrap().clone()
    );
    let trace = peer.delivery_requests.lock().unwrap().clone();
    assert_eq!(trace.len(), 2);
    assert_eq!(trace[0], trace[1]);
    assert_eq!(
        confirm(&f, &runtime, &peer, &run, &intent).await.unwrap(),
        accepted
    );
    assert_eq!(peer.delivery_requests.lock().unwrap().len(), 2);
    assert_eq!(local_version(&f).await, before);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn invalid_delivery_reply_is_uncertain_and_cannot_replace_committed_intent() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    ready(&f, &runtime, &peer, &run).await;
    let intent = prepare(&f, &runtime, &peer, &run).await;
    assert!(
        runtime
            .confirm_owner_managed_result_delivery(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id,
                &intent.id,
                &"0".repeat(64)
            )
            .await
            .is_err()
    );
    assert!(peer.delivery_requests.lock().unwrap().is_empty());
    peer.delivery_mode.store(3, Ordering::SeqCst);
    assert!(matches!(
        confirm(&f, &runtime, &peer, &run, &intent).await,
        Err(Error::UncertainEffect)
    ));
    let current = history(&f, &runtime, &peer, &run).await;
    assert_eq!(current.state, ManagedResultDeliveryState::Committed);
    assert!(current.accepted.is_none());
    assert_eq!(prepare(&f, &runtime, &peer, &run).await, current);
    let recovered = confirm(&f, &runtime, &peer, &run, &intent).await.unwrap();
    assert_eq!(recovered.state, ManagedResultDeliveryState::Accepted);
    let trace = peer.delivery_requests.lock().unwrap().clone();
    assert_eq!(trace[0], trace[1]);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn cancellation_during_delivery_reply_keeps_signed_history_without_content_rights() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    ready(&f, &runtime, &peer, &run).await;
    let before = local_version(&f).await;
    let intent = prepare(&f, &runtime, &peer, &run).await;
    peer.delivery_mode.store(2, Ordering::SeqCst);
    let accepted = confirm(&f, &runtime, &peer, &run, &intent).await.unwrap();
    assert_eq!(accepted.state, ManagedResultDeliveryState::Accepted);
    assert_eq!(history(&f, &runtime, &peer, &run).await, accepted);
    peer.retirement_state.store(2, Ordering::SeqCst);
    let retired = runtime
        .refresh_owner_managed_result_retirement(
            f.order(),
            &run.id,
            &peer.launch.payload.process_attempt_id,
        )
        .await
        .unwrap();
    assert_eq!(retired.state, ManagedResultDeliveryState::Retired);
    assert!(
        runtime
            .owner_managed_result_manifest(f.order(), &run.id, &before.id)
            .await
            .is_err()
    );
    assert_eq!(local_version(&f).await, before);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn source_revocation_blocks_new_confirmation_but_not_original_accepted_history() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    ready(&f, &runtime, &peer, &run).await;
    let intent = prepare(&f, &runtime, &peer, &run).await;
    let accepted = confirm(&f, &runtime, &peer, &run, &intent).await.unwrap();
    let RunRequest::Single(request) = &f.input.request else {
        unreachable!()
    };
    let grant = request.folders.iter().find(|g| g.id == "source").unwrap();
    f.runtime
        .files
        .as_ref()
        .unwrap()
        .revoke_native_folder(&grant.project_id, &grant.id, &grant.revision)
        .unwrap();
    assert!(confirm(&f, &runtime, &peer, &run, &intent).await.is_err());
    assert_eq!(peer.delivery_requests.lock().unwrap().len(), 1);
    assert_eq!(history(&f, &runtime, &peer, &run).await, accepted);
    peer.retirement_state.store(2, Ordering::SeqCst);
    assert_eq!(
        runtime
            .refresh_owner_managed_result_retirement(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .unwrap()
            .state,
        ManagedResultDeliveryState::Retired
    );
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn retirement_history_is_monotone_and_never_retires_local_ready_bytes() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let id = ready(&f, &runtime, &peer, &run).await;
    let before = local_version(&f).await;
    let intent = prepare(&f, &runtime, &peer, &run).await;
    confirm(&f, &runtime, &peer, &run, &intent).await.unwrap();
    let pending = runtime
        .refresh_owner_managed_result_retirement(
            f.order(),
            &run.id,
            &peer.launch.payload.process_attempt_id,
        )
        .await
        .unwrap();
    assert_eq!(pending.retirement.as_ref().unwrap().payload.revision, 1);
    peer.retirement_state.store(3, Ordering::SeqCst);
    let uncertain = runtime
        .refresh_owner_managed_result_retirement(
            f.order(),
            &run.id,
            &peer.launch.payload.process_attempt_id,
        )
        .await
        .unwrap();
    assert_eq!(uncertain.state, ManagedResultDeliveryState::Unconfirmed);
    peer.retirement_state.store(1, Ordering::SeqCst);
    assert!(
        runtime
            .refresh_owner_managed_result_retirement(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .is_err()
    );
    assert_eq!(history(&f, &runtime, &peer, &run).await, uncertain);
    peer.retirement_state.store(4, Ordering::SeqCst);
    let retired = runtime
        .refresh_owner_managed_result_retirement(
            f.order(),
            &run.id,
            &peer.launch.payload.process_attempt_id,
        )
        .await
        .unwrap();
    assert_eq!(retired.state, ManagedResultDeliveryState::Retired);
    let queries = peer.retirement_queries.load(Ordering::SeqCst);
    assert_eq!(
        runtime
            .refresh_owner_managed_result_retirement(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .unwrap(),
        retired
    );
    assert_eq!(peer.retirement_queries.load(Ordering::SeqCst), queries);
    assert_eq!(ready(&f, &runtime, &peer, &run).await, id);
    assert_eq!(local_version(&f).await, before);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn only_confirmed_unaccepted_expiry_allows_an_explicit_new_delivery_intent() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    ready(&f, &runtime, &peer, &run).await;
    let intent = prepare(&f, &runtime, &peer, &run).await;
    peer.delivery_mode.store(4, Ordering::SeqCst);
    assert!(matches!(
        confirm(&f, &runtime, &peer, &run, &intent).await,
        Err(Error::UncertainEffect)
    ));
    // Actual original30s window; no host-clock mutation or timestamp rebinding.
    tokio::time::sleep(Duration::from_millis(
        intent.request.accept_before.saturating_sub(now()).max(0) as u64 + 1,
    ))
    .await;
    let expired = confirm(&f, &runtime, &peer, &run, &intent).await.unwrap();
    assert_eq!(expired.state, ManagedResultDeliveryState::Expired);
    assert_eq!(expired.request, intent.request);
    assert_eq!(peer.delivery_requests.lock().unwrap().len(), 2);
    let replacement = prepare(&f, &runtime, &peer, &run).await;
    assert_ne!(replacement.id, intent.id);
    assert_eq!(replacement.ordinal, 2);
    assert_eq!(replacement.result_version_id, intent.result_version_id);
    assert_eq!(
        peer.delivery_requests.lock().unwrap().len(),
        2,
        "Preparation must not send replacement"
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM owner_managed_result_deliveries")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "Original expired request must remain retained");
    peer.delivery_mode.store(0, Ordering::SeqCst);
    assert_eq!(
        confirm(&f, &runtime, &peer, &run, &replacement)
            .await
            .unwrap()
            .state,
        ManagedResultDeliveryState::Accepted
    );
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn owner_delivery_projection_binds_displayed_intent_and_preserves_history_after_cancel() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let id = ready(&f, &runtime, &peer, &run).await;
    let attempt = &peer.launch.payload.process_attempt_id;
    assert!(
        runtime
            .owner_managed_result_delivery_view(f.order(), &run.id, attempt)
            .await
            .unwrap()
            .delivery
            .is_none()
    );
    let view = runtime
        .prepare_owner_managed_result_delivery_view(f.order(), &run.id, attempt)
        .await
        .unwrap();
    assert_eq!(view.order_id, f.order());
    assert_eq!(view.run_id, run.id);
    assert_eq!(view.process_attempt_id, *attempt);
    let display = view.delivery.unwrap();
    assert_eq!(display.result_version_id, id);
    assert_eq!(
        display.state,
        cowork_contracts::OwnerManagedResultDeliveryState::Prepared
    );
    assert!(peer.delivery_requests.lock().unwrap().is_empty());
    let bad = cowork_contracts::ConfirmOwnerManagedResultDelivery {
        id: display.id.clone(),
        request_sha256: "0".repeat(64),
    };
    assert!(
        runtime
            .confirm_owner_managed_result_delivery_view(f.order(), &run.id, attempt, &bad)
            .await
            .is_err()
    );
    assert!(peer.delivery_requests.lock().unwrap().is_empty());
    let input = cowork_contracts::ConfirmOwnerManagedResultDelivery {
        id: display.id,
        request_sha256: display.request_sha256,
    };
    let accepted = runtime
        .confirm_owner_managed_result_delivery_view(f.order(), &run.id, attempt, &input)
        .await
        .unwrap();
    assert_eq!(
        accepted.delivery.as_ref().unwrap().state,
        cowork_contracts::OwnerManagedResultDeliveryState::Accepted
    );
    let encoded = serde_json::to_string(&accepted).unwrap();
    for forbidden in [
        "native_origin_sha256",
        "runtime_instance_sha256",
        "control_public_key",
        "runner_public_key",
        "authorization_sha256",
        "\"request\":",
        "\"signature\":",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "Private field leaked: {forbidden}"
        );
    }
    assert!(
        runtime
            .owner_managed_result_delivery_view("foreign-order", &run.id, attempt)
            .await
            .is_err()
    );
    runtime.store.cancel(&run.id).await.unwrap();
    assert!(
        runtime
            .owner_managed_results(f.order(), &run.id)
            .await
            .is_err()
    );
    let detail = runtime.store.detail(&run.id).await.unwrap();
    assert!(
        detail
            .process_attempts
            .iter()
            .any(|value| &value.id == attempt && value.run_id == run.id)
    );
    assert_eq!(
        runtime
            .owner_managed_result_delivery_view(f.order(), &run.id, attempt)
            .await
            .unwrap(),
        accepted
    );
    peer.retirement_state.store(2, Ordering::SeqCst);
    let retired = runtime
        .refresh_owner_managed_result_retirement_view(f.order(), &run.id, attempt)
        .await
        .unwrap();
    assert_eq!(
        retired.delivery.unwrap().state,
        cowork_contracts::OwnerManagedResultDeliveryState::Retired
    );
    assert_eq!(peer.delivery_requests.lock().unwrap().len(), 1);
    assert_eq!(local_version(&f).await.id, id);
    drop(runtime);
    drop(peer);
    f.close().await;
}

async fn recover_lost_ack_without_content_rights(revoke_source: bool) {
    let (f, runtime, peer, run, _) = prepared(true).await;
    ready(&f, &runtime, &peer, &run).await;
    let original_version = local_version(&f).await;
    let intent = prepare(&f, &runtime, &peer, &run).await;
    peer.delivery_mode.store(1, Ordering::SeqCst);
    assert!(matches!(
        confirm(&f, &runtime, &peer, &run, &intent).await,
        Err(Error::UncertainEffect)
    ));
    let original_ack = peer.delivery_accepted.lock().unwrap().clone().unwrap();
    assert_eq!(
        history(&f, &runtime, &peer, &run).await.state,
        ManagedResultDeliveryState::Committed
    );
    if revoke_source {
        let RunRequest::Single(request) = &f.input.request else {
            unreachable!()
        };
        let grant = request.folders.iter().find(|g| g.id == "source").unwrap();
        f.runtime
            .files
            .as_ref()
            .unwrap()
            .revoke_native_folder(&grant.project_id, &grant.id, &grant.revision)
            .unwrap();
    } else {
        runtime.store.cancel(&run.id).await.unwrap();
    }
    assert!(confirm(&f, &runtime, &peer, &run, &intent).await.is_err());
    assert!(
        runtime
            .owner_managed_results(f.order(), &run.id)
            .await
            .is_err()
    );
    let recovered = runtime
        .refresh_owner_managed_result_retirement(
            f.order(),
            &run.id,
            &peer.launch.payload.process_attempt_id,
        )
        .await
        .unwrap();
    assert_eq!(recovered.state, ManagedResultDeliveryState::Accepted);
    assert_eq!(recovered.accepted.as_ref(), Some(&original_ack));
    let sent = peer.delivery_requests.lock().unwrap().clone();
    let looked_up = peer.delivery_history_requests.lock().unwrap().clone();
    assert_eq!(
        sent.len(),
        1,
        "History lookup must not resend the admission request"
    );
    assert_eq!(
        looked_up, sent,
        "History lookup must use exactly the original signed request"
    );
    assert_eq!(
        peer.retirement_queries.load(Ordering::SeqCst),
        0,
        "One explicit call retrieves only the ACK"
    );
    peer.retirement_state.store(2, Ordering::SeqCst);
    assert_eq!(
        runtime
            .refresh_owner_managed_result_retirement(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .unwrap()
            .state,
        ManagedResultDeliveryState::Retired
    );
    assert_eq!(peer.delivery_requests.lock().unwrap().len(), 1);
    assert_eq!(local_version(&f).await, original_version);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn lost_delivery_ack_is_readable_after_source_revocation_without_new_acceptance() {
    recover_lost_ack_without_content_rights(true).await;
}

#[tokio::test]
async fn lost_delivery_ack_is_readable_after_cancellation_without_new_acceptance() {
    recover_lost_ack_without_content_rights(false).await;
}

#[tokio::test]
async fn missing_or_invalid_ack_history_cannot_expire_or_replace_committed_intent() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    ready(&f, &runtime, &peer, &run).await;
    let intent = prepare(&f, &runtime, &peer, &run).await;
    peer.delivery_mode.store(1, Ordering::SeqCst);
    assert!(confirm(&f, &runtime, &peer, &run, &intent).await.is_err());
    let committed = history(&f, &runtime, &peer, &run).await;
    for mode in [5, 6] {
        peer.delivery_mode.store(mode, Ordering::SeqCst);
        assert!(
            runtime
                .refresh_owner_managed_result_retirement(
                    f.order(),
                    &run.id,
                    &peer.launch.payload.process_attempt_id
                )
                .await
                .is_err()
        );
        assert_eq!(history(&f, &runtime, &peer, &run).await, committed);
        let same = prepare(&f, &runtime, &peer, &run).await;
        assert_eq!(same.id, intent.id);
        assert_eq!(same.request, intent.request);
    }
    assert_eq!(peer.delivery_requests.lock().unwrap().len(), 1);
    assert_eq!(peer.retirement_queries.load(Ordering::SeqCst), 0);
    drop(runtime);
    drop(peer);
    f.close().await;
}
