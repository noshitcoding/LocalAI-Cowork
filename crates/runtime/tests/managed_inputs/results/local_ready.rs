//! Local Ready authority remains independent of remote execution readiness.
use super::*;

fn local_runtime(runtime: &Runtime) -> Runtime {
    let mut local = Runtime::new(runtime.store.clone(), vec![], true).unwrap();
    local.files = runtime.files.clone();
    local
}

struct OfflineRunner(AtomicUsize);
#[async_trait::async_trait]
impl ProcessRunnerAccess for OfflineRunner {
    async fn observe(
        &self,
        _: &ExecutionEnvironmentRef,
    ) -> cowork_domain::Result<Signed<EnvironmentObservation>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(Error::Forbidden)
    }
    async fn fence(
        &self,
        _: Signed<cowork_runner_protocol::FenceRequest>,
    ) -> cowork_domain::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(Error::Forbidden)
    }
    async fn renew_execution_lease(
        &self,
        _: Signed<cowork_runner_protocol::ExecutionLeaseRenewal>,
    ) -> cowork_domain::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(Error::Forbidden)
    }
    async fn launch(
        &self,
        _: Signed<LaunchRequest>,
        _: CancellationToken,
    ) -> cowork_domain::Result<Signed<Receipt>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(Error::Forbidden)
    }
}

#[tokio::test]
async fn ready_reads_and_exact_replay_need_no_configured_runner() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let attempt = &peer.launch.payload.process_attempt_id;
    let id = runtime
        .retain_managed_result_version(f.order(), &run.id, attempt)
        .await
        .unwrap();
    let version = stored(&f).await;
    let local = local_runtime(&runtime);
    assert_eq!(
        local
            .managed_result_version_chunk(f.order(), &run.id, attempt, &id, chunk(&version.result),)
            .await
            .unwrap(),
        b"native result"
    );
    assert_eq!(
        local
            .retain_managed_result_version(f.order(), &run.id, attempt)
            .await
            .unwrap(),
        id
    );
    // A valid local version is not an unscoped content/hash credential.
    assert!(
        local
            .managed_result_version_chunk(
                &uuid::Uuid::new_v4().to_string(),
                &run.id,
                attempt,
                &id,
                chunk(&version.result),
            )
            .await
            .is_err()
    );
    assert!(
        local
            .managed_result_version_chunk(
                f.order(),
                &run.id,
                attempt,
                &uuid::Uuid::new_v4().to_string(),
                chunk(&version.result),
            )
            .await
            .is_err()
    );
    assert_eq!(stored(&f).await, version);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        runtime.store.process_attempts(&run.id).await.unwrap().len(),
        1
    );
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    drop(local);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn ready_survives_unreachable_runner_and_revoked_execution_target_and_environment() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let attempt = &peer.launch.payload.process_attempt_id;
    let id = runtime
        .retain_managed_result_version(f.order(), &run.id, attempt)
        .await
        .unwrap();
    let version = stored(&f).await;
    let unavailable = Arc::new(OfflineRunner(AtomicUsize::new(0)));
    let offline = local_runtime(&runtime)
        .with_registered_process_runner(
            ProcessRunnerConfig::new(f.binding.clone(), f.control.clone(), unavailable.clone())
                .unwrap(),
        )
        .await
        .unwrap();
    runtime
        .store
        .set_execution_target_active(&f.identity, 2, false)
        .await
        .unwrap();
    let reference = run.spec.environment.as_ref().unwrap();
    let environment = runtime
        .store
        .environment(&run.spec.project_id, &reference.environment_id)
        .await
        .unwrap();
    runtime
        .store
        .change_environment(
            &run.spec.project_id,
            &reference.environment_id,
            environment.revision,
            "retained-local-result",
            &EnvironmentChange::RevokeVersion {
                version_id: reference.version_id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        offline
            .managed_result_version_chunk(f.order(), &run.id, attempt, &id, chunk(&version.result),)
            .await
            .unwrap(),
        b"native result"
    );
    assert_eq!(
        offline
            .retain_managed_result_version(f.order(), &run.id, attempt)
            .await
            .unwrap(),
        id
    );
    assert_eq!(unavailable.0.load(Ordering::SeqCst), 0);
    assert!(
        runtime
            .managed_result_manifest(f.order(), &run.id, attempt)
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    assert_eq!(stored(&f).await, version);
    drop(offline);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn canceled_order_still_denies_ready_local_reads_and_replay() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let attempt = &peer.launch.payload.process_attempt_id;
    let id = runtime
        .retain_managed_result_version(f.order(), &run.id, attempt)
        .await
        .unwrap();
    let version = stored(&f).await;
    let local = local_runtime(&runtime);
    runtime.store.cancel(&run.id).await.unwrap();
    assert!(
        local
            .managed_result_version_chunk(f.order(), &run.id, attempt, &id, chunk(&version.result),)
            .await
            .is_err()
    );
    assert!(
        local
            .retain_managed_result_version(f.order(), &run.id, attempt)
            .await
            .is_err()
    );
    assert_eq!(stored(&f).await, version);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    drop(local);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn revoked_source_still_denies_ready_local_reads_and_replay() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let attempt = &peer.launch.payload.process_attempt_id;
    let id = runtime
        .retain_managed_result_version(f.order(), &run.id, attempt)
        .await
        .unwrap();
    let version = stored(&f).await;
    let local = local_runtime(&runtime);
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
    assert!(
        local
            .managed_result_version_chunk(f.order(), &run.id, attempt, &id, chunk(&version.result),)
            .await
            .is_err()
    );
    assert!(
        local
            .retain_managed_result_version(f.order(), &run.id, attempt)
            .await
            .is_err()
    );
    assert_eq!(stored(&f).await, version);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    drop(local);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn changed_local_controller_still_denies_ready_local_reads_and_replay() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let attempt = &peer.launch.payload.process_attempt_id;
    let id = runtime
        .retain_managed_result_version(f.order(), &run.id, attempt)
        .await
        .unwrap();
    let version = stored(&f).await;
    let local = local_runtime(&runtime);
    sqlx::query("UPDATE broker_controller SET generation=generation+1 WHERE singleton=1")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        local
            .managed_result_version_chunk(f.order(), &run.id, attempt, &id, chunk(&version.result),)
            .await
            .is_err()
    );
    assert!(
        local
            .retain_managed_result_version(f.order(), &run.id, attempt)
            .await
            .is_err()
    );
    assert_eq!(stored(&f).await, version);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    drop(local);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn receiving_cannot_use_local_ready_authority_or_bypass_fresh_store_gates() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let attempt = &peer.launch.payload.process_attempt_id;
    let intent = prepare_intent(&f, &runtime, &run, &peer).await;
    let stage = create(&f, &intent);
    let version = attach(&f, &run, &peer, &intent, &stage).await;
    stage
        .accept_chunk(&chunk(&version.result), b"native result")
        .unwrap();
    let verified = stage.clone().verify_owned().unwrap();
    let current = context(&f, &run, &peer).await;
    let local = local_runtime(&runtime);
    assert!(
        local
            .managed_result_version_chunk(
                f.order(),
                &run.id,
                attempt,
                &version.id,
                chunk(&version.result),
            )
            .await
            .is_err()
    );
    assert!(
        local
            .retain_managed_result_version(f.order(), &run.id, attempt)
            .await
            .is_err()
    );
    runtime
        .store
        .set_execution_target_active(&f.identity, 2, false)
        .await
        .unwrap();
    let observation = peer.inner.observation();
    let origin = origin(&f);
    assert!(
        runtime
            .store
            .prepare_managed_result_version(
                f.order(),
                &run.id,
                attempt,
                &version.result,
                &version.result.attestation.payload.runtime_instance_sha256,
                &observation,
                &origin,
                &current.authorization_sha256,
            )
            .await
            .is_err()
    );
    assert!(
        runtime
            .store
            .attach_managed_result_custody(
                &version.id,
                stage.record(),
                f.order(),
                &run.id,
                attempt,
                &observation,
                &origin,
                &current.authorization_sha256,
            )
            .await
            .is_err()
    );
    assert!(
        runtime
            .store
            .publish_managed_result_version(
                &version.id,
                &verified,
                f.order(),
                &run.id,
                attempt,
                &observation,
                &origin,
                &current.authorization_sha256,
            )
            .await
            .is_err()
    );
    assert_eq!(stored(&f).await, version);
    assert_eq!(peer.reads.load(Ordering::SeqCst), 1);
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    drop(verified);
    drop(stage);
    drop(local);
    drop(runtime);
    drop(peer);
    f.close().await;
}
