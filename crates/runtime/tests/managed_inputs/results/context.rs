//! The real Runtime/Store/native-copy path with explicitly synthetic observation
//! signatures. Physical stop and native fence behavior require separate probes.
use super::*;

#[tokio::test]
async fn reconciled_interrupted_original_can_receive_without_resume_or_second_launch() {
    let (f, runtime, peer, run, token) = prepared(false).await;
    let mut intent = super::super::reconciliation::interrupt(&f, &run.id, &token).await;
    intent.finish_cancellation = false;
    let settled = runtime
        .reconcile_owner_managed_process(
            f.order(),
            &run.id,
            &peer.launch.payload.process_attempt_id,
            intent,
        )
        .await
        .unwrap();
    assert_eq!(settled.run.state, RunState::Interrupted);
    assert!(matches!(
        settled.process_attempt.state,
        ProcessAttemptState::ReconciledCompleted { .. }
    ));
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    let version_id = runtime
        .retain_managed_result_version(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
        .await
        .unwrap();
    let payload: String =
        sqlx::query_scalar("SELECT payload FROM owner_managed_result_versions WHERE id=$1")
            .bind(&version_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let version: cowork_storage::ManagedResultVersion = serde_json::from_str(&payload).unwrap();
    assert_eq!(
        version.state,
        cowork_storage::ManagedResultVersionStatus::Ready
    );
    let after = f.runtime.store.stored_run(&run.id).await.unwrap();
    assert!(after.lease.is_none());
    assert_eq!(after.record.revision, before.record.revision);
    assert_eq!(after.record.state, RunState::Interrupted);
    assert_eq!(
        f.runtime
            .store
            .process_attempts(&run.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    assert!(peer.contexts.load(Ordering::SeqCst) >= 2);
    assert!(
        peer.requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.payload.format_version == 2 && r.payload.context.is_some())
    );
    assert_eq!(
        f.runtime
            .store
            .managed_completion_receipt(&run.id, &peer.launch.payload.process_attempt_id)
            .await
            .unwrap()
            .unwrap(),
        peer.receipt
    );
    assert_eq!(
        std::fs::read(f.dir.path().join("originals/value.txt")).unwrap(),
        b"original"
    );
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn reconciled_business_cancellation_still_denies_new_context_and_bytes() {
    let (f, runtime, peer, run, token) = prepared(false).await;
    let intent = super::super::reconciliation::interrupt(&f, &run.id, &token).await;
    let settled = runtime
        .reconcile_owner_managed_process(
            f.order(),
            &run.id,
            &peer.launch.payload.process_attempt_id,
            intent,
        )
        .await
        .unwrap();
    assert_eq!(settled.run.state, RunState::Canceled);
    assert!(
        runtime
            .retain_managed_result_version(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .is_err()
    );
    assert_eq!(peer.contexts.load(Ordering::SeqCst), 0);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 0);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn uncertain_interrupted_attempt_has_no_content_context() {
    let (f, runtime, peer, run, token) = prepared(false).await;
    super::super::reconciliation::interrupt(&f, &run.id, &token).await;
    assert!(
        runtime
            .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
            .await
            .is_err()
    );
    assert_eq!(peer.contexts.load(Ordering::SeqCst), 0);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 0);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn invalid_or_lost_context_has_no_read_retry_or_legacy_fallback() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    for mode in [10, 11, 12, 13, 15, 16] {
        peer.mode.store(mode, Ordering::SeqCst);
        let before = peer.contexts.load(Ordering::SeqCst);
        assert!(
            runtime
                .managed_result_manifest(
                    f.order(),
                    &run.id,
                    &peer.launch.payload.process_attempt_id
                )
                .await
                .is_err(),
            "mode {mode}"
        );
        assert_eq!(peer.contexts.load(Ordering::SeqCst), before + 1);
        assert_eq!(peer.reads.load(Ordering::SeqCst), 0);
    }
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn cancellation_during_context_observation_discards_it_before_content_post() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    peer.mode.store(14, Ordering::SeqCst);
    assert!(
        runtime
            .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
            .await
            .is_err()
    );
    assert_eq!(peer.contexts.load(Ordering::SeqCst), 1);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 0);
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn target_revocation_during_context_observation_discards_it_before_content_post() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    peer.mode.store(17, Ordering::SeqCst);
    assert!(
        runtime
            .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
            .await
            .is_err()
    );
    assert_eq!(peer.contexts.load(Ordering::SeqCst), 1);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 0);
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    drop(runtime);
    drop(peer);
    f.close().await;
}
