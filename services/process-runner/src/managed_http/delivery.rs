//! History/status are strictly lookups. Neither can accept an intent or clean up.
use super::*;
use cowork_managed_runtime::containerd::retirement::ManagedRetirement;

pub(super) fn unconfirmed() -> ApiError {
    ApiError(StatusCode::CONFLICT, "managed_result_delivery_unconfirmed")
}

/// Original objects survive failed cleanup in this process-local custody map.
/// No serialized receipt, query or restart can construct this state.
pub(super) struct Custody {
    published: Option<Arc<Published>>,
    native: Option<ManagedRetirement>,
    delivery: Signed<ManagedResultDeliveryReceipt>,
    accepted_durable: bool,
    status: Signed<ManagedResultRetirementReceipt>,
    evidence: Option<serde_json::Value>,
    candidate: Option<(
        Signed<ManagedResultRetirementReceipt>,
        Option<serde_json::Value>,
    )>,
}
impl Custody {
    fn prepare(&mut self) -> anyhow::Result<bool> {
        if self.native.is_some() {
            return Ok(true);
        }
        let original = self
            .published
            .take()
            .ok_or_else(|| anyhow::anyhow!("Original delivery custody missing"))?;
        match Arc::try_unwrap(original) {
            Ok(original) => {
                self.native = Some(original.completion.into_retirement());
                Ok(true)
            }
            Err(original) => {
                self.published = Some(original);
                Ok(false)
            }
        }
    }
}

pub(super) async fn confirm(
    State(service): State<Service>,
    Path(attempt): Path<String>,
    Json(request): Json<Signed<ManagedResultDeliveryRequest>>,
) -> Answer<Json<Signed<ManagedResultDeliveryReceipt>>> {
    if !identifier(&attempt) || request.payload.process_attempt_id != attempt {
        return Err(denied());
    }
    let _admission = service.0.delivery_admission.lock().await;
    // Resolve the original journal first. Missing or replaced original storage
    // is uncertainty, never an expiry hint or permission for a replacement ACK.
    if let Some(original) = service
        .0
        .journal
        .existing(&request)
        .map_err(|_| unconfirmed())?
    {
        return Ok(Json(original));
    }
    let mut results = service.0.results.lock().await;
    let published = results.get(&attempt).cloned().ok_or_else(unconfirmed)?;
    if published.retiring.load(Ordering::Acquire) {
        return Err(unconfirmed());
    }
    let attestation = published.attestation.as_ref().ok_or_else(unconfirmed)?;
    let snapshot = match &published.completion.output {
        ManagedOutputStatus::Ready(snapshot) => snapshot,
        ManagedOutputStatus::Rejected(_) => return Err(unconfirmed()),
    };
    let manifest = snapshot.manifest().map_err(|_| unconfirmed())?;
    // Pure original provenance, including the real completed SQLite row. This
    // does not touch a fence, input/execution lease or dispatch authority.
    let floor = service
        .0
        .broker
        .validate_managed_result_delivery_origin(
            &request,
            &published.completion.receipt,
            attestation,
        )
        .await
        .map_err(|error| {
            broker_denied(&service, "managed_result_delivery_origin", &attempt, &error)
        })?;
    let observed = now()?;
    let original = &request.payload;
    let input = &published.grant.payload;
    if observed < floor || observed < original.issued_at {
        return Err(unconfirmed());
    }
    if original.execution_plan_id != input.execution_plan_id
        || original.execution_plan_sha256 != input.execution_plan_sha256
        || original.run_id != input.run_id
        || original.runner != input.runner
        || original.target != input.target
        || original.manifest_sha256 != manifest.sha256
        || attestation.payload.input_manifest_sha256 != input.input_manifest_sha256
        || attestation.payload.physical_record_sha256
            != published.completion.physical.physical_record_sha256
    {
        return Err(denied());
    }
    // Even an expired intent receives that classification only after the
    // original journal, live instance, published objects and provenance match.
    if let Some(accepted) = delivery_window_after_origin(&service.0.journal, &request, observed)? {
        return Ok(Json(accepted));
    }
    original
        .validate(
            &service.0.profile.binding,
            &service.0.profile.target,
            &service.0.broker.instance_fingerprint(),
            observed,
        )
        .map_err(|_| denied())?;
    let receipt = service
        .0
        .signer
        .sign(
            "managed_result_delivery_receipt",
            ManagedResultDeliveryReceipt {
                format_version: 1,
                request,
                accepted_at: observed,
            },
        )
        .map_err(|_| unconfirmed())?;
    let pending = service
        .0
        .signer
        .sign(
            "managed_result_retirement_receipt",
            ManagedResultRetirementReceipt {
                format_version: 1,
                delivery_receipt_sha256: canonical_hash(&receipt),
                process_attempt_id: attempt.clone(),
                run_id: input.run_id.clone(),
                runtime_instance_sha256: service.0.broker.instance_fingerprint(),
                revision: 1,
                state: ManagedResultRetirementState::Pending,
                recorded_at: observed,
                physical_completion_sha256: attestation.payload.physical_record_sha256.clone(),
                retirement_evidence_sha256: None,
            },
        )
        .map_err(|_| unconfirmed())?;
    let mut custodians = service.0.retirements.lock().map_err(|_| unconfirmed())?;
    if custodians.contains_key(&attempt) {
        return Err(unconfirmed());
    }
    // Hold original native custody BEFORE the first irreversible write. Both
    // success and lost write completion hand this same context to its worker;
    // no later query reconstructs cleanup from an accepted JSON record.
    let custody = Arc::new(std::sync::Mutex::new(Custody {
        published: Some(published.clone()),
        native: None,
        delivery: receipt.clone(),
        accepted_durable: false,
        status: pending.clone(),
        evidence: None,
        candidate: None,
    }));
    custodians.insert(attempt.clone(), custody.clone());
    let accepted = service.0.journal.accept(receipt, pending);
    if let Ok(original) = &accepted
        && !original.newly_accepted
    {
        // A historical duplicate is only history, never a new cleanup permit.
        custodians.remove(&attempt);
        return Ok(Json(original.receipt.clone()));
    }
    // No await/cancellation point between the attempted write and the owned
    // continuation. An uncertain reply retains every original native object.
    if let Ok(mut original) = custody.lock() {
        original.accepted_durable = accepted.is_ok();
    }
    published.retiring.store(true, Ordering::Release);
    drop(results.remove(&attempt));
    drop(published);
    drop(custodians);
    drop(results);
    let finished_service = service.clone();
    tokio::spawn(async move {
        retry_cleanup(
            || {
                let original = custody.clone();
                let worker_service = finished_service.clone();
                async move {
                    matches!(
                        tokio::task::spawn_blocking(move || {
                            retire_original(&worker_service, &original)
                        })
                        .await,
                        Ok(Ok(true))
                    )
                }
            },
            Duration::from_millis(100),
        )
        .await;
        if let Ok(mut held) = finished_service.0.retirements.lock() {
            held.remove(&attempt);
        }
    });
    accepted
        .map(|value| Json(value.receipt))
        .map_err(|_| unconfirmed())
}

fn retire_original(service: &Service, custody: &std::sync::Mutex<Custody>) -> anyhow::Result<bool> {
    let mut custody = custody
        .lock()
        .map_err(|_| anyhow::anyhow!("Original retirement custody poisoned"))?;
    if !custody.accepted_durable {
        service
            .0
            .journal
            .reobserve_acceptance(&custody.delivery, &custody.status)?;
        custody.accepted_durable = true;
    }
    // A lost successful write must retry the EXACT original signed candidate,
    // including its original timestamp. No equivalent revision is re-signed.
    if let Some((status, evidence)) = custody.candidate.as_ref() {
        service.0.journal.record_status(status, evidence.clone())?;
        let (status, evidence) = custody.candidate.take().unwrap();
        custody.status = status;
        custody.evidence = evidence;
    }
    if custody.status.payload.state == ManagedResultRetirementState::Retired {
        // Reobserve the final original durable record on every GC retry. This
        // idempotent call does no write and never reconstructs native custody.
        service
            .0
            .journal
            .record_status(&custody.status, custody.evidence.clone())?;
        custody
            .native
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Original native retirement missing"))?
            .collect_metadata()?;
        return Ok(true);
    }
    // Existing readers check the retiring fence after their final await. No new
    // reader can clone this removed Published entry. Only those originals drain.
    let mut prepared = false;
    for _ in 0..100 {
        if custody.prepare()? {
            prepared = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let evidence = if prepared {
        custody
            .native
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Original native retirement missing"))?
            .retire()
            .map(serde_json::to_value)
            .map_err(anyhow::Error::from)
            .and_then(|value| value.map_err(anyhow::Error::from))
    } else {
        Err(anyhow::anyhow!("Original result readers remain busy"))
    };
    let previous_time = custody.status.payload.recorded_at;
    let mut status = custody.status.payload.clone();
    status.revision = status
        .revision
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("Retirement revision overflow"))?;
    status.recorded_at = now().map_err(|_| anyhow::anyhow!("Retirement clock unavailable"))?;
    anyhow::ensure!(
        status.recorded_at >= previous_time,
        "Retirement clock moved backwards"
    );
    let evidence = match evidence {
        Ok(evidence) => {
            status.state = ManagedResultRetirementState::Retired;
            status.retirement_evidence_sha256 = Some(canonical_hash(&evidence));
            Some(evidence)
        }
        Err(error) => {
            let _ = service.0.diagnostics.record(
                "managed_result_retirement",
                &status.process_attempt_id,
                &error,
                serde_json::Value::Null,
            );
            if custody.status.payload.state == ManagedResultRetirementState::Unconfirmed {
                return Ok(false);
            }
            status.state = ManagedResultRetirementState::Unconfirmed;
            None
        }
    };
    let retired = evidence.is_some();
    let signed = service
        .0
        .signer
        .sign("managed_result_retirement_receipt", status)?;
    // Fixed frame fsync AND exact readback complete before metadata unlink.
    // Any failure leaves the original native capability in the custody map.
    custody.candidate = Some((signed, evidence));
    let (signed, evidence) = custody.candidate.as_ref().unwrap();
    service.0.journal.record_status(signed, evidence.clone())?;
    let (signed, evidence) = custody.candidate.take().unwrap();
    custody.status = signed;
    custody.evidence = evidence;
    if retired {
        custody
            .native
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Original native retirement missing"))?
            .collect_metadata()?;
    }
    Ok(retired)
}

/// A single independently owned worker retains the same captured capability
/// across transient failures. Each blocking attempt is separate; delays are
/// asynchronous so service shutdown can stop this worker without redispatch.
async fn retry_cleanup<F, Fut>(mut attempt: F, mut backoff: Duration)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    loop {
        if attempt().await {
            return;
        }
        tokio::time::sleep(backoff).await;
        backoff = backoff.saturating_mul(2).min(Duration::from_secs(30));
    }
}

#[cfg(test)]
#[path = "delivery_worker_tests.rs"]
mod worker_tests;

pub(super) async fn history(
    State(service): State<Service>,
    Path(attempt): Path<String>,
    Json(request): Json<Signed<ManagedResultDeliveryRequest>>,
) -> Answer<Json<Signed<ManagedResultDeliveryReceipt>>> {
    if !identifier(&attempt) || request.payload.process_attempt_id != attempt {
        return Err(denied());
    }
    let receipt = service
        .0
        .journal
        .existing(&request)
        .map_err(|_| ApiError(StatusCode::CONFLICT, "managed_result_delivery_unconfirmed"))?
        .ok_or(ApiError(
            StatusCode::CONFLICT,
            "managed_result_delivery_unconfirmed",
        ))?;
    Ok(Json(receipt))
}

pub(super) async fn status(
    State(service): State<Service>,
    Path(attempt): Path<String>,
    Json(query): Json<Signed<ManagedResultRetirementQuery>>,
) -> Answer<Json<Signed<ManagedResultRetirementReceipt>>> {
    if !identifier(&attempt) || query.payload.process_attempt_id != attempt {
        return Err(denied());
    }
    let status = service.0.journal.status(&query, now()?).map_err(|_| {
        ApiError(
            StatusCode::CONFLICT,
            "managed_result_retirement_unconfirmed",
        )
    })?;
    Ok(Json(status))
}
