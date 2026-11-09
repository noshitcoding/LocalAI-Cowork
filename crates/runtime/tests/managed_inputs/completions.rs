//! Original signed history does not mint input, output or execution authority.
use super::*;
use cowork_runner_protocol::{ExecutionState, envelope_hash};

fn result() -> ProcessResult {
    ProcessResult {
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
    }
}
fn receipt(f: &Fixture, request: &Signed<LaunchRequest>) -> Signed<Receipt> {
    f.runner
        .key
        .sign(
            "receipt",
            Receipt {
                request: request.payload.clone(),
                request_envelope_sha256: envelope_hash(request),
                state: ExecutionState::Completed { result: result() },
                revision: 3,
                updated_at: now(),
            },
        )
        .unwrap()
}

#[tokio::test]
async fn original_completion_is_retained_before_process_settlement_and_replays_exactly() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (run, token, request) = f.admitted(&plan).await;
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request.clone())
        .await
        .unwrap();
    let signed = receipt(&f, &request);
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    assert_eq!(
        f.runtime
            .store
            .retain_managed_completion_receipt(&signed)
            .await
            .unwrap(),
        signed
    );
    assert_eq!(
        f.runtime
            .store
            .retain_managed_completion_receipt(&signed)
            .await
            .unwrap(),
        signed
    );
    assert_eq!(
        f.runtime
            .store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .revision,
        before.record.revision
    );
    let attempt = f
        .runtime
        .store
        .process_attempts(&run.id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(attempt.state, ProcessAttemptState::LaunchCommitted);
    assert_eq!(
        f.runtime
            .store
            .managed_completion_receipt(&run.id, &attempt.id)
            .await
            .unwrap(),
        Some(signed.clone())
    );
    f.runtime
        .store
        .finish_process(&run.id, &token, &request.payload.tool_call_id, result())
        .await
        .unwrap();
    assert_eq!(
        f.runtime
            .store
            .retain_managed_completion_receipt(&signed)
            .await
            .unwrap(),
        signed
    );
    assert!(source.manifest().await.is_err());
    let before_count = f.runtime.store.runs(f.order()).await.unwrap().len();
    assert_eq!(before_count, 1);
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    drop(source);
    f.close().await;
}

#[tokio::test]
async fn no_input_custody_means_no_managed_completion_history() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (run, _, request) = f.admitted(&plan).await;
    assert!(
        f.runtime
            .store
            .retain_managed_completion_receipt(&receipt(&f, &request))
            .await
            .is_err()
    );
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
async fn changed_signed_completion_intents_signer_and_result_are_rejected() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (_, token, request) = f.admitted(&plan).await;
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request.clone())
        .await
        .unwrap();
    let original = receipt(&f, &request);
    for change in 0..7 {
        let mut value = original.payload.clone();
        match change {
            0 => value.request.workspace_resource_id.push('x'),
            1 => value.request_envelope_sha256 = "0".repeat(64),
            2 => value.revision = 2,
            3 => value.state = ExecutionState::Admitted,
            4 => value.updated_at = request.payload.issued_at - 1,
            5 => {
                if let ExecutionState::Completed { result } = &mut value.state {
                    result.stdout.sha256 = "0".repeat(64)
                }
            }
            _ => value.request.invocation.arguments.push("changed".into()),
        }
        assert!(
            f.runtime
                .store
                .retain_managed_completion_receipt(&f.runner.key.sign("receipt", value).unwrap())
                .await
                .is_err(),
            "change {change}"
        );
    }
    assert!(
        f.runtime
            .store
            .retain_managed_completion_receipt(
                &f.control.sign("receipt", original.payload.clone()).unwrap()
            )
            .await
            .is_err()
    );
    assert_eq!(
        f.runtime
            .store
            .retain_managed_completion_receipt(&original)
            .await
            .unwrap(),
        original
    );
    drop(source);
    f.close().await;
}

#[tokio::test]
async fn original_completion_never_overwrites_a_different_signed_history() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (run, token, request) = f.admitted(&plan).await;
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request.clone())
        .await
        .unwrap();
    let original = receipt(&f, &request);
    f.runtime
        .store
        .retain_managed_completion_receipt(&original)
        .await
        .unwrap();
    let mut changed = original.payload.clone();
    changed.updated_at += 1;
    assert!(matches!(
        f.runtime
            .store
            .retain_managed_completion_receipt(&f.runner.key.sign("receipt", changed).unwrap())
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        f.runtime
            .store
            .managed_completion_receipt(&run.id, &request.payload.process_attempt_id)
            .await
            .unwrap(),
        Some(original)
    );
    drop(source);
    f.close().await;
}

#[tokio::test]
async fn observed_completion_does_not_revive_canceled_worker_or_input_rights() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (run, token, request) = f.admitted(&plan).await;
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request.clone())
        .await
        .unwrap();
    f.runtime.store.cancel(&run.id).await.unwrap();
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    let original = receipt(&f, &request);
    f.runtime
        .store
        .retain_managed_completion_receipt(&original)
        .await
        .unwrap();
    let after = f.runtime.store.stored_run(&run.id).await.unwrap();
    assert_eq!(before.record.revision, after.record.revision);
    assert_eq!(before.record.state, after.record.state);
    assert!(source.manifest().await.is_err());
    assert!(
        f.runtime
            .prepare_managed_process_input(&token, request.clone())
            .await
            .is_err()
    );
    assert!(
        f.runtime
            .store
            .managed_completion_receipt("foreign-run", &request.payload.process_attempt_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    drop(source);
    f.close().await;
}

#[tokio::test]
async fn proven_nonstart_cannot_adopt_a_later_signed_completed_history() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (_, token, request, permit) = f.admitted_with_permit(&plan).await;
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request.clone())
        .await
        .unwrap();
    permit
        .reject_before_transport(&token, "test-local-rejection")
        .await
        .unwrap();
    assert!(matches!(
        f.runtime
            .store
            .retain_managed_completion_receipt(&receipt(&f, &request))
            .await,
        Err(Error::Conflict)
    ));
    drop(source);
    f.close().await;
}

#[tokio::test]
async fn corrupted_completion_custody_is_not_returned_as_original_history() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (run, token, request) = f.admitted(&plan).await;
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request.clone())
        .await
        .unwrap();
    let original = receipt(&f, &request);
    f.runtime
        .store
        .retain_managed_completion_receipt(&original)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE owner_managed_completion_receipts SET receipt_sha256=$1 WHERE attempt_id=$2",
    )
    .bind("0".repeat(64))
    .bind(&request.payload.process_attempt_id)
    .execute(&f.pool)
    .await
    .unwrap();
    assert!(
        f.runtime
            .store
            .managed_completion_receipt(&run.id, &request.payload.process_attempt_id)
            .await
            .is_err()
    );
    assert!(
        f.runtime
            .store
            .retain_managed_completion_receipt(&original)
            .await
            .is_err()
    );
    drop(source);
    f.close().await;
}
