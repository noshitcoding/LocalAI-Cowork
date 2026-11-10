use super::*;
use sqlx::any::AnyPoolOptions;

#[path = "managed_result_delivery_tests.rs"]
mod managed_result_delivery_tests;
#[path = "result_context_tests.rs"]
mod result_context_tests;

async fn result_read_request(
    f: &Fixture,
) -> (
    cowork_contracts::ExecutionTargetRef,
    Signed<ManagedResultReadRequest>,
) {
    let runner = cowork_contracts::ExecutionRunnerIdentity {
        runner_id: f.binding.runner_id.clone(),
        generation: f.binding.generation,
        adapter_build_sha256: f.binding.adapter_build_sha256.clone(),
        control_public_key: f.binding.control_public_key.clone(),
        runner_public_key: f.binding.runner_public_key.clone(),
    };
    let target = cowork_contracts::ExecutionTargetRef {
        target_id: cowork_domain::execution_targets::execution_target_id(&runner).unwrap(),
        revision: 1,
        identity_sha256: cowork_domain::execution_targets::runner_identity_hash(&runner).unwrap(),
    };
    let original = request(f);
    let Admission::Launch(permit) = f.broker.admit(original.clone()).await.unwrap() else {
        panic!()
    };
    f.broker.dispatch_owned(permit).await.unwrap();
    let completed = f
        .broker
        .complete(
            &original.payload.process_attempt_id,
            &original.payload.request_sha256,
            result(),
        )
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let request = ManagedResultReadRequest {
        format_version: 1,
        runtime_instance_sha256: f.broker.instance_fingerprint(),
        request_id: uuid::Uuid::new_v4().to_string(),
        actor_id: "owner".into(),
        authorization_sha256: "a".repeat(64),
        execution_plan_id: "original-plan".into(),
        execution_plan_sha256: "b".repeat(64),
        process_attempt_id: "attempt-1".into(),
        run_id: "run-1".into(),
        launch_envelope_sha256: envelope_hash(&original),
        execution_receipt_sha256: canonical_hash(&completed),
        runner,
        target: target.clone(),
        operation: ManagedResultReadOperation::Manifest,
        issued_at: now,
        expires_at: now + 30000,
        context: None,
    };
    (
        target,
        f.control.sign("managed_result_read", request).unwrap(),
    )
}
#[tokio::test]
async fn result_read_permits_are_purpose_bound_instance_bound_and_never_execution_authority() {
    let f = fixture().await;
    let (target, request) = result_read_request(&f).await;
    let mut missing = request.payload.clone();
    missing.process_attempt_id = "missing-original".into();
    assert!(
        f.broker
            .authorize_managed_result_read(
                f.control.sign("managed_result_read", missing).unwrap(),
                &target
            )
            .await
            .is_err()
    );
    let mut wrong = request.payload.clone();
    wrong.execution_receipt_sha256 = "0".repeat(64);
    assert!(
        f.broker
            .authorize_managed_result_read(
                f.control.sign("managed_result_read", wrong).unwrap(),
                &target
            )
            .await
            .is_err()
    );
    assert!(
        f.broker
            .authorize_managed_result_read(
                f.runner
                    .sign("managed_result_read", request.payload.clone())
                    .unwrap(),
                &target
            )
            .await
            .is_err()
    );
    assert!(
        f.broker
            .authorize_managed_result_read(
                f.control.sign("launch", request.payload.clone()).unwrap(),
                &target
            )
            .await
            .is_err()
    );
    let permit = f
        .broker
        .authorize_managed_result_read(request.clone(), &target)
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(permit.request()),
        canonical_hash(&request.payload)
    );
    let reopened = Broker::open(pool(&f.url).await, f.binding.clone(), f.runner.clone())
        .await
        .unwrap();
    assert!(matches!(
        reopened.consume_managed_result_read(permit, &target).await,
        Err(Error::Authority)
    ));
    assert!(
        reopened
            .authorize_managed_result_read(request.clone(), &target)
            .await
            .is_err()
    );
    let permit = f
        .broker
        .authorize_managed_result_read(request.clone(), &target)
        .await
        .unwrap();
    f.broker
        .clone()
        .consume_managed_result_read(permit, &target)
        .await
        .unwrap();
    let prepared = f
        .broker
        .authorize_managed_result_read(request.clone(), &target)
        .await
        .unwrap();
    let original = f
        .broker
        .receipt(&request.payload.process_attempt_id)
        .await
        .unwrap();
    f.broker
        .fence(
            f.control
                .sign(
                    "fence",
                    FenceRequest {
                        runner_id: f.binding.runner_id.clone(),
                        runner_generation: f.binding.generation,
                        run_id: request.payload.run_id.clone(),
                        lease_epoch: original.payload.request.lease_epoch,
                        revoked: true,
                    },
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        f.broker
            .consume_managed_result_read(prepared, &target)
            .await,
        Err(Error::Authority)
    ));
    assert!(
        f.broker
            .authorize_managed_result_read(request.clone(), &target)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runner_executions")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runner_execution_lease_renewals")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0
    );
    f.broker
        .fence(
            f.control
                .sign(
                    "fence",
                    FenceRequest {
                        runner_id: f.binding.runner_id.clone(),
                        runner_generation: f.binding.generation,
                        run_id: request.payload.run_id.clone(),
                        lease_epoch: original.payload.request.lease_epoch + 1,
                        revoked: false,
                    },
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        f.broker
            .authorize_managed_result_read(request.clone(), &target)
            .await
            .is_err(),
        "A later positive execution epoch cannot revive an old content read grant"
    );
    sqlx::query("DELETE FROM runner_fences")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        f.broker
            .authorize_managed_result_read(request, &target)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runner_fences")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0,
        "A read must never create an absent positive fence"
    );
}
#[tokio::test]
async fn result_read_epoch_cannot_reset_or_prune_on_uncommitted_future_observation() {
    use std::time::{Duration, Instant};
    let clock = |instant| crate::broker::result_clock::ResultReadClock {
        instant,
        boot_ns: None,
    };
    let f = fixture().await;
    let (_, request) = result_read_request(&f).await;
    let anchor = Instant::now();
    let issued = request.payload.issued_at;
    let original = f
        .broker
        .retain_result_read_epoch(&request.payload, issued, clock(anchor), 0)
        .unwrap();
    assert_eq!(
        f.broker
            .retain_result_read_epoch(
                &request.payload,
                issued + 1,
                clock(anchor + Duration::from_secs(25)),
                0
            )
            .unwrap(),
        original
    );
    let mut later = request.payload.clone();
    later.request_id = uuid::Uuid::new_v4().to_string();
    later.issued_at = request.payload.expires_at + 1;
    later.expires_at = later.issued_at + 30000;
    f.broker
        .retain_result_read_epoch(
            &later,
            later.issued_at,
            clock(anchor + Duration::from_secs(31)),
            0,
        )
        .unwrap();
    // The later wall observation may still fail its SQL commit. Only the
    // already durable floor may prune the original nonce tombstone.
    assert!(matches!(
        f.broker.retain_result_read_epoch(
            &request.payload,
            issued + 2,
            clock(anchor + Duration::from_secs(31)),
            0
        ),
        Err(Error::Expired)
    ));
    let mut changed = request.payload;
    changed.actor_id = "different-owner".into();
    assert!(matches!(
        f.broker
            .retain_result_read_epoch(&changed, issued, clock(anchor), 0),
        Err(Error::Conflict)
    ));
}
#[tokio::test]
async fn queued_result_read_cannot_return_bytes_after_a_durable_clock_rollback() {
    let f = fixture().await;
    let (target, request) = result_read_request(&f).await;
    let permit = f
        .broker
        .authorize_managed_result_read(request.clone(), &target)
        .await
        .unwrap();
    sqlx::query("UPDATE runner_binding SET high_water=$1")
        .bind(request.payload.issued_at + 60000)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(matches!(
        f.broker.consume_managed_result_read(permit, &target).await,
        Err(Error::ClockRollback)
    ));
    assert!(matches!(
        f.broker
            .authorize_managed_result_read(request.clone(), &target)
            .await,
        Err(Error::ClockRollback)
    ));
}

#[tokio::test]
async fn input_transfer_requires_fresh_fence_and_does_not_admit_execution() {
    let f = fixture().await;
    let signed = request(&f);
    let wrong = f.control.sign("fence", signed.payload.clone()).unwrap();
    assert!(f.broker.authorize_input_transfer(&wrong).await.is_err());
    let permit = f.broker.authorize_input_transfer(&signed).await.unwrap();
    assert_eq!(
        f.broker
            .clone()
            .consume_input_transfer(permit)
            .await
            .unwrap(),
        signed.payload
    );
    assert!(matches!(
        f.broker.receipt(&signed.payload.process_attempt_id).await,
        Err(Error::NotFound)
    ));
    let prepared = f.broker.authorize_input_transfer(&signed).await.unwrap();
    f.broker
        .fence(
            f.control
                .sign(
                    "fence",
                    FenceRequest {
                        runner_id: f.binding.runner_id.clone(),
                        runner_generation: f.binding.generation,
                        run_id: signed.payload.run_id.clone(),
                        lease_epoch: signed.payload.lease_epoch,
                        revoked: true,
                    },
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        f.broker.consume_input_transfer(prepared).await,
        Err(Error::Authority)
    ));
    assert!(f.broker.authorize_input_transfer(&signed).await.is_err());
}

#[tokio::test]
async fn transfer_and_owned_dispatch_cannot_cross_admission_or_broker_instances() {
    let f = fixture().await;
    let signed = request(&f);
    let wrong = f.broker.authorize_input_transfer(&signed).await.unwrap();
    let reopened = Broker::open(f.pool.clone(), f.binding.clone(), f.runner.clone())
        .await
        .unwrap();
    assert!(matches!(
        reopened.consume_input_transfer(wrong).await,
        Err(Error::Authority)
    ));
    let prepared = f.broker.authorize_input_transfer(&signed).await.unwrap();
    let Admission::Launch(permit) = f.broker.admit(signed.clone()).await.unwrap() else {
        panic!()
    };
    assert!(matches!(
        f.broker.consume_input_transfer(prepared).await,
        Err(Error::Uncertain)
    ));
    assert!(matches!(
        f.broker.authorize_input_transfer(&signed).await,
        Err(Error::Uncertain)
    ));
    let owned = f.broker.dispatch_owned(permit).await.unwrap();
    f.broker.clone().validate_owned_dispatch(&owned).unwrap();
    assert!(reopened.validate_owned_dispatch(&owned).is_err());
    assert!(f.broker.is_bound_to(&f.binding));
    let mut changed = f.binding.clone();
    changed.runner_public_key[0] ^= 1;
    assert!(!f.broker.is_bound_to(&changed));
    assert!(matches!(
        f.broker.admit(signed).await.unwrap(),
        Admission::Existing(_)
    ));
}

#[tokio::test]
async fn reopened_broker_cannot_remint_a_seen_input_epoch_or_admit_it() {
    let f = fixture().await;
    let signed = request(&f);
    let permit = f.broker.authorize_input_transfer(&signed).await.unwrap();
    f.broker.consume_input_transfer(permit).await.unwrap();
    // A new instance shares the exact durable ledger, but cannot restore the
    // first Instant. Deny immediately, including while UTC is still fresh.
    let reopened = Broker::open(pool(&f.url).await, f.binding.clone(), f.runner.clone())
        .await
        .unwrap();
    assert!(matches!(
        reopened.authorize_input_transfer(&signed).await,
        Err(Error::Uncertain)
    ));
    assert!(matches!(
        reopened.admit(signed.clone()).await,
        Err(Error::Uncertain)
    ));
    let mut alias = signed.payload.clone();
    alias.process_attempt_id = "new-volume-same-effect".into();
    assert!(matches!(
        reopened
            .authorize_input_transfer(&f.control.sign("launch", alias).unwrap())
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        reopened.receipt(&signed.payload.process_attempt_id).await,
        Err(Error::NotFound)
    ));
    // The still-live original retains the only epoch and handoff.
    let permit = f.broker.authorize_input_transfer(&signed).await.unwrap();
    f.broker.consume_input_transfer(permit).await.unwrap();
    let Admission::Launch(permit) = f.broker.admit(signed.clone()).await.unwrap() else {
        panic!()
    };
    let dispatch = f.broker.dispatch_owned(permit).await.unwrap();
    f.broker
        .complete(
            &signed.payload.process_attempt_id,
            &signed.payload.request_sha256,
            result(),
        )
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runner_input_custody")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0
    );
    assert!(matches!(
        reopened.admit(signed).await.unwrap(),
        Admission::Existing(_)
    ));
    assert!(reopened.validate_owned_dispatch(&dispatch).is_err());
}

#[tokio::test]
async fn corrupt_input_custody_never_becomes_fresh_input_or_admission() {
    let f = fixture().await;
    let signed = request(&f);
    f.broker.authorize_input_transfer(&signed).await.unwrap();
    let payload: String = sqlx::query_scalar("SELECT payload FROM runner_input_custody")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let mut corrupt: serde_json::Value = serde_json::from_str(&payload).unwrap();
    corrupt["payload"]["broker_instance_sha256"] = "0".repeat(64).into();
    sqlx::query("UPDATE runner_input_custody SET payload=$1")
        .bind(serde_json::to_string(&corrupt).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f.broker.authorize_input_transfer(&signed).await.is_err());
    assert!(f.broker.admit(signed.clone()).await.is_err());
    sqlx::query("UPDATE runner_input_custody SET payload=$1,attempt_id='wrong-index'")
        .bind(payload)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f.broker.authorize_input_transfer(&signed).await.is_err());
    assert!(f.broker.admit(signed).await.is_err());
}

#[tokio::test]
async fn queued_input_permit_cannot_outlive_its_initial_monotonic_deadline() {
    let f = fixture().await;
    let mut raw = request(&f).payload;
    raw.expires_at = raw.issued_at + 2000;
    let signed = f.control.sign("launch", raw).unwrap();
    let permit = f.broker.authorize_input_transfer(&signed).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    assert!(f.broker.consume_input_transfer(permit).await.is_err());
    assert!(matches!(
        f.broker.receipt(&signed.payload.process_attempt_id).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn repeated_input_operations_keep_the_first_monotonic_epoch_and_intent() {
    use std::time::{Duration, Instant};
    let f = fixture().await;
    let mut request = timed_request(&f, 1_700_000_000_000).payload;
    request.expires_at = request.issued_at + 30_000;
    let anchor = Instant::now();
    let first = f
        .broker
        .retain_input_epoch(&request, request.issued_at, anchor)
        .unwrap();
    assert_eq!(first, anchor + Duration::from_secs(30));
    // After 25 real seconds, a wall value only 1ms newer must not mint
    // another thirty seconds. No operating-system clock is changed here.
    assert_eq!(
        f.broker
            .retain_input_epoch(
                &request,
                request.issued_at + 1,
                anchor + Duration::from_secs(25)
            )
            .unwrap(),
        first
    );
    assert_eq!(
        f.broker
            .initial_input_deadline(&request, request.issued_at + 10_000)
            .unwrap(),
        anchor + Duration::from_secs(10)
    );
    assert!(matches!(
        f.broker.retain_input_epoch(
            &request,
            request.issued_at + 2,
            anchor + Duration::from_secs(31)
        ),
        Err(Error::Expired)
    ));
    assert!(matches!(
        f.broker
            .retain_input_epoch(&request, request.issued_at - 1, anchor),
        Err(Error::ClockRollback)
    ));
    let mut alias = request.clone();
    alias.process_attempt_id = "another-attempt-same-effect".into();
    assert!(matches!(
        f.broker
            .retain_input_epoch(&alias, request.issued_at, anchor),
        Err(Error::Conflict)
    ));
    alias = request.clone();
    alias.invocation.arguments.push("changed".into());
    assert!(matches!(
        f.broker.initial_input_deadline(&alias, request.expires_at),
        Err(Error::Conflict)
    ));
}

#[tokio::test]
async fn queued_authority_observation_samples_clock_after_its_database_turn() {
    use std::{future::Future, task::Poll, time::Duration};
    let f = fixture().await;
    let signed = request(&f);
    let Admission::Launch(permit) = f.broker.admit(signed).await.unwrap() else {
        panic!()
    };
    let dispatched = f.broker.dispatch(permit).await.unwrap();
    // Occupy the single physical connection. Polling the real public observer
    // below must wait; a previous caller can then commit a newer true time.
    let mut earlier = f.pool.begin().await.unwrap();
    let pending = f.broker.execution_allowed(&dispatched);
    tokio::pin!(pending);
    std::future::poll_fn(|cx| {
        assert!(matches!(pending.as_mut().poll(cx), Poll::Pending));
        Poll::Ready(())
    })
    .await;
    let queued_at = chrono::Utc::now().timestamp_millis();
    let until = tokio::time::Instant::now() + Duration::from_secs(2);
    while chrono::Utc::now().timestamp_millis() <= queued_at {
        assert!(tokio::time::Instant::now() < until);
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let later = chrono::Utc::now().timestamp_millis();
    sqlx::query("UPDATE runner_binding SET high_water=$1 WHERE id=1")
        .bind(later)
        .execute(&mut *earlier)
        .await
        .unwrap();
    earlier.commit().await.unwrap();
    assert!(
        chrono::Utc::now().timestamp_millis() >= later,
        "This regression uses a forward-moving real clock"
    );
    assert!(
        pending.await.unwrap(),
        "Waiting behind a newer real observation is not a clock rollback"
    );
}

struct Fixture {
    _dir: tempfile::TempDir,
    pool: sqlx::AnyPool,
    url: String,
    control: SigningKey,
    runner: SigningKey,
    binding: Binding,
    broker: Broker,
}
async fn pool(url: &str) -> sqlx::AnyPool {
    sqlx::any::install_default_drivers();
    AnyPoolOptions::new()
        .max_connections(1)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("PRAGMA busy_timeout=10000")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("PRAGMA synchronous=FULL")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .unwrap()
}
async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runner.sqlite3");
    let encoded = percent_encoding::utf8_percent_encode(
        path.to_str().unwrap(),
        percent_encoding::NON_ALPHANUMERIC,
    );
    let url = format!("sqlite:{encoded}?mode=rwc");
    let pool = pool(&url).await;
    let control = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let runner = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let binding = Binding {
        runner_id: "owned-runner".into(),
        generation: 7,
        adapter_build_sha256: "a".repeat(64),
        control_public_key: control.public_key(),
        runner_public_key: runner.public_key(),
    };
    let broker = Broker::open(pool.clone(), binding.clone(), runner.clone())
        .await
        .unwrap();
    Fixture {
        _dir: dir,
        pool,
        url,
        control,
        runner,
        binding,
        broker,
    }
}
fn request(f: &Fixture) -> Signed<LaunchRequest> {
    let now = chrono::Utc::now().timestamp_millis();
    f.control
        .sign(
            "launch",
            LaunchRequest {
                format_version: 1,
                runner_id: f.binding.runner_id.clone(),
                runner_generation: f.binding.generation,
                adapter_build_sha256: f.binding.adapter_build_sha256.clone(),
                process_attempt_id: "attempt-1".into(),
                run_id: "run-1".into(),
                tool_call_id: "call-1".into(),
                request_sha256: "b".repeat(64),
                worker_fence_sha256: "c".repeat(64),
                lease_epoch: 11,
                environment: ExecutionEnvironmentRef {
                    environment_id: "env-1".into(),
                    version_id: "version-1".into(),
                    definition_sha256: "d".repeat(64),
                    qualification_sha256: "e".repeat(64),
                },
                workspace_resource_id: "workspace-1".into(),
                invocation: ProcessInvocation {
                    program: "qualified-fixture".into(),
                    arguments: vec!["literal".into()],
                    timeout_seconds: 2,
                },
                issued_at: now,
                expires_at: now + 20_000,
                execution_deadline: now + 25_000,
            },
        )
        .unwrap()
}
fn result() -> ProcessResult {
    ProcessResult {
        termination: cowork_contracts::ProcessTermination::Exited,
        process_id: Some(1),
        exit_code: Some(0),
        signal: None,
        stdout: cowork_contracts::ProcessCapture {
            bytes: b"once".to_vec(),
            sha256: cowork_domain::hash(b"once"),
            total_bytes: 4,
            truncated: false,
        },
        stderr: cowork_contracts::ProcessCapture {
            bytes: vec![],
            sha256: cowork_domain::hash(b""),
            total_bytes: 0,
            truncated: false,
        },
        duration_ms: 1,
        output_complete: true,
    }
}
fn permit(admission: Admission) -> LaunchPermit {
    match admission {
        Admission::Launch(p) => p,
        _ => panic!("first admission must grant one permit"),
    }
}

#[tokio::test]
async fn authenticated_signature_purpose_and_request_tampering() {
    let f = fixture().await;
    let signed = request(&f);
    signed
        .verify("launch", &f.binding.control_public_key)
        .unwrap();
    assert!(
        signed
            .verify("fence", &f.binding.control_public_key)
            .is_err()
    );
    let forged = f.runner.sign("launch", signed.payload.clone()).unwrap();
    assert!(matches!(
        f.broker.admit(forged).await,
        Err(Error::Authority)
    ));
    let mut mutated = signed;
    mutated.payload.invocation.arguments.push("other".into());
    assert!(matches!(
        f.broker.admit(mutated).await,
        Err(Error::Authority)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runner_executions")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn dropped_owned_dispatch_is_retained_without_new_handoff_after_reopen() {
    let f = fixture().await;
    let signed = request(&f);
    let p = permit(f.broker.admit(signed.clone()).await.unwrap());
    let owned = f.broker.dispatch_owned(p).await.unwrap();
    assert_eq!(owned.request(), &signed.payload);
    drop(owned); // Crash/lost worker before any observed physical effect.
    let reopened = Broker::open(pool(&f.url).await, f.binding.clone(), f.runner.clone())
        .await
        .unwrap();
    let Admission::Existing(receipt) = reopened.admit(signed).await.unwrap() else {
        panic!("A retained dispatch must never mint another in-memory handoff");
    };
    assert_eq!(receipt.payload.state, ExecutionState::DispatchCommitted);
    assert_eq!(receipt.payload.revision, 2);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runner_executions")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn repeated_admission_restart_and_completion_are_immutable() {
    let f = fixture().await;
    let signed = request(&f);
    let p = permit(f.broker.admit(signed.clone()).await.unwrap());
    let before = f.broker.receipt("attempt-1").await.unwrap();
    before
        .verify("receipt", &f.binding.runner_public_key)
        .unwrap();
    assert!(matches!(
        f.broker.admit(signed.clone()).await.unwrap(),
        Admission::Existing(_)
    ));
    f.broker.dispatch(p).await.unwrap();
    let reopened = Broker::open(pool(&f.url).await, f.binding.clone(), f.runner.clone())
        .await
        .unwrap();
    assert!(matches!(
        reopened.admit(signed.clone()).await.unwrap(),
        Admission::Existing(_)
    ));
    let complete = reopened
        .complete("attempt-1", &signed.payload.request_sha256, result())
        .await
        .unwrap();
    let replay = reopened
        .complete("attempt-1", &signed.payload.request_sha256, result())
        .await
        .unwrap();
    assert_eq!(complete, replay);
    assert!(
        matches!(reopened.admit(signed.clone()).await.unwrap(),Admission::Existing(r) if r == complete)
    );
    let mut changed = result();
    changed.exit_code = Some(1);
    assert!(matches!(
        reopened
            .complete("attempt-1", &signed.payload.request_sha256, changed)
            .await,
        Err(Error::Conflict)
    ));
}

#[tokio::test]
async fn concurrent_independent_pools_yield_exactly_one_permit() {
    let f = fixture().await;
    let other = Broker::open(pool(&f.url).await, f.binding.clone(), f.runner.clone())
        .await
        .unwrap();
    let r = request(&f);
    let (a, b) = tokio::join!(f.broker.admit(r.clone()), other.admit(r));
    let count = [a.unwrap(), b.unwrap()]
        .into_iter()
        .filter(|a| matches!(a, Admission::Launch(_)))
        .count();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn revocation_blocks_dispatch_and_cannot_reopen_same_epoch() {
    let f = fixture().await;
    let signed = request(&f);
    let p = permit(f.broker.admit(signed.clone()).await.unwrap());
    let fence = FenceRequest {
        runner_id: f.binding.runner_id.clone(),
        runner_generation: f.binding.generation,
        run_id: signed.payload.run_id.clone(),
        lease_epoch: signed.payload.lease_epoch,
        revoked: true,
    };
    f.broker
        .fence(f.control.sign("fence", fence.clone()).unwrap())
        .await
        .unwrap();
    assert!(matches!(f.broker.dispatch(p).await, Err(Error::Authority)));
    let mut reopen = fence;
    reopen.revoked = false;
    assert!(matches!(
        f.broker
            .fence(f.control.sign("fence", reopen.clone()).unwrap())
            .await,
        Err(Error::Authority)
    ));
    reopen.lease_epoch += 1;
    assert!(matches!(
        f.broker
            .fence(f.control.sign("fence", reopen).unwrap())
            .await,
        Err(Error::Uncertain)
    ));
    assert!(matches!(
        f.broker.admit(signed).await.unwrap(),
        Admission::Existing(_)
    ));
}

#[tokio::test]
async fn durable_clock_rollback_blocks_new_work_but_not_revocation() {
    let f = fixture().await;
    let signed = request(&f);
    let now = signed.payload.issued_at;
    let p = permit(f.broker.admit_at(signed.clone(), now + 1).await.unwrap());
    assert!(matches!(
        f.broker.dispatch_at(p, now).await,
        Err(Error::ClockRollback)
    ));
    f.broker
        .fence(
            f.control
                .sign(
                    "fence",
                    FenceRequest {
                        runner_id: f.binding.runner_id.clone(),
                        runner_generation: 7,
                        run_id: "run-1".into(),
                        lease_epoch: 11,
                        revoked: true,
                    },
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let before = f.broker.receipt("attempt-1").await.unwrap();
    assert!(
        matches!(f.broker.admit_at(signed,now-1).await.unwrap(),Admission::Existing(r) if r == before)
    );
}

#[tokio::test]
async fn expiry_conflicting_call_and_unsafe_completion_fail_closed() {
    let f = fixture().await;
    let signed = request(&f);
    assert!(matches!(
        f.broker
            .admit_at(signed.clone(), signed.payload.expires_at)
            .await,
        Err(Error::Expired)
    ));
    let _p = permit(f.broker.admit(signed.clone()).await.unwrap());
    assert!(matches!(
        f.broker
            .complete("attempt-1", &signed.payload.request_sha256, result())
            .await,
        Err(Error::Uncertain)
    ));
    let mut changed = signed.payload.clone();
    changed.process_attempt_id = "new-attempt".into();
    let conflict = f.control.sign("launch", changed).unwrap();
    assert!(matches!(
        f.broker.admit(conflict).await,
        Err(Error::Conflict)
    ));
}

#[tokio::test]
async fn changed_bootstrap_and_corrupt_index_or_signature_are_denied() {
    let f = fixture().await;
    let _p = permit(f.broker.admit(request(&f)).await.unwrap());
    let mut binding = f.binding.clone();
    binding.generation += 1;
    assert!(matches!(
        Broker::open(f.pool.clone(), binding, f.runner.clone()).await,
        Err(Error::Authority)
    ));
    sqlx::query("UPDATE runner_executions SET run_id='other-run' WHERE attempt_id='attempt-1'")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(matches!(
        f.broker.receipt("attempt-1").await,
        Err(Error::Authority)
    ));
    sqlx::query("UPDATE runner_executions SET run_id='run-1' WHERE attempt_id='attempt-1'")
        .execute(&f.pool)
        .await
        .unwrap();
    let mut forged = f.broker.receipt("attempt-1").await.unwrap();
    forged.signature[0] ^= 1;
    sqlx::query("UPDATE runner_executions SET payload=$1 WHERE attempt_id='attempt-1'")
        .bind(serde_json::to_string(&forged).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(matches!(
        f.broker.receipt("attempt-1").await,
        Err(Error::Authority)
    ));
}

#[tokio::test]
async fn corrupt_fence_does_not_grant_launch() {
    let f = fixture().await;
    let signed = request(&f);
    let p = permit(f.broker.admit(signed).await.unwrap());
    sqlx::query("UPDATE runner_fences SET epoch=12 WHERE run_id='run-1'")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(matches!(f.broker.dispatch(p).await, Err(Error::Authority)));
}

#[tokio::test]
async fn supervisor_requires_exact_authenticated_dispatched_attempt() {
    let f = fixture().await;
    let signed = request(&f);
    assert!(!f.broker.execution_allowed(&signed.payload).await.unwrap());
    let p = permit(f.broker.admit(signed.clone()).await.unwrap());
    assert!(!f.broker.execution_allowed(&signed.payload).await.unwrap());
    let dispatched = f.broker.dispatch(p).await.unwrap();
    assert!(f.broker.execution_allowed(&dispatched).await.unwrap());
    let mut forged = dispatched.clone();
    forged.execution_deadline += 1_000;
    assert!(!f.broker.execution_allowed(&forged).await.unwrap());
    forged = dispatched.clone();
    forged.process_attempt_id = "unadmitted-attempt".into();
    assert!(!f.broker.execution_allowed(&forged).await.unwrap());
    f.broker
        .complete("attempt-1", &dispatched.request_sha256, result())
        .await
        .unwrap();
    assert!(!f.broker.execution_allowed(&dispatched).await.unwrap());
}

fn timed_request(f: &Fixture, now: i64) -> Signed<LaunchRequest> {
    let mut payload = request(f).payload;
    payload.issued_at = now;
    payload.expires_at = now + 10_000;
    payload.execution_deadline = now + 60_000;
    f.control.sign("launch", payload).unwrap()
}

fn renewal(
    f: &Fixture,
    launch: &Signed<LaunchRequest>,
    revision: u64,
    now: i64,
    expires_at: i64,
) -> Signed<ExecutionLeaseRenewal> {
    f.control
        .sign(
            "execution_lease",
            ExecutionLeaseRenewal {
                runner_id: launch.payload.runner_id.clone(),
                runner_generation: launch.payload.runner_generation,
                adapter_build_sha256: launch.payload.adapter_build_sha256.clone(),
                process_attempt_id: launch.payload.process_attempt_id.clone(),
                run_id: launch.payload.run_id.clone(),
                request_envelope_sha256: envelope_hash(launch),
                worker_fence_sha256: launch.payload.worker_fence_sha256.clone(),
                lease_epoch: launch.payload.lease_epoch,
                revision,
                issued_at: now,
                expires_at,
            },
        )
        .unwrap()
}

async fn dispatch_at(f: &Fixture, launch: &Signed<LaunchRequest>, now: i64) -> LaunchRequest {
    let p = permit(f.broker.admit_at(launch.clone(), now).await.unwrap());
    f.broker.dispatch_at(p, now).await.unwrap()
}

#[tokio::test]
async fn controller_lease_expiry_stops_execution_before_original_run_deadline() {
    let f = fixture().await;
    let now = 1_700_000_000_000;
    let signed = timed_request(&f, now);
    let dispatched = dispatch_at(&f, &signed, now).await;
    assert!(
        f.broker
            .execution_allowed_at(&dispatched, now + 9_999)
            .await
            .unwrap()
    );
    assert!(
        !f.broker
            .execution_allowed_at(&dispatched, now + 10_000)
            .await
            .unwrap()
    );
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(
                renewal(&f, &signed, 12, now + 10_000, now + 20_000),
                now + 10_000
            )
            .await,
        Err(Error::Expired)
    ));
    assert!(matches!(
        f.broker
            .execution_allowed_at(&dispatched, now + 9_999)
            .await,
        Err(Error::ClockRollback)
    ));
}

#[tokio::test]
async fn execution_lease_history_survives_restart_and_replay_never_extends_it() {
    let f = fixture().await;
    let now = 1_700_000_000_000;
    let launch = timed_request(&f, now);
    let dispatched = dispatch_at(&f, &launch, now).await;
    let first = renewal(&f, &launch, 12, now + 3_000, now + 13_000);
    f.broker
        .renew_execution_lease_at(first.clone(), now + 3_000)
        .await
        .unwrap();
    let reopened = Broker::open(pool(&f.url).await, f.binding.clone(), f.runner.clone())
        .await
        .unwrap();
    assert!(
        reopened
            .execution_allowed_at(&dispatched, now + 10_001)
            .await
            .unwrap()
    );
    reopened
        .renew_execution_lease_at(first.clone(), now + 12_000)
        .await
        .unwrap();
    let second = renewal(&f, &launch, 14, now + 12_000, now + 22_000);
    reopened
        .renew_execution_lease_at(second.clone(), now + 12_000)
        .await
        .unwrap();
    // Out-of-order delivery of an accepted old revision reads history only.
    reopened
        .renew_execution_lease_at(first.clone(), now + 20_000)
        .await
        .unwrap();
    assert!(
        reopened
            .execution_allowed_at(&dispatched, now + 21_999)
            .await
            .unwrap()
    );
    assert!(
        !reopened
            .execution_allowed_at(&dispatched, now + 22_000)
            .await
            .unwrap()
    );
    reopened
        .renew_execution_lease_at(second, now + 23_000)
        .await
        .unwrap();
    assert!(
        !reopened
            .execution_allowed_at(&dispatched, now + 23_000)
            .await
            .unwrap()
    );
    assert!(matches!(
        reopened
            .renew_execution_lease_at(
                renewal(&f, &launch, 15, now + 23_000, now + 33_000),
                now + 23_000
            )
            .await,
        Err(Error::Expired)
    ));
    let saved: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT revision,expires_at FROM runner_execution_lease_renewals ORDER BY revision",
    )
    .fetch_all(&f.pool)
    .await
    .unwrap();
    assert_eq!(saved, vec![(12, now + 13_000), (14, now + 22_000)]);
    let mut conflict = first.payload;
    conflict.expires_at += 1_000;
    assert!(matches!(
        reopened
            .renew_execution_lease_at(
                f.control.sign("execution_lease", conflict).unwrap(),
                now + 23_000
            )
            .await,
        Err(Error::Conflict)
    ));
}

#[tokio::test]
async fn execution_lease_authentication_and_exact_launch_binding_are_required() {
    let f = fixture().await;
    let now = 1_700_000_000_000;
    let launch = timed_request(&f, now);
    dispatch_at(&f, &launch, now).await;
    let original = renewal(&f, &launch, 12, now + 3_000, now + 13_000);
    let mut tampered = original.clone();
    tampered.payload.expires_at += 1;
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(tampered, now + 3_000)
            .await,
        Err(Error::Authority)
    ));
    for signed in [
        f.runner
            .sign("execution_lease", original.payload.clone())
            .unwrap(),
        f.control.sign("fence", original.payload.clone()).unwrap(),
    ] {
        assert!(matches!(
            f.broker.renew_execution_lease_at(signed, now + 3_000).await,
            Err(Error::Authority)
        ));
    }
    let changes: [fn(&mut ExecutionLeaseRenewal); 9] = [
        |r| r.runner_id = "another-runner".into(),
        |r| r.runner_generation += 1,
        |r| r.adapter_build_sha256 = "f".repeat(64),
        |r| r.run_id = "another-run".into(),
        |r| r.request_envelope_sha256 = "f".repeat(64),
        |r| r.worker_fence_sha256 = "f".repeat(64),
        |r| r.lease_epoch += 1,
        |r| r.revision = 11,
        |r| r.revision = u64::MAX,
    ];
    for change in changes {
        let mut changed = original.payload.clone();
        change(&mut changed);
        assert!(matches!(
            f.broker
                .renew_execution_lease_at(
                    f.control.sign("execution_lease", changed).unwrap(),
                    now + 3_000
                )
                .await,
            Err(Error::Authority)
        ));
    }
    let mut changed = original.payload;
    changed.process_attempt_id = "different-attempt".into();
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(
                f.control.sign("execution_lease", changed).unwrap(),
                now + 3_000
            )
            .await,
        Err(Error::NotFound)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runner_execution_lease_renewals")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn execution_lease_deadlines_future_issuance_and_stale_revisions_are_denied() {
    let f = fixture().await;
    let now = 1_700_000_000_000;
    let launch = timed_request(&f, now);
    dispatch_at(&f, &launch, now).await;
    let original = renewal(&f, &launch, 12, now + 3_000, now + 13_000);
    let changes: [fn(&mut ExecutionLeaseRenewal); 4] = [
        |r| r.expires_at = r.issued_at + 30_001,
        |r| r.expires_at = r.issued_at,
        |r| r.expires_at += 60_000,
        |r| r.issued_at -= 4_000,
    ];
    for change in changes {
        let mut changed = original.payload.clone();
        change(&mut changed);
        assert!(matches!(
            f.broker
                .renew_execution_lease_at(
                    f.control.sign("execution_lease", changed).unwrap(),
                    now + 3_000
                )
                .await,
            Err(Error::Authority)
        ));
    }
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(original.clone(), now + 2_999)
            .await,
        Err(Error::Expired)
    ));
    let too_short = renewal(&f, &launch, 12, now + 3_000, now + 10_000);
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(too_short, now + 3_000)
            .await,
        Err(Error::Conflict)
    ));
    let newest = renewal(&f, &launch, 14, now + 3_000, now + 13_000);
    f.broker
        .renew_execution_lease_at(newest, now + 3_000)
        .await
        .unwrap();
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(original, now + 3_001)
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(
                renewal(&f, &launch, 15, now + 2_999, now + 14_000),
                now + 2_999
            )
            .await,
        Err(Error::ClockRollback)
    ));
}

#[tokio::test]
async fn expired_renewal_observation_is_durable_and_revocation_remains_allowed() {
    let f = fixture().await;
    let now = 1_700_000_000_000;
    let launch = timed_request(&f, now);
    let dispatched = dispatch_at(&f, &launch, now).await;
    let r = renewal(&f, &launch, 12, now + 10_000, now + 20_000);
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(r.clone(), now + 10_000)
            .await,
        Err(Error::Expired)
    ));
    assert!(matches!(
        f.broker.renew_execution_lease_at(r, now + 9_999).await,
        Err(Error::ClockRollback)
    ));
    f.broker
        .fence(
            f.control
                .sign(
                    "fence",
                    FenceRequest {
                        runner_id: launch.payload.runner_id,
                        runner_generation: launch.payload.runner_generation,
                        run_id: launch.payload.run_id,
                        lease_epoch: launch.payload.lease_epoch,
                        revoked: true,
                    },
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        !f.broker
            .execution_allowed_at(&dispatched, now + 10_001)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn execution_lease_does_not_grant_admission_or_revoked_or_completed_work() {
    let f = fixture().await;
    let now = 1_700_000_000_000;
    let launch = timed_request(&f, now);
    let p = permit(f.broker.admit_at(launch.clone(), now).await.unwrap());
    let r = renewal(&f, &launch, 12, now + 3_000, now + 13_000);
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(r.clone(), now + 3_000)
            .await,
        Err(Error::Uncertain)
    ));
    let dispatched = f.broker.dispatch_at(p, now).await.unwrap();
    f.broker
        .renew_execution_lease_at(r.clone(), now + 3_000)
        .await
        .unwrap();
    f.broker
        .fence(
            f.control
                .sign(
                    "fence",
                    FenceRequest {
                        runner_id: launch.payload.runner_id.clone(),
                        runner_generation: launch.payload.runner_generation,
                        run_id: launch.payload.run_id.clone(),
                        lease_epoch: launch.payload.lease_epoch,
                        revoked: true,
                    },
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(
                renewal(&f, &launch, 13, now + 4_000, now + 14_000),
                now + 4_000
            )
            .await,
        Err(Error::Authority)
    ));
    // Historical exact replay cannot reopen the durable fence.
    f.broker
        .renew_execution_lease_at(r, now + 4_000)
        .await
        .unwrap();
    assert!(
        !f.broker
            .execution_allowed_at(&dispatched, now + 4_000)
            .await
            .unwrap()
    );
    f.broker
        .complete("attempt-1", &launch.payload.request_sha256, result())
        .await
        .unwrap();
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(
                renewal(&f, &launch, 13, now + 4_000, now + 14_000),
                now + 4_000
            )
            .await,
        Err(Error::Uncertain)
    ));
}

#[tokio::test]
async fn corrupt_execution_lease_payload_or_index_cannot_extend_authority() {
    let f = fixture().await;
    let now = 1_700_000_000_000;
    let launch = timed_request(&f, now);
    let dispatched = dispatch_at(&f, &launch, now).await;
    let r = renewal(&f, &launch, 12, now + 3_000, now + 13_000);
    f.broker
        .renew_execution_lease_at(r.clone(), now + 3_000)
        .await
        .unwrap();
    for query in [
        "UPDATE runner_execution_lease_renewals SET expires_at=expires_at+1",
        "UPDATE runner_execution_lease_renewals SET revision=revision+1",
    ] {
        sqlx::query(query).execute(&f.pool).await.unwrap();
        assert!(matches!(
            f.broker
                .execution_allowed_at(&dispatched, now + 4_000)
                .await,
            Err(Error::Authority)
        ));
        sqlx::query("UPDATE runner_execution_lease_renewals SET revision=12,expires_at=$1")
            .bind(now + 13_000)
            .execute(&f.pool)
            .await
            .unwrap();
    }
    let stored: String = sqlx::query_scalar("SELECT payload FROM runner_execution_lease_renewals")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let mut forged: serde_json::Value = serde_json::from_str(&stored).unwrap();
    let signature_byte = forged["signature"][0].as_u64().unwrap();
    forged["signature"][0] = (signature_byte ^ 1).into();
    sqlx::query("UPDATE runner_execution_lease_renewals SET payload=$1")
        .bind(serde_json::to_string(&forged).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(matches!(
        f.broker
            .execution_allowed_at(&dispatched, now + 4_000)
            .await,
        Err(Error::Authority)
    ));
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(
                renewal(&f, &launch, 13, now + 4_000, now + 14_000),
                now + 4_000
            )
            .await,
        Err(Error::Authority)
    ));
    // A genuine controller signature proves the incoming message, not that the
    // runner accepted it before the previous lease expired. Injecting that raw
    // message cannot replace the separately runner-signed durable acceptance.
    let never_accepted = renewal(&f, &launch, 12, now + 15_000, now + 25_000);
    never_accepted
        .verify("execution_lease", &f.binding.control_public_key)
        .unwrap();
    sqlx::query("UPDATE runner_execution_lease_renewals SET expires_at=$1,payload=$2")
        .bind(now + 25_000)
        .bind(serde_json::to_string(&never_accepted).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        f.broker
            .execution_allowed_at(&dispatched, now + 16_000)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn concurrent_execution_lease_replays_preserve_one_immutable_revision() {
    let f = fixture().await;
    let now = 1_700_000_000_000;
    let launch = timed_request(&f, now);
    dispatch_at(&f, &launch, now).await;
    let other = Broker::open(pool(&f.url).await, f.binding.clone(), f.runner.clone())
        .await
        .unwrap();
    let r = renewal(&f, &launch, 12, now + 3_000, now + 13_000);
    let (a, b) = tokio::join!(
        f.broker.renew_execution_lease_at(r.clone(), now + 3_000),
        other.renew_execution_lease_at(r, now + 3_000)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runner_execution_lease_renewals")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn execution_lease_can_reach_but_never_extend_original_deadline() {
    let f = fixture().await;
    let now = 1_700_000_000_000;
    let launch = timed_request(&f, now);
    let dispatched = dispatch_at(&f, &launch, now).await;
    f.broker
        .renew_execution_lease_at(
            renewal(&f, &launch, 12, now + 3_000, now + 33_000),
            now + 3_000,
        )
        .await
        .unwrap();
    let last = renewal(
        &f,
        &launch,
        13,
        now + 32_000,
        launch.payload.execution_deadline,
    );
    f.broker
        .renew_execution_lease_at(last.clone(), now + 32_000)
        .await
        .unwrap();
    assert!(
        f.broker
            .execution_allowed_at(&dispatched, now + 59_999)
            .await
            .unwrap()
    );
    assert!(matches!(
        f.broker
            .renew_execution_lease_at(
                renewal(&f, &launch, 14, now + 59_999, now + 60_001),
                now + 59_999,
            )
            .await,
        Err(Error::Authority)
    ));
    f.broker
        .renew_execution_lease_at(last, now + 60_000)
        .await
        .unwrap();
    assert!(
        !f.broker
            .execution_allowed_at(&dispatched, now + 60_000)
            .await
            .unwrap()
    );
}
