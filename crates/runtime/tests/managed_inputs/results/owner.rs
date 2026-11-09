//! Public projections use real SQL/native custody, with the existing synthetic peer.
use super::*;

fn local(runtime: &Runtime) -> Runtime {
    let mut value = Runtime::new(runtime.store.clone(), vec![], true).unwrap();
    value.files = runtime.files.clone();
    value
}

#[tokio::test]
async fn owner_list_without_reception_is_local_and_allocates_no_stage_or_sql_version() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let local = local(&runtime);
    let first = local.begin_upload_transfer().unwrap();
    let second = local.begin_upload_transfer().unwrap();
    assert!(matches!(
        local.owner_managed_results(f.order(), &run.id).await,
        Err(Error::TransferBusy)
    ));
    assert!(matches!(
        local
            .owner_managed_result_entry(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
            .await,
        Err(Error::TransferBusy)
    ));
    drop(first);
    drop(second);
    let list = local
        .owner_managed_results(f.order(), &run.id)
        .await
        .unwrap();
    assert_eq!(
        list.entries,
        vec![OwnerManagedResultEntry {
            order_id: f.order().into(),
            run_id: run.id.clone(),
            process_attempt_id: peer.launch.payload.process_attempt_id.clone(),
            version: None,
        }]
    );
    assert!(
        local
            .owner_managed_result_manifest(f.order(), &run.id, &uuid::Uuid::new_v4().to_string())
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM owner_managed_result_versions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert!(!f.dir.path().join("broker/managed-results").exists());
    assert_eq!(peer.reads.load(Ordering::SeqCst), 0);
    drop(local);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn owner_receiving_metadata_never_authorizes_ready_reads_or_resumes_on_get() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let intent = prepare_intent(&f, &runtime, &run, &peer).await;
    let local = local(&runtime);
    let entry = local
        .owner_managed_results(f.order(), &run.id)
        .await
        .unwrap()
        .entries
        .remove(0);
    assert_eq!(
        entry.version.as_ref().unwrap().state,
        OwnerManagedResultVersionState::Receiving
    );
    assert_eq!(entry.version.as_ref().unwrap().id, intent.id);
    assert!(
        f.runtime
            .store
            .local_ready_managed_result_version(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id,
                &origin(&f)
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        local
            .owner_managed_result_manifest(f.order(), &run.id, &intent.id)
            .await
            .is_err()
    );
    assert!(
        local
            .owner_managed_result_chunk(f.order(), &run.id, &intent.id, chunk(&intent.result))
            .await
            .is_err()
    );
    assert!(
        local
            .receive_owner_managed_result(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .is_err()
    );
    assert_eq!(stored(&f).await, intent);
    assert!(!f.dir.path().join("broker/managed-results").exists());
    assert_eq!(peer.reads.load(Ordering::SeqCst), 1);
    drop(local);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn owner_ready_projection_manifest_chunk_and_explicit_replay_work_without_runner() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    // One slot remains: retain and entry must use it sequentially.
    let occupied = runtime.begin_upload_transfer().unwrap();
    let ready = runtime
        .receive_owner_managed_result(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
        .await
        .unwrap();
    drop(occupied);
    let version = ready.version.as_ref().unwrap();
    assert_eq!(version.state, OwnerManagedResultVersionState::Ready);
    assert_eq!(version.file_count, 1);
    assert_eq!(version.total_bytes, 13);
    let original = stored(&f).await;
    runtime
        .store
        .set_execution_target_active(&f.identity, 2, false)
        .await
        .unwrap();
    let local = local(&runtime);
    assert_eq!(
        local
            .owner_managed_results(f.order(), &run.id)
            .await
            .unwrap()
            .entries,
        vec![ready.clone()]
    );
    let manifest = local
        .owner_managed_result_manifest(f.order(), &run.id, &version.id)
        .await
        .unwrap();
    assert_eq!(manifest.entry, ready);
    assert_eq!(manifest.manifest, original.result.manifest);
    let bytes = local
        .owner_managed_result_chunk(f.order(), &run.id, &version.id, chunk(&original.result))
        .await
        .unwrap();
    assert_eq!(bytes.bytes, b"native result");
    assert_eq!(bytes.metadata.sha256, hash(b"native result"));
    assert_eq!(bytes.metadata.size, 13);
    assert_eq!(
        local
            .receive_owner_managed_result(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .unwrap(),
        ready
    );
    let json = serde_json::to_value(&manifest).unwrap();
    let entry_keys: Vec<_> = json["entry"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        entry_keys,
        ["order_id", "process_attempt_id", "run_id", "version"]
    );
    let version_keys: Vec<_> = json["entry"]["version"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        version_keys,
        [
            "created_at",
            "file_count",
            "id",
            "manifest_sha256",
            "revision",
            "state",
            "total_bytes"
        ]
    );
    assert_eq!(stored(&f).await, original);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    drop(local);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn owner_version_ids_and_hashes_cannot_escape_original_order_or_run() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let id = runtime
        .retain_managed_result_version(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
        .await
        .unwrap();
    let version = stored(&f).await;
    let local = local(&runtime);
    let foreign = uuid::Uuid::new_v4().to_string();
    for (order, run_id, version_id) in [
        (foreign.as_str(), run.id.as_str(), id.as_str()),
        (f.order(), foreign.as_str(), id.as_str()),
        (f.order(), run.id.as_str(), foreign.as_str()),
    ] {
        assert!(
            local
                .owner_managed_result_manifest(order, run_id, version_id)
                .await
                .is_err()
        );
        assert!(
            local
                .owner_managed_result_chunk(order, run_id, version_id, chunk(&version.result))
                .await
                .is_err()
        );
    }
    assert!(
        local
            .owner_managed_results(&foreign, &run.id)
            .await
            .is_err()
    );
    let mut wrong_hash = chunk(&version.result);
    wrong_hash.expected_sha256 = "0".repeat(64);
    assert!(
        local
            .owner_managed_result_chunk(f.order(), &run.id, &id, wrong_hash)
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    drop(local);
    drop(runtime);
    drop(peer);
    f.close().await;
}

async fn assert_denied(denial: &str) {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let id = runtime
        .retain_managed_result_version(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
        .await
        .unwrap();
    let version = stored(&f).await;
    let local = local(&runtime);
    match denial {
        "cancel" => {
            runtime.store.cancel(&run.id).await.unwrap();
        }
        "source" => {
            let RunRequest::Single(request) = &f.input.request else {
                unreachable!()
            };
            let grant = &request.folders[0];
            runtime
                .files
                .as_ref()
                .unwrap()
                .revoke_native_folder(&grant.project_id, &grant.id, &grant.revision)
                .unwrap();
        }
        _ => {
            sqlx::query("UPDATE broker_controller SET generation=generation+1 WHERE singleton=1")
                .execute(&f.pool)
                .await
                .unwrap();
        }
    }
    assert!(
        local
            .owner_managed_results(f.order(), &run.id)
            .await
            .is_err(),
        "{denial}"
    );
    assert!(
        local
            .owner_managed_result_manifest(f.order(), &run.id, &id)
            .await
            .is_err(),
        "{denial}"
    );
    assert!(
        local
            .owner_managed_result_chunk(f.order(), &run.id, &id, chunk(&version.result))
            .await
            .is_err(),
        "{denial}"
    );
    assert_eq!(stored(&f).await, version);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    drop(local);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn owner_local_views_keep_cancellation_gate() {
    Box::pin(assert_denied("cancel")).await;
}
#[tokio::test]
async fn owner_local_views_keep_source_gate() {
    Box::pin(assert_denied("source")).await;
}
#[tokio::test]
async fn owner_local_views_keep_controller_gate() {
    Box::pin(assert_denied("controller")).await;
}

#[tokio::test]
async fn owner_corrupt_ready_bytes_never_fall_back_to_remote_or_rewrite_state() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let id = runtime
        .retain_managed_result_version(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
        .await
        .unwrap();
    let version = stored(&f).await;
    std::fs::write(
        f.dir
            .path()
            .join("broker/managed-results")
            .join(version.custody.as_ref().unwrap().id())
            .join("transfer/payload.pack"),
        b"bad",
    )
    .unwrap();
    assert!(
        runtime
            .owner_managed_result_manifest(f.order(), &run.id, &id)
            .await
            .is_err()
    );
    assert!(
        runtime
            .owner_managed_result_chunk(f.order(), &run.id, &id, chunk(&version.result))
            .await
            .is_err()
    );
    assert!(
        runtime
            .receive_owner_managed_result(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .is_err()
    );
    assert_eq!(stored(&f).await, version);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn owner_list_limit_fails_explicitly_before_returning_any_partial_list() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    // Index tampering supplies 129 candidates to exercise the response bound;
    // no fabricated receipt is accepted as a candidate or read authority.
    let mut tx = f.pool.begin().await.unwrap();
    for _ in 0..128 {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO process_attempts(id,run_id,tool_call_id,revision,payload) SELECT $1,run_id,$1,revision,payload FROM process_attempts WHERE id=$2")
            .bind(&id).bind(&peer.launch.payload.process_attempt_id).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO owner_managed_input_transfers(attempt_id,run_id,plan_id,grant_sha256,custody_sha256,payload) SELECT $1,run_id,plan_id,grant_sha256,custody_sha256,payload FROM owner_managed_input_transfers WHERE attempt_id=$2")
            .bind(&id).bind(&peer.launch.payload.process_attempt_id).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO owner_managed_completion_receipts(attempt_id,run_id,plan_id,receipt_sha256,payload) SELECT $1,run_id,plan_id,receipt_sha256,payload FROM owner_managed_completion_receipts WHERE attempt_id=$2")
            .bind(&id).bind(&peer.launch.payload.process_attempt_id).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let error = local(&runtime)
        .owner_managed_results(f.order(), &run.id)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Validation(ref value) if value == "managed_result_list_limit_exceeded")
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 0);
    assert!(!f.dir.path().join("broker/managed-results").exists());
    drop(runtime);
    drop(peer);
    f.close().await;
}
