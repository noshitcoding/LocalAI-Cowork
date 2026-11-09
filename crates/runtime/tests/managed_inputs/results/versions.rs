//! Actual SQL/native private reception with a synthetic signed output peer.
use super::*;
use cowork_broker::ManagedResultStage;
use cowork_storage::{ManagedResultContext, ManagedResultVersion, ManagedResultVersionStatus};
#[path = "local_ready.rs"]
mod local_ready;
#[path = "owner.rs"]
mod owner;

async fn stored(f: &Fixture) -> ManagedResultVersion {
    let payload: String = sqlx::query_scalar("SELECT payload FROM owner_managed_result_versions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    serde_json::from_str(&payload).unwrap()
}
fn origin(f: &Fixture) -> String {
    f.runtime
        .files
        .as_ref()
        .unwrap()
        .local_controller_origin()
        .unwrap()
}
async fn context(f: &Fixture, run: &RunRecord, peer: &ResultRunner) -> ManagedResultContext {
    f.runtime
        .store
        .managed_result_context(
            f.order(),
            &run.id,
            &peer.launch.payload.process_attempt_id,
            &peer.inner.observation(),
            &origin(f),
        )
        .await
        .unwrap()
}
async fn prepare_intent(
    f: &Fixture,
    runtime: &Runtime,
    run: &RunRecord,
    peer: &ResultRunner,
) -> ManagedResultVersion {
    let result = runtime
        .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
        .await
        .unwrap();
    let current = context(f, run, peer).await;
    f.runtime
        .store
        .prepare_managed_result_version(
            f.order(),
            &run.id,
            &peer.launch.payload.process_attempt_id,
            &result,
            &result.attestation.payload.runtime_instance_sha256,
            &peer.inner.observation(),
            &origin(f),
            &current.authorization_sha256,
        )
        .await
        .unwrap()
}
async fn attach(
    f: &Fixture,
    run: &RunRecord,
    peer: &ResultRunner,
    version: &ManagedResultVersion,
    stage: &ManagedResultStage,
) -> ManagedResultVersion {
    let current = context(f, run, peer).await;
    f.runtime
        .store
        .attach_managed_result_custody(
            &version.id,
            stage.record(),
            f.order(),
            &run.id,
            &peer.launch.payload.process_attempt_id,
            &peer.inner.observation(),
            &origin(f),
            &current.authorization_sha256,
        )
        .await
        .unwrap()
}
fn create(f: &Fixture, version: &ManagedResultVersion) -> Arc<ManagedResultStage> {
    Arc::new(
        f.runtime
            .files
            .as_ref()
            .unwrap()
            .create_managed_result_stage(version.stage_binding(), version.result.manifest.clone())
            .unwrap(),
    )
}

#[tokio::test]
async fn verified_version_replays_original_id_and_reads_after_native_broker_reopen() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let attempt = &peer.launch.payload.process_attempt_id;
    let id = runtime
        .retain_managed_result_version(f.order(), &run.id, attempt)
        .await
        .unwrap();
    let version = stored(&f).await;
    assert_eq!(version.state, ManagedResultVersionStatus::Ready);
    assert_eq!(version.id, id);
    assert!(version.custody.is_some());
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    let mut reopened = runtime.clone();
    reopened.files = Some(Arc::new(
        FileBroker::open(
            &f.dir.path().join("broker"),
            vec![FolderConfig {
                id: "source".into(),
                project_id: run.spec.project_id.clone(),
                name: "Source".into(),
                path: f.dir.path().join("originals"),
                writable: true,
            }],
        )
        .unwrap(),
    ));
    assert_eq!(
        reopened
            .retain_managed_result_version(f.order(), &run.id, attempt)
            .await
            .unwrap(),
        id
    );
    assert_eq!(
        reopened
            .managed_result_version_chunk(f.order(), &run.id, attempt, &id, chunk(&version.result))
            .await
            .unwrap(),
        b"native result"
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    assert_eq!(stored(&f).await.revision, version.revision);
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    assert_eq!(
        f.runtime
            .store
            .process_attempts(&run.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        std::fs::read(f.dir.path().join("originals/value.txt")).unwrap(),
        b"original"
    );
    assert_eq!(
        runtime
            .run_copy_text(
                f.order(),
                &run.id,
                FileLocation {
                    folder_id: "source".into(),
                    path: "value.txt".into()
                }
            )
            .await
            .unwrap()
            .content,
        "derived copy"
    );
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    drop(reopened);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn broker_reopen_never_recreates_or_accepts_replaced_original_source_workspace() {
    // One controller/native origin per database, including PostgreSQL. Probe
    // both source failures against that same independently stored identity.
    let (mut f, mut runtime, peer, run, _) = prepared(true).await;
    let attempt = &peer.launch.payload.process_attempt_id;
    let id = runtime
        .retain_managed_result_version(f.order(), &run.id, attempt)
        .await
        .unwrap();
    let version = stored(&f).await;
    let workspace = f.dir.path().join("broker/workspaces").join(f.order());
    let moved = f.dir.path().join("original-source-workspace");
    runtime.files = None;
    f.runtime.files = None;
    std::fs::rename(&workspace, &moved).unwrap();
    for replace in [false, true] {
        if replace {
            std::fs::create_dir(&workspace).unwrap();
            std::fs::create_dir(workspace.join("files")).unwrap();
            std::fs::copy(
                moved.join("workspace.json"),
                workspace.join("workspace.json"),
            )
            .unwrap();
        }
        let mut reopened = runtime.clone();
        reopened.files = Some(Arc::new(
            FileBroker::open(
                &f.dir.path().join("broker"),
                vec![FolderConfig {
                    id: "source".into(),
                    project_id: run.spec.project_id.clone(),
                    name: "Source".into(),
                    path: f.dir.path().join("originals"),
                    writable: true,
                }],
            )
            .unwrap(),
        ));
        assert!(
            reopened
                .retain_managed_result_version(f.order(), &run.id, attempt)
                .await
                .is_err()
        );
        assert!(
            reopened
                .managed_result_version_chunk(
                    f.order(),
                    &run.id,
                    attempt,
                    &id,
                    chunk(&version.result)
                )
                .await
                .is_err()
        );
        assert_eq!(
            workspace.exists(),
            replace,
            "missing source must remain missing"
        );
        if replace {
            assert_eq!(
                std::fs::read_dir(workspace.join("files")).unwrap().count(),
                0
            );
        }
        assert_eq!(stored(&f).await, version);
        assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
        assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
        drop(reopened);
    }
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn lost_native_create_reply_uses_exact_original_preparation_and_sql_intent() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let intent = prepare_intent(&f, &runtime, &run, &peer).await;
    let stage = create(&f, &intent);
    let original = stage.record().clone();
    drop(stage);
    assert!(stored(&f).await.custody.is_none());
    assert_eq!(
        runtime
            .retain_managed_result_version(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .unwrap(),
        intent.id
    );
    assert_eq!(stored(&f).await.custody, Some(original));
    assert_eq!(
        std::fs::read_dir(f.dir.path().join("broker/managed-results"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn lost_attach_reply_resumes_only_independently_stored_native_custody() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let intent = prepare_intent(&f, &runtime, &run, &peer).await;
    let stage = create(&f, &intent);
    let original = attach(&f, &run, &peer, &intent, &stage).await;
    drop(stage);
    assert_eq!(
        runtime
            .retain_managed_result_version(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .unwrap(),
        intent.id
    );
    assert_eq!(stored(&f).await.custody, original.custody);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn cancellation_during_chunk_read_keeps_partial_result_unpublished() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    peer.mode.store(6, Ordering::SeqCst);
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
    let version = stored(&f).await;
    assert_eq!(version.state, ManagedResultVersionStatus::Receiving);
    let stage = f
        .runtime
        .files
        .as_ref()
        .unwrap()
        .reopen_managed_result_stage(version.custody.as_ref().unwrap())
        .unwrap();
    assert_eq!(stage.missing_chunks().unwrap().len(), 1);
    assert!(stage.verify().is_err());
    assert!(
        runtime
            .managed_result_version_chunk(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id,
                &version.id,
                chunk(&version.result)
            )
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    drop(stage);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn resource_revocation_blocks_receiving_resume_without_any_chunk_read() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let intent = prepare_intent(&f, &runtime, &run, &peer).await;
    let stage = create(&f, &intent);
    attach(&f, &run, &peer, &intent, &stage).await;
    drop(stage);
    let RunRequest::Single(request) = &f.input.request else {
        unreachable!()
    };
    let grant = &request.folders[0];
    f.runtime
        .files
        .as_ref()
        .unwrap()
        .revoke_native_folder(&grant.project_id, &grant.id, &grant.revision)
        .unwrap();
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
    assert_eq!(
        stored(&f).await.state,
        ManagedResultVersionStatus::Receiving
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 1);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn bad_peer_content_requires_explicit_retry_of_same_incomplete_version() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    peer.mode.store(7, Ordering::SeqCst);
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
    let original = stored(&f).await;
    assert_eq!(original.state, ManagedResultVersionStatus::Receiving);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    peer.mode.store(0, Ordering::SeqCst);
    assert_eq!(
        runtime
            .retain_managed_result_version(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .unwrap(),
        original.id
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 3);
    assert_eq!(stored(&f).await.custody, original.custody);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn corrupt_published_pack_is_not_repaired_from_remote_or_originals() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let attempt = &peer.launch.payload.process_attempt_id;
    let id = runtime
        .retain_managed_result_version(f.order(), &run.id, attempt)
        .await
        .unwrap();
    let version = stored(&f).await;
    let path = f
        .dir
        .path()
        .join("broker/managed-results")
        .join(version.custody.as_ref().unwrap().id())
        .join("transfer/payload.pack");
    std::fs::write(path, b"corrupt bytes").unwrap();
    assert!(
        runtime
            .managed_result_version_chunk(f.order(), &run.id, attempt, &id, chunk(&version.result))
            .await
            .is_err()
    );
    assert!(
        runtime
            .retain_managed_result_version(f.order(), &run.id, attempt)
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    assert_eq!(stored(&f).await.revision, version.revision);
    assert_eq!(
        std::fs::read(f.dir.path().join("originals/value.txt")).unwrap(),
        b"original"
    );
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn another_verified_receiver_cannot_publish_the_original_version() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let intent = prepare_intent(&f, &runtime, &run, &peer).await;
    let original = create(&f, &intent);
    let attached = attach(&f, &run, &peer, &intent, &original).await;
    let mut wrong = intent.stage_binding();
    wrong.attestation_sha256 = hash(b"different-attestation");
    let foreign = Arc::new(
        f.runtime
            .files
            .as_ref()
            .unwrap()
            .create_managed_result_stage(wrong, intent.result.manifest.clone())
            .unwrap(),
    );
    foreign
        .accept_chunk(&chunk(&intent.result), b"native result")
        .unwrap();
    let verified = foreign.clone().verify_owned().unwrap();
    let current = context(&f, &run, &peer).await;
    assert!(
        f.runtime
            .store
            .publish_managed_result_version(
                &intent.id,
                &verified,
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id,
                &peer.inner.observation(),
                &origin(&f),
                &current.authorization_sha256
            )
            .await
            .is_err()
    );
    assert_eq!(stored(&f).await, attached);
    drop(verified);
    drop(foreign);
    drop(original);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn cancel_after_full_native_verification_still_denies_sql_publication() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let intent = prepare_intent(&f, &runtime, &run, &peer).await;
    let stage = create(&f, &intent);
    let attached = attach(&f, &run, &peer, &intent, &stage).await;
    stage
        .accept_chunk(&chunk(&intent.result), b"native result")
        .unwrap();
    let verified = stage.clone().verify_owned().unwrap();
    let current = context(&f, &run, &peer).await;
    f.runtime.store.cancel(&run.id).await.unwrap();
    assert!(
        f.runtime
            .store
            .publish_managed_result_version(
                &intent.id,
                &verified,
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id,
                &peer.inner.observation(),
                &origin(&f),
                &current.authorization_sha256
            )
            .await
            .is_err()
    );
    assert_eq!(stored(&f).await, attached);
    drop(verified);
    drop(stage);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn partial_native_preparation_is_not_recreated_or_attached_on_resume() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let intent = prepare_intent(&f, &runtime, &run, &peer).await;
    let stage = create(&f, &intent);
    let record = stage.record().clone();
    drop(stage);
    let path = f
        .dir
        .path()
        .join("broker/managed-results")
        .join(record.id())
        .join("record.json");
    assert!(path.is_absolute() && path.starts_with(f.dir.path()));
    std::fs::remove_file(&path).unwrap();
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
    assert!(!path.exists());
    assert_eq!(stored(&f).await, intent);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 1);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn corrupted_version_index_is_not_custody_or_remote_read_authority() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let intent = prepare_intent(&f, &runtime, &run, &peer).await;
    sqlx::query("UPDATE owner_managed_result_versions SET state='ready' WHERE id=$1")
        .bind(&intent.id)
        .execute(&f.pool)
        .await
        .unwrap();
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
    assert_eq!(peer.reads.load(Ordering::SeqCst), 1);
    assert!(!f.dir.path().join("broker/managed-results").exists());
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn acquired_local_version_survives_remote_retention_without_a_new_read_lease() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    peer.mode.store(9, Ordering::SeqCst);
    let attempt = &peer.launch.payload.process_attempt_id;
    let id = runtime
        .retain_managed_result_version(f.order(), &run.id, attempt)
        .await
        .unwrap();
    let version = stored(&f).await;
    let wait = version.result.attestation.payload.retained_until - now() + 150;
    // PostgreSQL acquisition must fit inside the original signed retention;
    // wait for that actual expiry rather than assuming a subsecond transfer.
    assert!(wait < 6000);
    if wait > 0 {
        tokio::time::sleep(Duration::from_millis(wait as u64)).await;
    }
    assert!(now() >= version.result.attestation.payload.retained_until);
    assert_eq!(
        runtime
            .managed_result_version_chunk(f.order(), &run.id, attempt, &id, chunk(&version.result))
            .await
            .unwrap(),
        b"native result"
    );
    assert_eq!(
        runtime
            .retain_managed_result_version(f.order(), &run.id, attempt)
            .await
            .unwrap(),
        id
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    assert_eq!(stored(&f).await.revision, version.revision);
    drop(runtime);
    drop(peer);
    f.close().await;
}
