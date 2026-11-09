//! A current-fence observation for an original physically retained result.
//! No execution permission or restart adoption is issued by this route.
use super::*;

fn original_retention(
    service: &Service,
    published: &Published,
    request: &ManagedResultContextRequest,
) -> Answer<i64> {
    if published.retiring.load(Ordering::Acquire) {
        return Err(delivery::unconfirmed());
    }
    let attestation = published.attestation.as_ref().ok_or_else(uncertain)?;
    let snapshot = match &published.completion.output {
        ManagedOutputStatus::Ready(snapshot) => snapshot,
        ManagedOutputStatus::Rejected(_) => {
            return Err(ApiError(StatusCode::CONFLICT, "managed_result_unavailable"));
        }
    };
    let manifest = snapshot.manifest().map_err(|_| uncertain())?;
    let observed = now()?;
    attestation
        .verify(
            "managed_output",
            &service.0.profile.binding.runner_public_key,
        )
        .map_err(|_| denied())?;
    attestation
        .payload
        .validate(
            &published.grant.payload,
            &published.launch,
            &published.completion.receipt,
            &manifest,
            &service.0.broker.instance_fingerprint(),
            observed,
        )
        .map_err(|_| denied())?;
    if request.process_attempt_id != published.launch.payload.process_attempt_id
        || request.run_id != published.grant.payload.run_id
        || request.launch_envelope_sha256 != envelope_hash(&published.launch)
        || request.execution_receipt_sha256 != canonical_hash(&published.completion.receipt)
        || request.runtime_instance_sha256 != attestation.payload.runtime_instance_sha256
        || request.runner != published.grant.payload.runner
        || request.target != published.grant.payload.target
    {
        return Err(denied());
    }
    let retention = published.retention.as_ref().ok_or_else(uncertain)?;
    if !retention.current(observed, Instant::now())
        || !published
            .boot_retention
            .as_ref()
            .is_some_and(|fence| fence.current())
    {
        return Err(ApiError(
            StatusCode::GONE,
            "managed_result_retention_expired",
        ));
    }
    Ok(attestation.payload.retained_until)
}

pub(super) async fn observe(
    State(service): State<Service>,
    Path(id): Path<String>,
    Json(signed): Json<Signed<ManagedResultContextRequest>>,
) -> Answer<Json<Signed<ManagedResultReadContext>>> {
    if !identifier(&id) || signed.payload.process_attempt_id != id {
        return Err(denied());
    }
    signed
        .verify(
            "managed_result_context_request",
            &service.0.profile.binding.control_public_key,
        )
        .map_err(|_| denied())?;
    // This map retains only original native custody. Durable receipt bytes or
    // client-supplied context cannot reconstruct a Published entry after crash.
    let published = service
        .0
        .results
        .lock()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(uncertain)?;
    let request = signed.payload.clone();
    let retained_until = original_retention(&service, &published, &request)?;
    let observed = service
        .0
        .broker
        .observe_managed_result_context(signed, &service.0.profile.target, retained_until)
        .await
        .map_err(|error| broker_denied(&service, "managed_result_context_observe", &id, &error))?;
    original_retention(&service, &published, &request)?;
    observed
        .payload
        .validate(&request, &service.0.profile.binding, now()?)
        .map_err(|_| denied())?;
    Ok(Json(observed))
}
