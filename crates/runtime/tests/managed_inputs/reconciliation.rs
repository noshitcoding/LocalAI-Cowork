//! Signed Store recovery using actual plan/input/attempt custody and synthetic
//! runner keys. No physical stop or service qualification is inferred.
use super::*;
use cowork_runner_protocol::{ExecutionState, envelope_hash};
#[path = "reconciliation_transport.rs"]
mod transport_tests;

pub(super) fn completion(f: &Fixture, request: &Signed<LaunchRequest>) -> Signed<Receipt> {
    f.runner
        .key
        .sign(
            "receipt",
            Receipt {
                request: request.payload.clone(),
                request_envelope_sha256: envelope_hash(request),
                state: ExecutionState::Completed {
                    result: ProcessResult {
                        termination: ProcessTermination::Exited,
                        process_id: Some(42),
                        exit_code: Some(0),
                        signal: None,
                        stdout: ProcessCapture {
                            bytes: b"copy done".to_vec(),
                            sha256: hash(b"copy done"),
                            total_bytes: 9,
                            truncated: false,
                        },
                        stderr: ProcessCapture {
                            bytes: vec![],
                            sha256: hash(b""),
                            total_bytes: 0,
                            truncated: false,
                        },
                        duration_ms: 5,
                        output_complete: true,
                    },
                },
                revision: 3,
                updated_at: now(),
            },
        )
        .unwrap()
}

pub(super) async fn prepare() -> (Fixture, RunRecord, String, Signed<LaunchRequest>, String) {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (run, token, request) = f.admitted(&plan).await;
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request.clone())
        .await
        .unwrap();
    drop(source);
    let origin = f
        .runtime
        .files
        .as_ref()
        .unwrap()
        .local_controller_origin()
        .unwrap();
    (f, run, token, request, origin)
}

pub(super) async fn interrupt(f: &Fixture, run: &str, token: &str) -> ReconcileOwnerManagedProcess {
    // Exercise the actual expired-worker recovery. It retains the uncertain
    // in-flight operation and clears only the expired worker ownership.
    f.runtime
        .store
        .mutate(run, Some(token), |r| {
            r.lease.as_mut().unwrap().until = now() - 1;
            Ok(Change::event("fixture_worker_lost", json!({})))
        })
        .await
        .unwrap();
    f.runtime.store.recover_expired_leases().await.unwrap();
    let saved = f.runtime.store.stored_run(run).await.unwrap();
    assert_eq!(saved.record.state, RunState::Interrupted);
    assert!(saved.lease.is_none());
    let process = f
        .runtime
        .store
        .process_attempts(run)
        .await
        .unwrap()
        .pop()
        .unwrap();
    ReconcileOwnerManagedProcess {
        expected_run_revision: saved.record.revision,
        expected_request_sha256: process.request_sha256,
        finish_cancellation: true,
    }
}

pub(super) async fn reconcile(
    f: &Fixture,
    run: &str,
    request: &Signed<LaunchRequest>,
    origin: &str,
    intent: &ReconcileOwnerManagedProcess,
    signed: &Signed<Receipt>,
) -> cowork_domain::Result<OwnerManagedProcessReconciliation> {
    f.runtime
        .store
        .reconcile_owner_managed_process(
            f.order(),
            run,
            &request.payload.process_attempt_id,
            origin,
            intent,
            signed,
        )
        .await
}

#[tokio::test]
async fn expired_original_input_ttl_does_not_block_signed_history_or_revive_content() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (run, token, original) = f.admitted(&plan).await;
    let mut launch = original.payload;
    launch.issued_at = now() - 24_000;
    launch.expires_at = (now() + 5_000).min(launch.expires_at);
    let request = f.control.sign("launch", launch).unwrap();
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request.clone())
        .await
        .unwrap();
    let signed = completion(&f, &request);
    let delay = (request.payload.expires_at - now() + 25).max(0) as u64;
    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    assert!(now() > request.payload.expires_at);
    assert!(source.manifest().await.is_err());
    drop(source);
    let intent = interrupt(&f, &run.id, &token).await;
    let origin = f
        .runtime
        .files
        .as_ref()
        .unwrap()
        .local_controller_origin()
        .unwrap();
    assert_eq!(
        reconcile(&f, &run.id, &request, &origin, &intent, &signed)
            .await
            .unwrap()
            .run
            .state,
        RunState::Canceled
    );
    assert!(
        f.runtime
            .prepare_managed_process_input(&token, request.clone())
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn physical_cancellation_with_unknown_exit_is_retained_without_inventing_exit_zero() {
    let (f, run, token, request, origin) = prepare().await;
    let intent = interrupt(&f, &run.id, &token).await;
    let mut body = completion(&f, &request).payload;
    let ExecutionState::Completed { result } = &mut body.state else {
        unreachable!()
    };
    result.termination = ProcessTermination::Canceled;
    result.process_id = None;
    result.exit_code = None;
    result.output_complete = false;
    let signed = f.runner.key.sign("receipt", body).unwrap();
    let answer = reconcile(&f, &run.id, &request, &origin, &intent, &signed)
        .await
        .unwrap();
    assert_eq!(answer.run.state, RunState::Canceled);
    let ProcessAttemptState::ReconciledCompleted { result, .. } = answer.process_attempt.state
    else {
        panic!("signed physical result")
    };
    assert_eq!(result.termination, ProcessTermination::Canceled);
    assert!(result.exit_code.is_none() && result.process_id.is_none());
    assert!(!result.output_complete);
    f.close().await;
}

#[tokio::test]
async fn signed_stop_settles_once_and_cancels_atomically_without_changing_physical_exit() {
    let (f, run, token, request, origin) = prepare().await;
    f.runtime.store.cancel(&run.id).await.unwrap();
    assert_eq!(
        f.runtime
            .store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .state,
        RunState::CancelRequested
    );
    let intent = interrupt(&f, &run.id, &token).await;
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    let signed = completion(&f, &request);
    let answer = reconcile(&f, &run.id, &request, &origin, &intent, &signed)
        .await
        .unwrap();
    assert_eq!(answer.run.state, RunState::Canceled);
    assert_eq!(answer.run.revision, before.record.revision + 1);
    let ProcessAttemptState::ReconciledCompleted {
        result,
        evidence_sha256,
    } = &answer.process_attempt.state
    else {
        panic!("signed reconciliation");
    };
    assert_eq!(evidence_sha256, &canonical_hash(&signed));
    assert_eq!(result.termination, ProcessTermination::Exited);
    assert_eq!(result.exit_code, Some(0));
    let saved = f.runtime.store.stored_run(&run.id).await.unwrap();
    assert!(
        saved.lease.is_none()
            && saved.checkpoint.in_flight.is_none()
            && saved.checkpoint.pending_tools.is_empty()
    );
    assert_eq!(saved.record.steps, before.record.steps + 1);
    assert_eq!(
        saved.checkpoint.messages.len(),
        before.checkpoint.messages.len() + 1
    );
    let replay = reconcile(&f, &run.id, &request, &origin, &intent, &signed)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&replay), canonical_hash(&answer));
    assert_eq!(
        canonical_hash(&saved),
        canonical_hash(&f.runtime.store.stored_run(&run.id).await.unwrap())
    );
    assert!(f.runtime.store.resume(&run.id).await.is_err());
    assert!(
        f.runtime
            .prepare_managed_process_input(&token, request.clone())
            .await
            .is_err()
    );
    assert_eq!(
        f.runtime
            .store
            .process_attempts(&run.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    f.close().await;
}

#[tokio::test]
async fn human_recovery_remains_interrupted_and_finish_flag_replay_cannot_change_it() {
    let (f, run, token, request, origin) = prepare().await;
    let mut intent = interrupt(&f, &run.id, &token).await;
    intent.finish_cancellation = false;
    let signed = completion(&f, &request);
    let answer = reconcile(&f, &run.id, &request, &origin, &intent, &signed)
        .await
        .unwrap();
    assert_eq!(answer.run.state, RunState::Interrupted);
    assert_eq!(
        answer.run.error_code.as_deref(),
        Some("executor_interrupted")
    );
    assert_eq!(
        canonical_hash(
            &reconcile(&f, &run.id, &request, &origin, &intent, &signed)
                .await
                .unwrap()
        ),
        canonical_hash(&answer)
    );
    let mut changed = intent.clone();
    changed.finish_cancellation = true;
    assert!(
        reconcile(&f, &run.id, &request, &origin, &changed, &signed)
            .await
            .is_err()
    );
    assert_eq!(
        f.runtime.store.resume(&run.id).await.unwrap().state,
        RunState::Queued
    );
    assert!(
        reconcile(&f, &run.id, &request, &origin, &intent, &signed)
            .await
            .is_err()
    );
    assert_eq!(
        f.runtime
            .store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .steps,
        answer.run.steps
    );
    f.close().await;
}

#[tokio::test]
async fn original_history_survives_revoked_remote_and_source_rights_and_missing_files() {
    let (mut f, run, token, request, origin) = prepare().await;
    let intent = interrupt(&f, &run.id, &token).await;
    let signed = completion(&f, &request);
    f.runtime
        .store
        .set_execution_target_active(&f.identity, 2, false)
        .await
        .unwrap();
    let reference = run.spec.environment.as_ref().unwrap();
    let environment = f
        .runtime
        .store
        .environment(&run.spec.project_id, &reference.environment_id)
        .await
        .unwrap();
    f.runtime
        .store
        .change_environment(
            &run.spec.project_id,
            &reference.environment_id,
            environment.revision,
            "revoke-for-history",
            &EnvironmentChange::RevokeVersion {
                version_id: reference.version_id.clone(),
            },
        )
        .await
        .unwrap();
    let RunRequest::Single(input) = &f.input.request else {
        unreachable!()
    };
    let source = input.folders.iter().find(|g| g.id == "source").unwrap();
    f.runtime
        .files
        .as_ref()
        .unwrap()
        .revoke_native_folder(&source.project_id, &source.id, &source.revision)
        .unwrap();
    f.runtime.files = None;
    std::fs::rename(
        f.dir.path().join("originals"),
        f.dir.path().join("removed-originals"),
    )
    .unwrap();
    std::fs::rename(
        f.dir.path().join("broker"),
        f.dir.path().join("removed-broker"),
    )
    .unwrap();
    let history = f
        .runtime
        .store
        .owner_managed_process_history_context(
            f.order(),
            &run.id,
            &request.payload.process_attempt_id,
            &origin,
        )
        .await
        .unwrap();
    assert!(history.receipt.is_none());
    assert_eq!(history.input.request, request);
    assert_eq!(
        reconcile(&f, &run.id, &request, &origin, &intent, &signed)
            .await
            .unwrap()
            .run
            .state,
        RunState::Canceled
    );
    assert!(!f.dir.path().join("originals").exists());
    assert!(!f.dir.path().join("broker").exists());
    f.close().await;
}

#[tokio::test]
async fn active_run_stale_intent_foreign_scope_and_changed_receipt_never_settle() {
    let (f, run, token, request, origin) = prepare().await;
    let signed = completion(&f, &request);
    let current = f.runtime.store.stored_run(&run.id).await.unwrap();
    let intent = ReconcileOwnerManagedProcess {
        expected_run_revision: current.record.revision,
        expected_request_sha256: request.payload.request_sha256.clone(),
        finish_cancellation: true,
    };
    assert!(
        reconcile(&f, &run.id, &request, &origin, &intent, &signed)
            .await
            .is_err()
    );
    let intent = interrupt(&f, &run.id, &token).await;
    let original = f.runtime.store.stored_run(&run.id).await.unwrap();
    for (order, run_id, attempt, native) in [
        (
            "foreign",
            run.id.as_str(),
            request.payload.process_attempt_id.as_str(),
            origin.as_str(),
        ),
        (
            f.order(),
            "foreign",
            request.payload.process_attempt_id.as_str(),
            origin.as_str(),
        ),
        (f.order(), run.id.as_str(), "foreign", origin.as_str()),
        (
            f.order(),
            run.id.as_str(),
            request.payload.process_attempt_id.as_str(),
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        ),
    ] {
        assert!(
            f.runtime
                .store
                .reconcile_owner_managed_process(order, run_id, attempt, native, &intent, &signed)
                .await
                .is_err()
        );
    }
    for mode in 0..3 {
        let mut bad = intent.clone();
        match mode {
            0 => bad.expected_run_revision -= 1,
            1 => bad.expected_request_sha256 = "0".repeat(64),
            _ => bad.expected_run_revision += 1,
        };
        assert!(
            reconcile(&f, &run.id, &request, &origin, &bad, &signed)
                .await
                .is_err()
        );
    }
    let invalid = f.control.sign("receipt", signed.payload.clone()).unwrap();
    assert!(
        reconcile(&f, &run.id, &request, &origin, &intent, &invalid)
            .await
            .is_err()
    );
    let mut changed = signed.payload.clone();
    changed.request_envelope_sha256 = "0".repeat(64);
    assert!(
        reconcile(
            &f,
            &run.id,
            &request,
            &origin,
            &intent,
            &f.runner.key.sign("receipt", changed).unwrap()
        )
        .await
        .is_err()
    );
    assert_eq!(
        canonical_hash(&original),
        canonical_hash(&f.runtime.store.stored_run(&run.id).await.unwrap())
    );
    assert!(
        f.runtime
            .store
            .managed_completion_receipt(&run.id, &request.payload.process_attempt_id)
            .await
            .unwrap()
            .is_none()
    );
    f.runtime
        .store
        .retain_managed_completion_receipt(&signed)
        .await
        .unwrap();
    let mut changed = signed.payload.clone();
    changed.updated_at += 1;
    assert!(
        reconcile(
            &f,
            &run.id,
            &request,
            &origin,
            &intent,
            &f.runner.key.sign("receipt", changed).unwrap()
        )
        .await
        .is_err()
    );
    assert_eq!(
        canonical_hash(&original),
        canonical_hash(&f.runtime.store.stored_run(&run.id).await.unwrap())
    );
    f.close().await;
}

#[tokio::test]
async fn workflow_or_different_active_attempt_cannot_use_owner_reconciliation() {
    let (f, run, token, request, origin) = prepare().await;
    let intent = interrupt(&f, &run.id, &token).await;
    let signed = completion(&f, &request);
    let original = f.runtime.store.stored_run(&run.id).await.unwrap();
    let mut changed = original.clone();
    changed.checkpoint.pending_tools[0].id.push('x');
    sqlx::query("UPDATE runs SET payload=$1 WHERE id=$2")
        .bind(serde_json::to_string(&changed).unwrap())
        .bind(&run.id)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        reconcile(&f, &run.id, &request, &origin, &intent, &signed)
            .await
            .is_err()
    );
    let mut workflow = original.clone();
    workflow.record.spec.workflow = Some(WorkflowBinding {
        inputs: None,
        workflow_id: "workflow".into(),
        version_id: "version".into(),
        definition_sha256: "a".repeat(64),
        plan_sha256: "b".repeat(64),
    });
    sqlx::query("UPDATE runs SET payload=$1 WHERE id=$2")
        .bind(serde_json::to_string(&workflow).unwrap())
        .bind(&run.id)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        f.runtime
            .store
            .owner_managed_process_history_context(
                f.order(),
                &run.id,
                &request.payload.process_attempt_id,
                &origin
            )
            .await
            .is_err()
    );
    assert!(
        reconcile(&f, &run.id, &request, &origin, &intent, &signed)
            .await
            .is_err()
    );
    sqlx::query("UPDATE runs SET payload=$1 WHERE id=$2")
        .bind(serde_json::to_string(&original).unwrap())
        .bind(&run.id)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        f.runtime
            .store
            .managed_completion_receipt(&run.id, &request.payload.process_attempt_id)
            .await
            .unwrap()
            .is_none()
    );
    f.close().await;
}

#[tokio::test]
async fn concurrent_exact_commands_record_only_one_checkpoint_result() {
    let (f, run, token, request, origin) = prepare().await;
    let intent = interrupt(&f, &run.id, &token).await;
    let signed = completion(&f, &request);
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    let (left, right) = tokio::join!(
        reconcile(&f, &run.id, &request, &origin, &intent, &signed),
        reconcile(&f, &run.id, &request, &origin, &intent, &signed)
    );
    assert_eq!(
        canonical_hash(&left.unwrap()),
        canonical_hash(&right.unwrap())
    );
    let after = f.runtime.store.stored_run(&run.id).await.unwrap();
    assert_eq!(after.record.revision, before.record.revision + 1);
    assert_eq!(after.record.steps, before.record.steps + 1);
    assert_eq!(
        after.checkpoint.messages.len(),
        before.checkpoint.messages.len() + 1
    );
    f.close().await;
}

#[tokio::test]
async fn sql_failure_rolls_back_receipt_checkpoint_and_cancellation_together() {
    let (f, run, token, request, origin) = prepare().await;
    let intent = interrupt(&f, &run.id, &token).await;
    let signed = completion(&f, &request);
    let postgres = std::env::var("COWORK_TEST_MANAGED_INPUT_DATABASE_URL").is_ok();
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    if postgres {
        sqlx::query("CREATE FUNCTION fixture_fail_owner_settle() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'fixture_settle_failure'; END; $$ LANGUAGE plpgsql")
            .execute(&f.pool).await.unwrap();
    }
    sqlx::query(if postgres {
        "CREATE TRIGGER fail_owner_settle BEFORE UPDATE ON process_attempts FOR EACH ROW EXECUTE FUNCTION fixture_fail_owner_settle()"
    } else { "CREATE TRIGGER fail_owner_settle BEFORE UPDATE ON process_attempts BEGIN SELECT RAISE(ABORT,'fixture_settle_failure'); END;" })
        .execute(&f.pool).await.unwrap();
    assert!(
        reconcile(&f, &run.id, &request, &origin, &intent, &signed)
            .await
            .is_err()
    );
    assert_eq!(
        canonical_hash(&before),
        canonical_hash(&f.runtime.store.stored_run(&run.id).await.unwrap())
    );
    assert!(
        f.runtime
            .store
            .managed_completion_receipt(&run.id, &request.payload.process_attempt_id)
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query(if postgres {
        "DROP TRIGGER fail_owner_settle ON process_attempts"
    } else {
        "DROP TRIGGER fail_owner_settle"
    })
    .execute(&f.pool)
    .await
    .unwrap();
    if postgres {
        sqlx::query("DROP FUNCTION fixture_fail_owner_settle()")
            .execute(&f.pool)
            .await
            .unwrap();
    }
    assert_eq!(
        reconcile(&f, &run.id, &request, &origin, &intent, &signed)
            .await
            .unwrap()
            .run
            .state,
        RunState::Canceled
    );
    f.close().await;
}

#[tokio::test]
async fn commit_rechecks_origin_after_initial_history_and_rolls_back_a_changed_origin() {
    let (f, run, token, request, origin) = prepare().await;
    let intent = interrupt(&f, &run.id, &token).await;
    let signed = completion(&f, &request);
    let postgres = std::env::var("COWORK_TEST_MANAGED_INPUT_DATABASE_URL").is_ok();
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    if postgres {
        sqlx::query("CREATE FUNCTION fixture_change_owner_origin() RETURNS trigger AS $$ BEGIN IF NEW.revision=OLD.revision+1 THEN UPDATE broker_controller SET origin_sha256='ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'; END IF; RETURN NEW; END; $$ LANGUAGE plpgsql")
            .execute(&f.pool).await.unwrap();
    }
    sqlx::query(if postgres { "CREATE TRIGGER change_origin_on_settle AFTER UPDATE ON runs FOR EACH ROW EXECUTE FUNCTION fixture_change_owner_origin()" }
        else { "CREATE TRIGGER change_origin_on_settle AFTER UPDATE ON runs WHEN NEW.revision=OLD.revision+1 BEGIN UPDATE broker_controller SET origin_sha256='ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'; END;" })
        .execute(&f.pool).await.unwrap();
    assert!(
        reconcile(&f, &run.id, &request, &origin, &intent, &signed)
            .await
            .is_err()
    );
    assert_eq!(
        canonical_hash(&before),
        canonical_hash(&f.runtime.store.stored_run(&run.id).await.unwrap())
    );
    assert!(
        f.runtime
            .store
            .managed_completion_receipt(&run.id, &request.payload.process_attempt_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.runtime.store.broker_controller(&origin).await.unwrap().id,
        f.runtime
            .store
            .owner_managed_process_history_context(
                f.order(),
                &run.id,
                &request.payload.process_attempt_id,
                &origin
            )
            .await
            .unwrap()
            .plan
            .plan
            .target
            .controller()
            .id
    );
    sqlx::query(if postgres {
        "DROP TRIGGER change_origin_on_settle ON runs"
    } else {
        "DROP TRIGGER change_origin_on_settle"
    })
    .execute(&f.pool)
    .await
    .unwrap();
    if postgres {
        sqlx::query("DROP FUNCTION fixture_change_owner_origin()")
            .execute(&f.pool)
            .await
            .unwrap();
    }
    f.close().await;
}
