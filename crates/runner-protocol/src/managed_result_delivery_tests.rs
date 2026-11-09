use super::*;
use cowork_contracts::{ExecutionRunnerIdentity, ExecutionTargetRef};

struct Keys {
    control: SigningKey,
    runner: SigningKey,
    binding: Binding,
    request: ManagedResultDeliveryRequest,
}

fn keys() -> Keys {
    let control = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let runner = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let identity = ExecutionRunnerIdentity {
        runner_id: "delivery-test-runner".into(),
        generation: 7,
        adapter_build_sha256: "a".repeat(64),
        control_public_key: control.public_key(),
        runner_public_key: runner.public_key(),
    };
    let binding = Binding {
        runner_id: identity.runner_id.clone(),
        generation: identity.generation,
        adapter_build_sha256: identity.adapter_build_sha256.clone(),
        control_public_key: identity.control_public_key.clone(),
        runner_public_key: identity.runner_public_key.clone(),
    };
    let target = ExecutionTargetRef {
        target_id: cowork_domain::execution_targets::execution_target_id(&identity).unwrap(),
        revision: 1,
        identity_sha256: cowork_domain::execution_targets::runner_identity_hash(&identity).unwrap(),
    };
    let request = ManagedResultDeliveryRequest {
        format_version: 1,
        request_id: uuid::Uuid::new_v4().to_string(),
        actor_id: "owner".into(),
        authorization_sha256: "b".repeat(64),
        runtime_instance_sha256: "c".repeat(64),
        execution_plan_id: "plan-1".into(),
        execution_plan_sha256: "d".repeat(64),
        run_id: "run-1".into(),
        process_attempt_id: "attempt-1".into(),
        launch_envelope_sha256: "e".repeat(64),
        execution_receipt_sha256: "f".repeat(64),
        output_attestation_sha256: "1".repeat(64),
        manifest_sha256: "2".repeat(64),
        recipient_version_id: uuid::Uuid::new_v4().to_string(),
        runner: identity,
        target,
        issued_at: 1_000,
        accept_before: 31_000,
    };
    Keys {
        control,
        runner,
        binding,
        request,
    }
}

fn accepted(k: &Keys) -> Signed<ManagedResultDeliveryReceipt> {
    let request = k
        .control
        .sign("managed_result_delivery_request", k.request.clone())
        .unwrap();
    k.runner
        .sign(
            "managed_result_delivery_receipt",
            ManagedResultDeliveryReceipt {
                format_version: 1,
                request,
                accepted_at: 1_001,
            },
        )
        .unwrap()
}

#[test]
fn accepted_delivery_is_history_after_its_first_acceptance_window() {
    let k = keys();
    let delivery = accepted(&k);
    delivery
        .verify(
            "managed_result_delivery_receipt",
            &k.binding.runner_public_key,
        )
        .unwrap();
    delivery
        .payload
        .validate(&delivery.payload.request, &k.binding)
        .unwrap();
    assert!(matches!(
        k.request.validate(
            &k.binding,
            &k.request.target,
            &k.request.runtime_instance_sha256,
            k.request.accept_before
        ),
        Err(Error::Expired)
    ));
    // Historical validation neither samples a new clock nor refreshes bytes.
    let original = serde_json::to_vec(&delivery).unwrap();
    delivery
        .payload
        .validate(&delivery.payload.request, &k.binding)
        .unwrap();
    assert_eq!(serde_json::to_vec(&delivery).unwrap(), original);
    assert!(
        delivery
            .verify("receipt", &k.binding.runner_public_key)
            .is_err()
    );
    assert!(
        delivery
            .payload
            .request
            .verify("launch", &k.binding.control_public_key)
            .is_err()
    );
    let mut late = delivery.payload.clone();
    late.accepted_at = k.request.accept_before;
    assert!(late.validate(&late.request, &k.binding).is_err());
    let mut changed = delivery.payload.request.clone();
    changed.payload.recipient_version_id = uuid::Uuid::new_v4().to_string();
    assert!(delivery.payload.validate(&changed, &k.binding).is_err());
}

#[test]
fn first_delivery_admission_rejects_altered_identity_provenance_and_window() {
    let k = keys();
    let validate = |r: &ManagedResultDeliveryRequest| {
        r.validate(
            &k.binding,
            &k.request.target,
            &k.request.runtime_instance_sha256,
            1_001,
        )
    };
    validate(&k.request).unwrap();
    let mut wrong = k.request.clone();
    wrong.runtime_instance_sha256 = "0".repeat(64);
    assert!(matches!(validate(&wrong), Err(Error::Authority)));
    let mut wrong = k.request.clone();
    wrong.runner.generation += 1;
    assert!(matches!(validate(&wrong), Err(Error::Authority)));
    let mut wrong = k.request.clone();
    wrong.target.revision += 1;
    assert!(matches!(validate(&wrong), Err(Error::Authority)));
    let mut wrong = k.request.clone();
    wrong.output_attestation_sha256.clear();
    assert!(matches!(validate(&wrong), Err(Error::Authority)));
    let mut wrong = k.request.clone();
    wrong.actor_id = "unbounded actor/authority".into();
    assert!(matches!(validate(&wrong), Err(Error::Authority)));
    let mut wrong = k.request.clone();
    wrong.recipient_version_id = "noncanonical-local-path".into();
    assert!(matches!(validate(&wrong), Err(Error::Authority)));
    let mut wrong = k.request.clone();
    wrong.accept_before += 1;
    assert!(matches!(validate(&wrong), Err(Error::Expired)));
    let mut wrong = k.request.clone();
    wrong.issued_at = 1_002;
    assert!(matches!(validate(&wrong), Err(Error::Expired)));
}

#[test]
fn retirement_cannot_claim_release_without_original_delivery_and_physical_evidence() {
    let k = keys();
    let delivery = accepted(&k);
    let physical = "3".repeat(64);
    let pending = ManagedResultRetirementReceipt {
        format_version: 1,
        delivery_receipt_sha256: canonical_hash(&delivery),
        process_attempt_id: k.request.process_attempt_id.clone(),
        run_id: k.request.run_id.clone(),
        runtime_instance_sha256: k.request.runtime_instance_sha256.clone(),
        revision: 1,
        state: ManagedResultRetirementState::Pending,
        recorded_at: 1_001,
        physical_completion_sha256: physical.clone(),
        retirement_evidence_sha256: None,
    };
    pending.validate(&delivery, &k.binding, &physical).unwrap();
    let mut retired = pending.clone();
    retired.revision = 2;
    retired.state = ManagedResultRetirementState::Retired;
    assert!(retired.validate(&delivery, &k.binding, &physical).is_err());
    retired.retirement_evidence_sha256 = Some("4".repeat(64));
    retired.validate(&delivery, &k.binding, &physical).unwrap();
    let signed = k
        .runner
        .sign("managed_result_retirement_receipt", retired.clone())
        .unwrap();
    signed
        .verify(
            "managed_result_retirement_receipt",
            &k.binding.runner_public_key,
        )
        .unwrap();
    assert!(
        signed
            .verify(
                "managed_result_delivery_receipt",
                &k.binding.runner_public_key
            )
            .is_err()
    );
    assert!(
        retired
            .validate(&delivery, &k.binding, &"5".repeat(64))
            .is_err()
    );
    let mut changed = retired.clone();
    changed.delivery_receipt_sha256 = "0".repeat(64);
    assert!(changed.validate(&delivery, &k.binding, &physical).is_err());
    let mut changed = retired.clone();
    changed.process_attempt_id = "another-attempt".into();
    assert!(changed.validate(&delivery, &k.binding, &physical).is_err());
    let mut changed = retired.clone();
    changed.recorded_at = delivery.payload.accepted_at - 1;
    assert!(changed.validate(&delivery, &k.binding, &physical).is_err());
    let mut uncertain = pending.clone();
    uncertain.state = ManagedResultRetirementState::Unconfirmed;
    uncertain.revision = 2;
    uncertain
        .validate(&delivery, &k.binding, &physical)
        .unwrap();
    let mut regressed = pending;
    regressed.revision = 3;
    assert!(
        regressed
            .validate(&delivery, &k.binding, &physical)
            .is_err()
    );
}

#[test]
fn retirement_query_is_short_lived_and_bound_to_the_original_accepted_receipt() {
    let k = keys();
    let delivery = accepted(&k);
    let query = ManagedResultRetirementQuery {
        format_version: 1,
        request_id: uuid::Uuid::new_v4().to_string(),
        delivery_receipt_sha256: canonical_hash(&delivery),
        process_attempt_id: k.request.process_attempt_id.clone(),
        runner: k.request.runner.clone(),
        target: k.request.target.clone(),
        issued_at: 50_000,
        expires_at: 80_000,
    };
    query.validate(&delivery, &k.binding, 50_001).unwrap();
    assert!(matches!(
        query.validate(&delivery, &k.binding, query.expires_at),
        Err(Error::Expired)
    ));
    let signed = k
        .control
        .sign("managed_result_retirement_query", query.clone())
        .unwrap();
    signed
        .verify(
            "managed_result_retirement_query",
            &k.binding.control_public_key,
        )
        .unwrap();
    assert!(
        signed
            .verify(
                "managed_result_delivery_request",
                &k.binding.control_public_key
            )
            .is_err()
    );
    let mut wrong = query;
    wrong.delivery_receipt_sha256 = "0".repeat(64);
    assert!(wrong.validate(&delivery, &k.binding, 50_001).is_err());
}

#[test]
fn delivery_wire_records_cannot_smuggle_native_or_extra_authority_fields() {
    let k = keys();
    let mut value = serde_json::to_value(&k.request).unwrap();
    value["native_device_id"] = serde_json::json!(123);
    assert!(serde_json::from_value::<ManagedResultDeliveryRequest>(value).is_err());
    let mut value = serde_json::to_value(&k.request).unwrap();
    value.as_object_mut().unwrap().remove("manifest_sha256");
    assert!(serde_json::from_value::<ManagedResultDeliveryRequest>(value).is_err());
    let mut value = serde_json::to_value(accepted(&k)).unwrap();
    value["payload"]["execution_authority"] = serde_json::json!(true);
    assert!(serde_json::from_value::<Signed<ManagedResultDeliveryReceipt>>(value).is_err());
}

async fn completed_delivery_origin(
    f: &Fixture,
) -> (
    Signed<ManagedResultDeliveryRequest>,
    Signed<Receipt>,
    Signed<ManagedOutputAttestation>,
) {
    let (_, read) = result_read_request(f).await;
    let original = f
        .broker
        .receipt(&read.payload.process_attempt_id)
        .await
        .unwrap();
    let at = original.payload.updated_at;
    let output = f
        .runner
        .sign(
            "managed_output",
            ManagedOutputAttestation {
                format_version: 1,
                runtime_instance_sha256: read.payload.runtime_instance_sha256.clone(),
                execution_plan_id: read.payload.execution_plan_id.clone(),
                execution_plan_sha256: read.payload.execution_plan_sha256.clone(),
                process_attempt_id: read.payload.process_attempt_id.clone(),
                run_id: read.payload.run_id.clone(),
                launch_envelope_sha256: read.payload.launch_envelope_sha256.clone(),
                input_manifest_sha256: "6".repeat(64),
                output_manifest_sha256: "7".repeat(64),
                execution_receipt_sha256: canonical_hash(&original),
                physical_record_sha256: "8".repeat(64),
                runner: read.payload.runner.clone(),
                target: read.payload.target.clone(),
                produced_at: at,
                retained_until: at + 60_000,
            },
        )
        .unwrap();
    let request = f
        .control
        .sign(
            "managed_result_delivery_request",
            ManagedResultDeliveryRequest {
                format_version: 1,
                request_id: uuid::Uuid::new_v4().to_string(),
                actor_id: "owner".into(),
                authorization_sha256: "9".repeat(64),
                runtime_instance_sha256: read.payload.runtime_instance_sha256,
                execution_plan_id: read.payload.execution_plan_id,
                execution_plan_sha256: read.payload.execution_plan_sha256,
                run_id: read.payload.run_id,
                process_attempt_id: read.payload.process_attempt_id,
                launch_envelope_sha256: read.payload.launch_envelope_sha256,
                execution_receipt_sha256: canonical_hash(&original),
                output_attestation_sha256: canonical_hash(&output),
                manifest_sha256: output.payload.output_manifest_sha256.clone(),
                recipient_version_id: uuid::Uuid::new_v4().to_string(),
                runner: read.payload.runner,
                target: read.payload.target,
                issued_at: at,
                accept_before: at + 30_000,
            },
        )
        .unwrap();
    (request, original, output)
}

#[tokio::test]
async fn delivery_origin_observation_preserves_execution_fences_leases_and_clock() {
    let f = fixture().await;
    let (request, original, output) = completed_delivery_origin(&f).await;
    let execution: String = sqlx::query_scalar("SELECT payload FROM runner_executions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let floor: i64 = sqlx::query_scalar("SELECT high_water FROM runner_binding")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let fence_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runner_fences")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let lease_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM runner_execution_lease_renewals")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let observed = f
        .broker
        .validate_managed_result_delivery_origin(&request, &original, &output)
        .await
        .unwrap();
    assert_eq!(observed, floor);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT payload FROM runner_executions")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        execution
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT high_water FROM runner_binding")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        floor
    );
    for (table, expected) in [
        ("runner_fences", fence_count),
        ("runner_execution_lease_renewals", lease_count),
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, expected);
    }
    f.pool.close().await;
}

#[tokio::test]
async fn missing_original_delivery_journal_is_uncertain_and_never_an_expiry_hint() {
    let f = fixture().await;
    let (request, original, output) = completed_delivery_origin(&f).await;
    sqlx::query("DELETE FROM runner_executions")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(matches!(
        f.broker
            .validate_managed_result_delivery_origin(&request, &original, &output)
            .await,
        Err(Error::Uncertain)
    ));
    f.pool.close().await;
}

#[tokio::test]
async fn changed_original_instance_or_signed_output_cannot_be_delivery_origin() {
    let f = fixture().await;
    let (request, original, output) = completed_delivery_origin(&f).await;
    let mut old_instance = request.payload.clone();
    old_instance.runtime_instance_sha256 = "0".repeat(64);
    let old_instance = f
        .control
        .sign("managed_result_delivery_request", old_instance)
        .unwrap();
    assert!(matches!(
        f.broker
            .validate_managed_result_delivery_origin(&old_instance, &original, &output)
            .await,
        Err(Error::Authority)
    ));
    let mut changed = output.clone();
    changed.payload.physical_record_sha256 = "0".repeat(64);
    assert!(matches!(
        f.broker
            .validate_managed_result_delivery_origin(&request, &original, &changed)
            .await,
        Err(Error::Authority)
    ));
    let mut early = request.payload.clone();
    early.issued_at = original.payload.updated_at - 1;
    let early = f
        .control
        .sign("managed_result_delivery_request", early)
        .unwrap();
    assert!(matches!(
        f.broker
            .validate_managed_result_delivery_origin(&early, &original, &output)
            .await,
        Err(Error::Authority)
    ));
    f.pool.close().await;
}
