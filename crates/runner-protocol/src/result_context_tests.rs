use super::*;
use crate::broker::result_clock::ResultReadClock;
use std::time::{Duration, Instant};

fn clock(instant: Instant) -> ResultReadClock {
    ResultReadClock {
        instant,
        boot_ns: None,
    }
}

fn observation_request(read: &ManagedResultReadRequest) -> ManagedResultContextRequest {
    let now = chrono::Utc::now().timestamp_millis();
    ManagedResultContextRequest {
        format_version: 1,
        request_id: uuid::Uuid::new_v4().to_string(),
        runtime_instance_sha256: read.runtime_instance_sha256.clone(),
        process_attempt_id: read.process_attempt_id.clone(),
        run_id: read.run_id.clone(),
        launch_envelope_sha256: read.launch_envelope_sha256.clone(),
        execution_receipt_sha256: read.execution_receipt_sha256.clone(),
        runner: read.runner.clone(),
        target: read.target.clone(),
        issued_at: now,
        expires_at: now + MANAGED_RESULT_READ_MILLIS,
    }
}

async fn observe(
    f: &Fixture,
    request: &ManagedResultContextRequest,
) -> Signed<ManagedResultReadContext> {
    f.broker
        .observe_managed_result_context(
            f.control
                .sign("managed_result_context_request", request.clone())
                .unwrap(),
            &request.target,
            request.expires_at,
        )
        .await
        .unwrap()
}

fn fresh_read(
    f: &Fixture,
    original: &ManagedResultReadRequest,
    context: Signed<ManagedResultReadContext>,
) -> Signed<ManagedResultReadRequest> {
    let mut read = original.clone();
    read.format_version = 2;
    read.request_id = uuid::Uuid::new_v4().to_string();
    read.issued_at = chrono::Utc::now().timestamp_millis();
    read.expires_at = context.payload.expires_at;
    read.context = Some(context);
    f.control.sign("managed_result_read", read).unwrap()
}

async fn set_fence(f: &Fixture, run: &str, epoch: u64, revoked: bool) {
    f.broker
        .fence(
            f.control
                .sign(
                    "fence",
                    FenceRequest {
                        runner_id: f.binding.runner_id.clone(),
                        runner_generation: f.binding.generation,
                        run_id: run.into(),
                        lease_epoch: epoch,
                        revoked,
                    },
                )
                .unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn fresh_result_context_reads_revoked_original_without_execution_mutations() {
    let f = fixture().await;
    let (target, legacy) = result_read_request(&f).await;
    let original = f
        .broker
        .receipt(&legacy.payload.process_attempt_id)
        .await
        .unwrap();
    set_fence(
        &f,
        &legacy.payload.run_id,
        original.payload.request.lease_epoch,
        true,
    )
    .await;
    let fence_before: String = sqlx::query_scalar("SELECT payload FROM runner_fences")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(
        f.broker
            .authorize_managed_result_read(legacy.clone(), &target)
            .await
            .is_err()
    );
    let request = observation_request(&legacy.payload);
    let context = observe(&f, &request).await;
    context
        .verify("managed_result_context", &f.binding.runner_public_key)
        .unwrap();
    assert_eq!(
        context,
        observe(&f, &request).await,
        "Exact replay retains original signature/times"
    );
    let read = fresh_read(&f, &legacy.payload, context);
    let permit = f
        .broker
        .authorize_managed_result_read(read, &target)
        .await
        .unwrap();
    f.broker
        .consume_managed_result_read(permit, &target)
        .await
        .unwrap();
    assert_eq!(
        original,
        f.broker
            .receipt(&legacy.payload.process_attempt_id)
            .await
            .unwrap()
    );
    assert_eq!(
        fence_before,
        sqlx::query_scalar::<_, String>("SELECT payload FROM runner_fences")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    );
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runner_executions")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    );
    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runner_execution_lease_renewals")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn fence_changes_invalidate_old_contexts_and_prepared_bytes_but_fresh_context_can_read() {
    let f = fixture().await;
    let (target, legacy) = result_read_request(&f).await;
    let original = f
        .broker
        .receipt(&legacy.payload.process_attempt_id)
        .await
        .unwrap();
    let request = observation_request(&legacy.payload);
    let context = observe(&f, &request).await;
    let read = fresh_read(&f, &legacy.payload, context);
    let permit = f
        .broker
        .authorize_managed_result_read(read.clone(), &target)
        .await
        .unwrap();
    set_fence(
        &f,
        &legacy.payload.run_id,
        original.payload.request.lease_epoch,
        true,
    )
    .await;
    assert!(matches!(
        f.broker.consume_managed_result_read(permit, &target).await,
        Err(Error::Authority)
    ));
    assert!(
        f.broker
            .authorize_managed_result_read(read.clone(), &target)
            .await
            .is_err()
    );
    assert!(matches!(
        f.broker
            .observe_managed_result_context(
                f.control
                    .sign("managed_result_context_request", request)
                    .unwrap(),
                &target,
                legacy.payload.expires_at,
            )
            .await,
        Err(Error::Conflict)
    ));
    let stopped = observe(&f, &observation_request(&legacy.payload)).await;
    let stopped_read = fresh_read(&f, &legacy.payload, stopped);
    let permit = f
        .broker
        .authorize_managed_result_read(stopped_read.clone(), &target)
        .await
        .unwrap();
    set_fence(
        &f,
        &legacy.payload.run_id,
        original.payload.request.lease_epoch + 1,
        false,
    )
    .await;
    assert!(
        f.broker
            .consume_managed_result_read(permit, &target)
            .await
            .is_err()
    );
    assert!(
        f.broker
            .authorize_managed_result_read(stopped_read, &target)
            .await
            .is_err()
    );
    assert!(
        f.broker
            .authorize_managed_result_read(legacy.clone(), &target)
            .await
            .is_err()
    );
    let later = observe(&f, &observation_request(&legacy.payload)).await;
    let permit = f
        .broker
        .authorize_managed_result_read(fresh_read(&f, &legacy.payload, later), &target)
        .await
        .unwrap();
    f.broker
        .consume_managed_result_read(permit, &target)
        .await
        .unwrap();
    assert_eq!(
        original,
        f.broker
            .receipt(&legacy.payload.process_attempt_id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn context_is_instance_purpose_and_original_receipt_bound_and_cannot_create_a_fence() {
    let f = fixture().await;
    let (target, legacy) = result_read_request(&f).await;
    let request = observation_request(&legacy.payload);
    for purpose in [
        "launch",
        "receipt",
        "managed_result_context",
        "managed_result_read",
    ] {
        assert!(
            f.broker
                .observe_managed_result_context(
                    f.control.sign(purpose, request.clone()).unwrap(),
                    &target,
                    request.expires_at,
                )
                .await
                .is_err()
        );
    }
    assert!(
        f.broker
            .observe_managed_result_context(
                f.runner
                    .sign("managed_result_context_request", request.clone())
                    .unwrap(),
                &target,
                request.expires_at,
            )
            .await
            .is_err()
    );
    for field in ["attempt", "run", "receipt", "envelope", "instance"] {
        let mut wrong = request.clone();
        match field {
            "attempt" => wrong.process_attempt_id = "missing-original".into(),
            "run" => wrong.run_id = "other-run".into(),
            "receipt" => wrong.execution_receipt_sha256 = "0".repeat(64),
            "envelope" => wrong.launch_envelope_sha256 = "0".repeat(64),
            _ => wrong.runtime_instance_sha256 = "0".repeat(64),
        }
        assert!(
            f.broker
                .observe_managed_result_context(
                    f.control
                        .sign("managed_result_context_request", wrong)
                        .unwrap(),
                    &target,
                    request.expires_at,
                )
                .await
                .is_err(),
            "{field}"
        );
    }
    let context = observe(&f, &request).await;
    let permit = f
        .broker
        .authorize_managed_result_read(fresh_read(&f, &legacy.payload, context.clone()), &target)
        .await
        .unwrap();
    let reopened = Broker::open(pool(&f.url).await, f.binding.clone(), f.runner.clone())
        .await
        .unwrap();
    assert!(
        reopened
            .consume_managed_result_read(permit, &target)
            .await
            .is_err()
    );
    assert!(
        reopened
            .authorize_managed_result_read(fresh_read(&f, &legacy.payload, context), &target)
            .await
            .is_err()
    );
    assert!(
        reopened
            .observe_managed_result_context(
                f.control
                    .sign("managed_result_context_request", request.clone())
                    .unwrap(),
                &target,
                request.expires_at,
            )
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM runner_fences")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        f.broker
            .observe_managed_result_context(
                f.control
                    .sign("managed_result_context_request", request.clone())
                    .unwrap(),
                &target,
                request.expires_at,
            )
            .await
            .is_err()
    );
    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runner_fences")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn context_original_monotonic_deadline_cannot_reset_after_tentative_future_or_failed_commit()
{
    let f = fixture().await;
    let (_, legacy) = result_read_request(&f).await;
    let request = observation_request(&legacy.payload);
    let anchor = Instant::now();
    let issued = request.issued_at;
    let (context, deadline) = f
        .broker
        .retain_managed_result_context(
            request.clone(),
            "a".repeat(64),
            issued,
            clock(anchor),
            0,
            request.expires_at,
        )
        .unwrap();
    assert_eq!(
        deadline,
        f.broker
            .retain_managed_result_context(
                request.clone(),
                "a".repeat(64),
                issued + 1,
                clock(anchor + Duration::from_secs(25)),
                0,
                request.expires_at,
            )
            .unwrap()
            .1
    );
    let mut future = request.clone();
    future.request_id = uuid::Uuid::new_v4().to_string();
    future.issued_at += 31000;
    future.expires_at += 31000;
    f.broker
        .retain_managed_result_context(
            future.clone(),
            "a".repeat(64),
            future.issued_at,
            clock(anchor + Duration::from_secs(31)),
            0,
            future.expires_at,
        )
        .unwrap();
    assert!(matches!(
        f.broker.retain_managed_result_context(
            request.clone(),
            "a".repeat(64),
            issued + 2,
            clock(anchor + Duration::from_secs(31)),
            0,
            request.expires_at,
        ),
        Err(Error::Expired)
    ));
    assert!(matches!(
        f.broker.managed_result_context_deadline(
            &context,
            issued + 2,
            clock(anchor + Duration::from_secs(31)),
        ),
        Err(Error::Expired)
    ));
    let mut changed = request.clone();
    changed.execution_receipt_sha256 = "b".repeat(64);
    assert!(matches!(
        f.broker.retain_managed_result_context(
            changed,
            "a".repeat(64),
            issued + 2,
            clock(anchor),
            0,
            request.expires_at,
        ),
        Err(Error::Conflict)
    ));
}

#[tokio::test]
async fn v2_requires_exact_runner_context_and_preserves_legacy_serialization() {
    let f = fixture().await;
    let (target, legacy) = result_read_request(&f).await;
    let bytes = serde_json::to_vec(&legacy).unwrap();
    assert!(
        !String::from_utf8(bytes.clone())
            .unwrap()
            .contains("\"context\"")
    );
    let decoded: Signed<ManagedResultReadRequest> = serde_json::from_slice(&bytes).unwrap();
    decoded
        .verify("managed_result_read", &f.binding.control_public_key)
        .unwrap();
    assert_eq!(bytes, serde_json::to_vec(&decoded).unwrap());
    let context = observe(&f, &observation_request(&legacy.payload)).await;
    let valid = fresh_read(&f, &legacy.payload, context.clone());
    for case in 0..7 {
        let mut wrong = valid.payload.clone();
        match case {
            0 => wrong.format_version = 1,
            1 => wrong.context = None,
            2 => wrong.execution_receipt_sha256 = "0".repeat(64),
            3 => wrong.expires_at += 1,
            4 => {
                wrong.context = Some(
                    f.control
                        .sign("managed_result_context", context.payload.clone())
                        .unwrap(),
                )
            }
            5 => wrong.context = Some(f.runner.sign("receipt", context.payload.clone()).unwrap()),
            _ => wrong.issued_at = context.payload.observed_at - 1,
        }
        assert!(
            f.broker
                .authorize_managed_result_read(
                    f.control.sign("managed_result_read", wrong).unwrap(),
                    &target
                )
                .await
                .is_err(),
            "case {case}"
        );
    }
    let permit = f
        .broker
        .authorize_managed_result_read(valid, &target)
        .await
        .unwrap();
    sqlx::query("UPDATE runner_binding SET high_water=$1")
        .bind(legacy.payload.expires_at + 1)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(matches!(
        f.broker.consume_managed_result_read(permit, &target).await,
        Err(Error::ClockRollback)
    ));
}

#[tokio::test]
async fn suspended_context_cannot_be_reanchored_by_replay_or_a_new_read_nonce() {
    let f = fixture().await;
    let (_, legacy) = result_read_request(&f).await;
    let request = observation_request(&legacy.payload);
    let instant = Instant::now();
    let boot_start = 1_000_000_000;
    let (context, _) = f
        .broker
        .retain_managed_result_context(
            request.clone(),
            "a".repeat(64),
            request.issued_at,
            ResultReadClock {
                instant,
                boot_ns: Some(boot_start),
            },
            0,
            request.expires_at + MANAGED_RESULT_RETENTION_MILLIS,
        )
        .unwrap();
    let after_suspend = ResultReadClock {
        instant,
        boot_ns: Some(boot_start + 60_000_000_000),
    };
    // UTC and Instant both remain inside the signed 30s window, while BOOTTIME
    // advanced over 60s. Original output retention is a week, not this deadline.
    let wall = request.issued_at + 1000;
    context
        .payload
        .validate(&request, &f.binding, wall)
        .unwrap();
    assert!(matches!(
        f.broker
            .managed_result_context_deadline(&context, wall, after_suspend),
        Err(Error::Expired)
    ));
    assert!(matches!(
        f.broker.retain_managed_result_context(
            request.clone(),
            "a".repeat(64),
            wall,
            after_suspend,
            0,
            request.expires_at + MANAGED_RESULT_RETENTION_MILLIS,
        ),
        Err(Error::Expired)
    ));
    let read = fresh_read(&f, &legacy.payload, context.clone());
    let mut later_read = read.payload;
    later_read.request_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        canonical_hash(later_read.context.as_ref().unwrap()),
        canonical_hash(&context)
    );
    assert!(matches!(
        f.broker.managed_result_context_deadline(
            later_read.context.as_ref().unwrap(),
            wall,
            after_suspend,
        ),
        Err(Error::Expired)
    ));
}

#[tokio::test]
async fn short_read_has_its_own_original_boot_deadline_within_a_longer_context() {
    let f = fixture().await;
    let (_, legacy) = result_read_request(&f).await;
    let request = observation_request(&legacy.payload);
    let instant = Instant::now();
    let anchor = ResultReadClock {
        instant,
        boot_ns: Some(1_000_000_000),
    };
    let (context, _) = f
        .broker
        .retain_managed_result_context(
            request.clone(),
            "a".repeat(64),
            request.issued_at,
            anchor,
            0,
            request.expires_at + MANAGED_RESULT_RETENTION_MILLIS,
        )
        .unwrap();
    let mut read = fresh_read(&f, &legacy.payload, context.clone()).payload;
    read.issued_at = request.issued_at;
    read.expires_at = request.issued_at + 5000;
    f.broker
        .retain_result_read_epoch(&read, request.issued_at, anchor, 0)
        .unwrap();
    let after_suspend = ResultReadClock {
        instant,
        boot_ns: Some(11_000_000_000),
    };
    let wall = request.issued_at + 1000;
    read.validate(
        &f.binding,
        &request.target,
        &request.runtime_instance_sha256,
        wall,
    )
    .unwrap();
    assert!(
        f.broker
            .managed_result_context_deadline(&context, wall, after_suspend)
            .is_ok()
    );
    assert!(matches!(
        f.broker
            .managed_result_read_deadline(&read, wall, after_suspend),
        Err(Error::Expired)
    ));
    assert!(matches!(
        f.broker
            .retain_result_read_epoch(&read, wall, after_suspend, 0),
        Err(Error::Expired)
    ));
    // Both before-byte authorization and after-commit consumption consult this
    // same retained epoch, including a suspend inside the commit await.
    assert!(matches!(
        f.broker
            .managed_result_read_deadline(&read, wall, after_suspend),
        Err(Error::Expired)
    ));
}

#[tokio::test]
async fn signed_but_older_fence_cannot_legitimize_a_context_for_a_newer_original_attempt() {
    let f = fixture().await;
    let (target, legacy) = result_read_request(&f).await;
    let original = f
        .broker
        .receipt(&legacy.payload.process_attempt_id)
        .await
        .unwrap();
    let older = FenceRequest {
        runner_id: f.binding.runner_id.clone(),
        runner_generation: f.binding.generation,
        run_id: legacy.payload.run_id.clone(),
        lease_epoch: original.payload.request.lease_epoch - 1,
        revoked: false,
    };
    let encoded = serde_json::to_string(&f.runner.sign("fence", older.clone()).unwrap()).unwrap();
    sqlx::query("UPDATE runner_fences SET epoch=$1,payload=$2")
        .bind(older.lease_epoch as i64)
        .bind(&encoded)
        .execute(&f.pool)
        .await
        .unwrap();
    let request = observation_request(&legacy.payload);
    assert!(matches!(
        f.broker
            .observe_managed_result_context(
                f.control
                    .sign("managed_result_context_request", request.clone())
                    .unwrap(),
                &target,
                request.expires_at,
            )
            .await,
        Err(Error::Authority)
    ));
    // Exercise the separate read guard even against a shape-valid original
    // context, so issuance validation is not its sole dependency.
    let (context, _) = f
        .broker
        .retain_managed_result_context(
            request.clone(),
            canonical_hash(&older),
            request.issued_at,
            ResultReadClock::sample().unwrap(),
            0,
            request.expires_at,
        )
        .unwrap();
    assert!(matches!(
        f.broker
            .authorize_managed_result_read(fresh_read(&f, &legacy.payload, context), &target,)
            .await,
        Err(Error::Authority)
    ));
    assert_eq!(
        encoded,
        sqlx::query_scalar::<_, String>("SELECT payload FROM runner_fences")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    );
    assert_eq!(
        original,
        f.broker
            .receipt(&legacy.payload.process_attempt_id)
            .await
            .unwrap()
    );
}

/// Independent copy of the original V1 wire declaration, without any context
/// field or use of the V2 serializer. Both signatures use the same Ed25519 key
/// and the original purpose, so a changed field order/omitted field is detected.
#[derive(serde::Serialize)]
struct OriginalV1ReadWire {
    format_version: u32,
    runtime_instance_sha256: String,
    request_id: String,
    actor_id: String,
    authorization_sha256: String,
    execution_plan_id: String,
    execution_plan_sha256: String,
    process_attempt_id: String,
    run_id: String,
    launch_envelope_sha256: String,
    execution_receipt_sha256: String,
    runner: cowork_contracts::ExecutionRunnerIdentity,
    target: cowork_contracts::ExecutionTargetRef,
    operation: ManagedResultReadOperation,
    issued_at: i64,
    expires_at: i64,
}

#[tokio::test]
async fn independent_original_v1_manifest_and_chunk_vectors_preserve_exact_signature_bytes() {
    let f = fixture().await;
    let (_, original) = result_read_request(&f).await;
    for operation in [
        ManagedResultReadOperation::Manifest,
        ManagedResultReadOperation::Chunk {
            attestation_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
            chunk: cowork_contracts::ReadWorkspaceTransferChunk {
                path: "input-0001/result.txt".into(),
                offset: 0,
                expected_sha256: "c".repeat(64),
            },
        },
    ] {
        let mut current = original.payload.clone();
        current.operation = operation;
        let legacy = OriginalV1ReadWire {
            format_version: 1,
            runtime_instance_sha256: current.runtime_instance_sha256.clone(),
            request_id: current.request_id.clone(),
            actor_id: current.actor_id.clone(),
            authorization_sha256: current.authorization_sha256.clone(),
            execution_plan_id: current.execution_plan_id.clone(),
            execution_plan_sha256: current.execution_plan_sha256.clone(),
            process_attempt_id: current.process_attempt_id.clone(),
            run_id: current.run_id.clone(),
            launch_envelope_sha256: current.launch_envelope_sha256.clone(),
            execution_receipt_sha256: current.execution_receipt_sha256.clone(),
            runner: current.runner.clone(),
            target: current.target.clone(),
            operation: current.operation.clone(),
            issued_at: current.issued_at,
            expires_at: current.expires_at,
        };
        assert_eq!(
            serde_json::to_vec(&legacy).unwrap(),
            serde_json::to_vec(&current).unwrap()
        );
        let legacy_signed = f.control.sign("managed_result_read", legacy).unwrap();
        let current_signed = f.control.sign("managed_result_read", current).unwrap();
        assert_eq!(legacy_signed.signature, current_signed.signature);
        assert_eq!(
            serde_json::to_vec(&legacy_signed).unwrap(),
            serde_json::to_vec(&current_signed).unwrap()
        );
        legacy_signed
            .verify("managed_result_read", &f.binding.control_public_key)
            .unwrap();
        current_signed
            .verify("managed_result_read", &f.binding.control_public_key)
            .unwrap();
    }
}
