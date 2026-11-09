//! Runtime transport boundary, with real Store provenance and synthetic keys.
use super::*;

struct HistoryPeer {
    inner: Arc<Runner>,
    control: SigningKey,
    signed: Signed<Receipt>,
    mode: AtomicUsize,
    gets: AtomicUsize,
    other_calls: AtomicUsize,
    marker: std::path::PathBuf,
}

#[async_trait::async_trait]
impl ProcessRunnerAccess for HistoryPeer {
    async fn receipt(&self, attempt: &str) -> cowork_domain::Result<Signed<Receipt>> {
        assert_eq!(attempt, self.signed.payload.request.process_attempt_id);
        self.gets.fetch_add(1, Ordering::SeqCst);
        match self.mode.load(Ordering::SeqCst) {
            1 => Err(Error::UncertainEffect),
            2 => {
                let mut body = self.signed.payload.clone();
                body.state = ExecutionState::Admitted;
                body.revision = 1;
                self.inner
                    .key
                    .sign("receipt", body)
                    .map_err(|_| Error::Forbidden)
            }
            3 => self
                .control
                .sign("receipt", self.signed.payload.clone())
                .map_err(|_| Error::Forbidden),
            4 => {
                // Actual original native marker changes during the awaited GET.
                std::fs::write(
                    &self.marker,
                    serde_json::to_vec(&uuid::Uuid::new_v4().to_string()).unwrap(),
                )
                .map_err(|e| Error::Storage(Box::new(e)))?;
                Ok(self.signed.clone())
            }
            _ => Ok(self.signed.clone()),
        }
    }
    async fn observe(
        &self,
        _: &ExecutionEnvironmentRef,
    ) -> cowork_domain::Result<Signed<EnvironmentObservation>> {
        self.other_calls.fetch_add(1, Ordering::SeqCst);
        Err(Error::Forbidden)
    }
    async fn fence(
        &self,
        _: Signed<cowork_runner_protocol::FenceRequest>,
    ) -> cowork_domain::Result<()> {
        self.other_calls.fetch_add(1, Ordering::SeqCst);
        Err(Error::Forbidden)
    }
    async fn renew_execution_lease(
        &self,
        _: Signed<cowork_runner_protocol::ExecutionLeaseRenewal>,
    ) -> cowork_domain::Result<()> {
        self.other_calls.fetch_add(1, Ordering::SeqCst);
        Err(Error::Forbidden)
    }
    async fn launch(
        &self,
        _: Signed<LaunchRequest>,
        _: CancellationToken,
    ) -> cowork_domain::Result<Signed<Receipt>> {
        self.other_calls.fetch_add(1, Ordering::SeqCst);
        Err(Error::Forbidden)
    }
}

fn local(f: &Fixture) -> Runtime {
    let mut runtime = Runtime::new(f.runtime.store.clone(), vec![], true).unwrap();
    runtime.files = f.runtime.files.clone();
    runtime
}
async fn transport(
    f: &Fixture,
    request: &Signed<LaunchRequest>,
    mode: usize,
) -> (Runtime, Arc<HistoryPeer>) {
    let peer = Arc::new(HistoryPeer {
        inner: f.runner.clone(),
        control: f.control.clone(),
        signed: completion(f, request),
        mode: AtomicUsize::new(mode),
        gets: AtomicUsize::new(0),
        other_calls: AtomicUsize::new(0),
        marker: f.dir.path().join("broker/broker-controller-origin.json"),
    });
    let config =
        ProcessRunnerConfig::new(f.binding.clone(), f.control.clone(), peer.clone()).unwrap();
    (
        local(f)
            .with_configured_process_runner(config)
            .await
            .unwrap(),
        peer,
    )
}

fn export_fixture(
    intent: &ReconcileOwnerManagedProcess,
    response: &OwnerManagedProcessReconciliation,
) {
    let Some(path) = std::env::var_os("COWORK_OWNER_RECONCILE_FIXTURE_PATH") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    assert!(
        path.is_absolute(),
        "fixture export must be an absolute new path"
    );
    let value = json!({"provenance":"synthetic controller/Store test, no physical qualification","intent":intent,"response":response});
    // Reparse the actual typed serialization before exporting public DTOs only.
    let _: ReconcileOwnerManagedProcess = serde_json::from_value(value["intent"].clone()).unwrap();
    let _: OwnerManagedProcessReconciliation =
        serde_json::from_value(value["response"].clone()).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    use std::io::Write;
    file.write_all(&serde_json::to_vec_pretty(&value).unwrap())
        .unwrap();
    file.sync_all().unwrap();
}

#[tokio::test]
async fn retained_receipt_and_exact_runtime_replay_need_no_runner_or_new_authority() {
    let (f, run, token, request, origin) = prepare().await;
    let intent = interrupt(&f, &run.id, &token).await;
    let signed = completion(&f, &request);
    f.runtime
        .store
        .retain_managed_completion_receipt(&signed)
        .await
        .unwrap();
    f.runtime
        .store
        .set_execution_target_active(&f.identity, 2, false)
        .await
        .unwrap();
    let runtime = local(&f);
    let answer = runtime
        .reconcile_owner_managed_process(
            f.order(),
            &run.id,
            &request.payload.process_attempt_id,
            intent.clone(),
        )
        .await
        .unwrap();
    assert_eq!(answer.run.state, RunState::Canceled);
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    assert_eq!(
        canonical_hash(
            &runtime
                .reconcile_owner_managed_process(
                    f.order(),
                    &run.id,
                    &request.payload.process_attempt_id,
                    intent.clone()
                )
                .await
                .unwrap()
        ),
        canonical_hash(&answer)
    );
    assert_eq!(
        canonical_hash(&before),
        canonical_hash(&f.runtime.store.stored_run(&run.id).await.unwrap())
    );
    assert_eq!(
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
            .receipt,
        Some(signed)
    );
    export_fixture(&intent, &answer);
    drop(runtime);
    f.close().await;
}

#[tokio::test]
async fn restarted_runtime_observes_original_once_even_after_target_and_environment_revocation() {
    let (f, run, token, request, _) = prepare().await;
    let intent = interrupt(&f, &run.id, &token).await;
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
            "history-runtime-revoke",
            &EnvironmentChange::RevokeVersion {
                version_id: reference.version_id.clone(),
            },
        )
        .await
        .unwrap();
    let (runtime, peer) = transport(&f, &request, 0).await;
    let answer = runtime
        .reconcile_owner_managed_process(
            f.order(),
            &run.id,
            &request.payload.process_attempt_id,
            intent.clone(),
        )
        .await
        .unwrap();
    assert_eq!(answer.run.state, RunState::Canceled);
    assert_eq!(
        canonical_hash(
            &runtime
                .reconcile_owner_managed_process(
                    f.order(),
                    &run.id,
                    &request.payload.process_attempt_id,
                    intent
                )
                .await
                .unwrap()
        ),
        canonical_hash(&answer)
    );
    assert_eq!(peer.gets.load(Ordering::SeqCst), 1);
    assert_eq!(peer.other_calls.load(Ordering::SeqCst), 0);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn active_stale_or_foreign_owner_intents_and_foreign_runner_identity_issue_no_receipt_get() {
    let (f, run, token, request, _) = prepare().await;
    let (runtime, peer) = transport(&f, &request, 0).await;
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    let active = ReconcileOwnerManagedProcess {
        expected_run_revision: before.record.revision,
        expected_request_sha256: request.payload.request_sha256.clone(),
        finish_cancellation: true,
    };
    assert!(
        runtime
            .reconcile_owner_managed_process(
                f.order(),
                &run.id,
                &request.payload.process_attempt_id,
                active
            )
            .await
            .is_err()
    );
    let intent = interrupt(&f, &run.id, &token).await;
    let mut stale = intent.clone();
    stale.expected_run_revision -= 1;
    assert!(
        runtime
            .reconcile_owner_managed_process(
                f.order(),
                &run.id,
                &request.payload.process_attempt_id,
                stale
            )
            .await
            .is_err()
    );
    for (order, run_id, attempt) in [
        (
            "foreign",
            run.id.as_str(),
            request.payload.process_attempt_id.as_str(),
        ),
        (
            f.order(),
            "foreign",
            request.payload.process_attempt_id.as_str(),
        ),
        (f.order(), run.id.as_str(), "foreign"),
    ] {
        assert!(
            runtime
                .reconcile_owner_managed_process(order, run_id, attempt, intent.clone())
                .await
                .is_err()
        );
    }
    let mut binding = f.binding.clone();
    binding.generation += 1;
    let foreign = local(&f)
        .with_configured_process_runner(
            ProcessRunnerConfig::new(binding, f.control.clone(), peer.clone()).unwrap(),
        )
        .await
        .unwrap();
    assert!(
        foreign
            .reconcile_owner_managed_process(
                f.order(),
                &run.id,
                &request.payload.process_attempt_id,
                intent
            )
            .await
            .is_err()
    );
    assert_eq!(peer.gets.load(Ordering::SeqCst), 0);
    assert_eq!(peer.other_calls.load(Ordering::SeqCst), 0);
    drop(foreign);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn pending_or_invalid_signed_observation_never_mutates_history_or_checkpoint() {
    let (f, run, token, request, _) = prepare().await;
    let intent = interrupt(&f, &run.id, &token).await;
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    let (runtime, peer) = transport(&f, &request, 2).await;
    assert!(
        runtime
            .reconcile_owner_managed_process(
                f.order(),
                &run.id,
                &request.payload.process_attempt_id,
                intent.clone()
            )
            .await
            .is_err()
    );
    assert_eq!(peer.gets.load(Ordering::SeqCst), 1);
    peer.mode.store(3, Ordering::SeqCst);
    assert!(
        runtime
            .reconcile_owner_managed_process(
                f.order(),
                &run.id,
                &request.payload.process_attempt_id,
                intent
            )
            .await
            .is_err()
    );
    assert_eq!(peer.gets.load(Ordering::SeqCst), 2);
    assert_eq!(peer.other_calls.load(Ordering::SeqCst), 0);
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
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn lost_original_observation_is_not_automatically_retried_or_settled() {
    let (f, run, token, request, _) = prepare().await;
    let intent = interrupt(&f, &run.id, &token).await;
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    let (runtime, peer) = transport(&f, &request, 1).await;
    assert!(matches!(
        runtime
            .reconcile_owner_managed_process(
                f.order(),
                &run.id,
                &request.payload.process_attempt_id,
                intent
            )
            .await,
        Err(Error::UncertainEffect)
    ));
    assert_eq!(peer.gets.load(Ordering::SeqCst), 1);
    assert_eq!(peer.other_calls.load(Ordering::SeqCst), 0);
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
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn original_native_origin_changed_during_get_never_settles_the_old_controller() {
    let (f, run, token, request, origin) = prepare().await;
    let intent = interrupt(&f, &run.id, &token).await;
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    let (runtime, peer) = transport(&f, &request, 4).await;
    assert!(
        runtime
            .reconcile_owner_managed_process(
                f.order(),
                &run.id,
                &request.payload.process_attempt_id,
                intent
            )
            .await
            .is_err()
    );
    assert_eq!(peer.gets.load(Ordering::SeqCst), 1);
    assert_eq!(peer.other_calls.load(Ordering::SeqCst), 0);
    assert_ne!(
        f.runtime
            .files
            .as_ref()
            .unwrap()
            .local_controller_origin()
            .unwrap(),
        origin
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
    drop(runtime);
    drop(peer);
    f.close().await;
}
