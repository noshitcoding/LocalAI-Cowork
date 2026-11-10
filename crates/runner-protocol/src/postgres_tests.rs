//! Explicitly isolated PostgreSQL contract. No physical process or sandbox claim.
use super::*;
use sqlx::{AnyPool, any::AnyPoolOptions};

async fn pool(url: &str) -> AnyPool {
    sqlx::any::install_default_drivers();
    AnyPoolOptions::new()
        .max_connections(4)
        .connect(url)
        .await
        .unwrap()
}

fn launch(binding: &Binding, control: &SigningKey, case: &str, now: i64) -> Signed<LaunchRequest> {
    control
        .sign(
            "launch",
            LaunchRequest {
                format_version: 1,
                runner_id: binding.runner_id.clone(),
                runner_generation: binding.generation,
                adapter_build_sha256: binding.adapter_build_sha256.clone(),
                process_attempt_id: format!("attempt-{case}"),
                run_id: format!("run-{case}"),
                tool_call_id: format!("tool-{case}"),
                request_sha256: "b".repeat(64),
                worker_fence_sha256: "c".repeat(64),
                lease_epoch: 11,
                environment: ExecutionEnvironmentRef {
                    environment_id: "fixture-env".into(),
                    version_id: "fixture-version".into(),
                    definition_sha256: "d".repeat(64),
                    qualification_sha256: "e".repeat(64),
                },
                workspace_resource_id: "fixture-workspace".into(),
                invocation: ProcessInvocation {
                    program: "synthetic-core-fixture".into(),
                    arguments: vec![],
                    timeout_seconds: 45,
                },
                issued_at: now,
                expires_at: now + 10_000,
                execution_deadline: now + 60_000,
            },
        )
        .unwrap()
}

fn renewal(
    control: &SigningKey,
    launch: &Signed<LaunchRequest>,
    revision: u64,
    issued_at: i64,
    expires_at: i64,
) -> Signed<ExecutionLeaseRenewal> {
    control
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
                issued_at,
                expires_at,
            },
        )
        .unwrap()
}

fn permit(admission: Admission) -> LaunchPermit {
    match admission {
        Admission::Launch(p) => p,
        Admission::Existing(_) => panic!("only first admission returns dispatch authority"),
    }
}

async fn dispatch(broker: &Broker, launch: &Signed<LaunchRequest>, now: i64) -> LaunchRequest {
    let p = permit(broker.admit_at(launch.clone(), now).await.unwrap());
    broker.dispatch_at(p, now).await.unwrap()
}

#[derive(Serialize, Deserialize)]
struct AcceptedReceipt {
    renewal: Signed<ExecutionLeaseRenewal>,
    previous_revision: u64,
    previous_expires_at: i64,
    accepted_at: i64,
}

fn physical_result_fixture() -> ProcessResult {
    let empty = cowork_contracts::ProcessCapture {
        bytes: vec![],
        sha256: cowork_domain::hash(b""),
        total_bytes: 0,
        truncated: false,
    };
    ProcessResult {
        termination: cowork_contracts::ProcessTermination::Exited,
        process_id: Some(1),
        exit_code: Some(0),
        signal: None,
        stdout: empty.clone(),
        stderr: empty,
        duration_ms: 1,
        output_complete: true,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a new private PostgreSQL16 database named cw_runner_lease_fixture_* via COWORK_TEST_RUNNER_LEASE_POSTGRES"]
async fn postgres_durable_signed_execution_lease_replay_cas_clock_and_corruption_contract() {
    let url =
        std::env::var("COWORK_TEST_RUNNER_LEASE_POSTGRES").expect("private fixture URL required");
    let first = pool(&url).await;
    let database: String = sqlx::query_scalar("SELECT current_database()::TEXT")
        .fetch_one(&first)
        .await
        .unwrap();
    assert!(database.starts_with("cw_runner_lease_fixture_"));
    let table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='public'",
    )
    .fetch_one(&first)
    .await
    .unwrap();
    assert_eq!(
        table_count, 0,
        "test must never adopt an existing product database"
    );
    let control = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let runner = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let binding = Binding {
        runner_id: "private-postgres-runner".into(),
        generation: 7,
        adapter_build_sha256: "a".repeat(64),
        control_public_key: control.public_key(),
        runner_public_key: runner.public_key(),
    };
    let a = Broker::open(first.clone(), binding.clone(), runner.clone())
        .await
        .unwrap();
    let second = pool(&url).await;
    let b = Broker::open(second.clone(), binding.clone(), runner.clone())
        .await
        .unwrap();
    let now = 1_700_000_000_000;
    let signed = launch(&binding, &control, "competition", now);
    let (one, two) = tokio::join!(
        a.admit_at(signed.clone(), now),
        b.admit_at(signed.clone(), now)
    );
    let (owner, p) = match (one.unwrap(), two.unwrap()) {
        (Admission::Launch(p), Admission::Existing(_)) => (&a, p),
        (Admission::Existing(_), Admission::Launch(p)) => (&b, p),
        _ => panic!("exactly one independent pool may obtain dispatch authority"),
    };
    let dispatched = owner.dispatch_at(p, now).await.unwrap();
    assert!(a.execution_allowed_at(&dispatched, now + 1).await.unwrap());
    let x = renewal(&control, &signed, 12, now + 3_000, now + 13_000);
    let y = renewal(&control, &signed, 12, now + 3_000, now + 14_000);
    let (left, right) = tokio::join!(
        a.renew_execution_lease_at(x.clone(), now + 3_000),
        b.renew_execution_lease_at(y.clone(), now + 3_000)
    );
    let accepted = match (left, right) {
        (Ok(()), Err(Error::Conflict)) => x,
        (Err(Error::Conflict), Ok(())) => y,
        other => panic!("exactly one conflicting signed revision may commit: {other:?}"),
    };
    let saved: String = sqlx::query_scalar(
        "SELECT payload FROM runner_execution_lease_renewals WHERE attempt_id=$1",
    )
    .bind(&signed.payload.process_attempt_id)
    .fetch_one(&first)
    .await
    .unwrap();
    let stored: Signed<AcceptedReceipt> = serde_json::from_str(&saved).unwrap();
    stored
        .verify("execution_lease_receipt", &binding.runner_public_key)
        .unwrap();
    stored
        .payload
        .renewal
        .verify("execution_lease", &binding.control_public_key)
        .unwrap();
    assert_eq!(stored.payload.renewal, accepted);
    assert_eq!(stored.payload.previous_revision, 11);
    assert_eq!(stored.payload.previous_expires_at, now + 10_000);
    assert_eq!(stored.payload.accepted_at, now + 3_000);
    let (left, right) = tokio::join!(
        a.renew_execution_lease_at(accepted.clone(), now + 4_000),
        b.renew_execution_lease_at(accepted.clone(), now + 4_000)
    );
    left.unwrap();
    right.unwrap();
    let replayed: String = sqlx::query_scalar(
        "SELECT payload FROM runner_execution_lease_renewals WHERE attempt_id=$1",
    )
    .bind(&signed.payload.process_attempt_id)
    .fetch_one(&second)
    .await
    .unwrap();
    assert_eq!(saved, replayed);
    assert!(matches!(
        a.admit_at(signed.clone(), now + 70_000).await.unwrap(),
        Admission::Existing(_)
    ));
    println!("PG case admission_dispatch_conflicting_revision_exact_replay: PASS");
    first.close().await;
    second.close().await;
    let reopened_pool = pool(&url).await;
    let reopened = Broker::open(reopened_pool.clone(), binding.clone(), runner.clone())
        .await
        .unwrap();
    assert!(
        reopened
            .execution_allowed_at(&dispatched, now + 10_001)
            .await
            .unwrap()
    );
    assert!(
        !reopened
            .execution_allowed_at(&dispatched, accepted.payload.expires_at)
            .await
            .unwrap()
    );
    reopened
        .renew_execution_lease_at(accepted.clone(), accepted.payload.expires_at + 1)
        .await
        .unwrap();
    assert!(
        !reopened
            .execution_allowed_at(&dispatched, accepted.payload.expires_at + 1)
            .await
            .unwrap()
    );
    assert!(matches!(
        reopened
            .renew_execution_lease_at(
                renewal(
                    &control,
                    &signed,
                    13,
                    accepted.payload.expires_at + 1,
                    now + 24_000
                ),
                accepted.payload.expires_at + 1
            )
            .await,
        Err(Error::Expired)
    ));
    assert!(matches!(
        reopened
            .execution_allowed_at(&dispatched, accepted.payload.expires_at - 1)
            .await,
        Err(Error::ClockRollback)
    ));
    println!("PG case reopen_expiry_historical_replay_no_resurrection_clock: PASS");

    let now = now + 100_000;
    let signed = launch(&binding, &control, "bounds", now);
    let dispatched = dispatch(&reopened, &signed, now).await;
    let r = renewal(&control, &signed, 12, now + 3_000, now + 13_000);
    for changed in [
        renewal(&control, &signed, 12, now + 3_000, now + 33_001),
        renewal(&control, &signed, 12, now + 30_000, now + 60_001),
    ] {
        assert!(matches!(
            reopened
                .renew_execution_lease_at(changed, now + 3_000)
                .await,
            Err(Error::Authority)
        ));
    }
    assert!(matches!(
        reopened
            .renew_execution_lease_at(r.clone(), now + 2_999)
            .await,
        Err(Error::Expired)
    ));
    let mut changed = r.payload.clone();
    changed.worker_fence_sha256 = "f".repeat(64);
    assert!(matches!(
        reopened
            .renew_execution_lease_at(
                control.sign("execution_lease", changed).unwrap(),
                now + 3_000
            )
            .await,
        Err(Error::Authority)
    ));
    assert!(matches!(
        reopened
            .renew_execution_lease_at(
                control.sign("fence", r.payload.clone()).unwrap(),
                now + 3_000
            )
            .await,
        Err(Error::Authority)
    ));
    reopened
        .renew_execution_lease_at(r.clone(), now + 3_000)
        .await
        .unwrap();
    let row_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runner_execution_lease_renewals WHERE attempt_id=$1",
    )
    .bind(&signed.payload.process_attempt_id)
    .fetch_one(&reopened_pool)
    .await
    .unwrap();
    assert_eq!(row_count, 1, "denied inputs must not add durable authority");
    println!("PG case_signature_binding_ttl_original_deadline_future_issuance: PASS");
    let payload: String = sqlx::query_scalar(
        "SELECT payload FROM runner_execution_lease_renewals WHERE attempt_id=$1",
    )
    .bind(&signed.payload.process_attempt_id)
    .fetch_one(&reopened_pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE runner_execution_lease_renewals SET expires_at=expires_at+1 WHERE attempt_id=$1",
    )
    .bind(&signed.payload.process_attempt_id)
    .execute(&reopened_pool)
    .await
    .unwrap();
    assert!(matches!(
        reopened
            .execution_allowed_at(&dispatched, now + 4_000)
            .await,
        Err(Error::Authority)
    ));
    sqlx::query("UPDATE runner_execution_lease_renewals SET expires_at=$1 WHERE attempt_id=$2")
        .bind(now + 13_000)
        .bind(&signed.payload.process_attempt_id)
        .execute(&reopened_pool)
        .await
        .unwrap();
    let mut forged: Signed<AcceptedReceipt> = serde_json::from_str(&payload).unwrap();
    forged.signature[0] ^= 1;
    sqlx::query("UPDATE runner_execution_lease_renewals SET payload=$1 WHERE attempt_id=$2")
        .bind(serde_json::to_string(&forged).unwrap())
        .bind(&signed.payload.process_attempt_id)
        .execute(&reopened_pool)
        .await
        .unwrap();
    assert!(matches!(
        reopened
            .execution_allowed_at(&dispatched, now + 4_000)
            .await,
        Err(Error::Authority)
    ));
    let raw = renewal(&control, &signed, 12, now + 15_000, now + 25_000);
    sqlx::query(
        "UPDATE runner_execution_lease_renewals SET expires_at=$1,payload=$2 WHERE attempt_id=$3",
    )
    .bind(raw.payload.expires_at)
    .bind(serde_json::to_string(&raw).unwrap())
    .bind(&signed.payload.process_attempt_id)
    .execute(&reopened_pool)
    .await
    .unwrap();
    assert!(
        reopened
            .execution_allowed_at(&dispatched, now + 16_000)
            .await
            .is_err()
    );
    sqlx::query(
        "UPDATE runner_execution_lease_renewals SET expires_at=$1,payload=$2 WHERE attempt_id=$3",
    )
    .bind(now + 13_000)
    .bind(&payload)
    .bind(&signed.payload.process_attempt_id)
    .execute(&reopened_pool)
    .await
    .unwrap();
    assert!(
        reopened
            .execution_allowed_at(&dispatched, now + 4_000)
            .await
            .unwrap()
    );
    println!("PG case_signed_receipt_index_signature_and_raw_message_injection: PASS");
    let fence = FenceRequest {
        runner_id: binding.runner_id.clone(),
        runner_generation: binding.generation,
        run_id: signed.payload.run_id.clone(),
        lease_epoch: 11,
        revoked: true,
    };
    reopened
        .fence(control.sign("fence", fence.clone()).unwrap())
        .await
        .unwrap();
    assert!(
        !reopened
            .execution_allowed_at(&dispatched, now + 4_001)
            .await
            .unwrap()
    );
    assert!(matches!(
        reopened
            .renew_execution_lease_at(
                renewal(&control, &signed, 13, now + 4_001, now + 14_001),
                now + 4_001
            )
            .await,
        Err(Error::Authority)
    ));
    let mut reopen = fence;
    reopen.revoked = false;
    assert!(matches!(
        reopened
            .fence(control.sign("fence", reopen.clone()).unwrap())
            .await,
        Err(Error::Authority)
    ));
    reopen.lease_epoch += 1;
    assert!(matches!(
        reopened.fence(control.sign("fence", reopen).unwrap()).await,
        Err(Error::Uncertain)
    ));
    // A physical-result fixture tests persistence, not an actual observed OS exit.
    let result = physical_result_fixture();
    let one = reopened
        .complete(
            &signed.payload.process_attempt_id,
            &signed.payload.request_sha256,
            result.clone(),
        )
        .await
        .unwrap();
    let two = reopened
        .complete(
            &signed.payload.process_attempt_id,
            &signed.payload.request_sha256,
            result,
        )
        .await
        .unwrap();
    assert_eq!(one, two);
    one.verify("receipt", &binding.runner_public_key).unwrap();
    assert_eq!(one.payload.revision, 3);
    assert!(
        !reopened
            .execution_allowed_at(&dispatched, now + 4_002)
            .await
            .unwrap()
    );
    assert!(matches!(
        reopened
            .renew_execution_lease_at(
                renewal(&control, &signed, 13, now + 4_002, now + 14_002),
                now + 4_002
            )
            .await,
        Err(Error::Uncertain)
    ));
    println!("PG case_fence_revocation_no_reopen_and_immutable_terminal_receipt: PASS");
    reopened_pool.close().await;
}
