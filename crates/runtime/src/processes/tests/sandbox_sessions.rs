//! Signed in-process transport verifies application recovery, not VM isolation.
use super::*;

struct LifecycleRunner {
    inner: Arc<FixtureRunner>,
    replies: tokio::sync::Mutex<BTreeMap<String, Signed<SandboxLifecycleReceipt>>>,
    posts: AtomicUsize,
    reads: AtomicUsize,
    lose_reply: AtomicBool,
    reject_next: AtomicBool,
}
#[async_trait::async_trait]
impl ProcessRunnerAccess for LifecycleRunner {
    async fn sandbox_lifecycle(
        &self,
        request: Signed<SandboxLifecycleRequest>,
    ) -> Result<Signed<SandboxLifecycleReceipt>> {
        self.posts.fetch_add(1, Ordering::SeqCst);
        if self.reject_next.swap(false, Ordering::SeqCst) {
            return Err(Error::WorkspaceBusy);
        }
        if request.payload.operation == SandboxLifecycleOperation::Inspect
            && self.replies.lock().await.is_empty()
        {
            return Err(Error::SandboxSessionNotFound);
        }
        let reply = self
            .inner
            .signer
            .sign(
                "sandbox_lifecycle_receipt",
                SandboxLifecycleReceipt {
                    request: request.payload.clone(),
                    sandbox_name: "owned-workspace".into(),
                    sandbox_uid: Some("owned-uid".into()),
                    pod_uid: Some("owned-pod".into()),
                    workspace_pvc: "workspace-pvc".into(),
                    state: match request.payload.operation {
                        SandboxLifecycleOperation::Stop => SandboxLifecycleState::Stopped,
                        SandboxLifecycleOperation::Suspend => SandboxLifecycleState::Suspended,
                        _ => SandboxLifecycleState::Ready,
                    },
                    observed_at: now(),
                },
            )
            .map_err(|_| Error::Forbidden)?;
        self.replies
            .lock()
            .await
            .insert(request.payload.request_id, reply.clone());
        if self.lose_reply.swap(false, Ordering::SeqCst) {
            return Err(Error::UncertainEffect);
        }
        Ok(reply)
    }
    async fn sandbox_lifecycle_receipt(&self, id: &str) -> Result<Signed<SandboxLifecycleReceipt>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.replies
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or(Error::UncertainEffect)
    }
    async fn observe(
        &self,
        reference: &ExecutionEnvironmentRef,
    ) -> Result<Signed<EnvironmentObservation>> {
        self.inner.observe(reference).await
    }
    async fn fence(&self, request: Signed<FenceRequest>) -> Result<()> {
        self.inner.fence(request).await
    }
    async fn renew_execution_lease(&self, request: Signed<ExecutionLeaseRenewal>) -> Result<()> {
        self.inner.renew_execution_lease(request).await
    }
    async fn launch(
        &self,
        _: Signed<LaunchRequest>,
        _: CancellationToken,
    ) -> Result<Signed<Receipt>> {
        panic!("lifecycle must never execute tools")
    }
}

#[tokio::test]
async fn fresh_inspection_proves_absence_without_launch_and_allows_later_ensure() {
    let mut f = setup_artifact(true).await;
    let original = f.runtime.process_runner.clone().unwrap();
    let runner = Arc::new(LifecycleRunner {
        inner: f.runner.clone(),
        replies: Default::default(),
        posts: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
        lose_reply: AtomicBool::new(false),
        reject_next: AtomicBool::new(false),
    });
    f.runtime = f
        .runtime
        .with_registered_process_runner(
            ProcessRunnerConfig::new(
                original.binding.clone(),
                SigningKey::from_pkcs8(&f.control_pkcs8).unwrap(),
                runner.clone(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let order = f.runtime.store.order(&f.input.order_id).await.unwrap();
    let mut request = ManageSandboxSession {
        environment: f.input.environment.clone().unwrap(),
        idempotency_key: "fresh-inspect".into(),
        operation: SandboxLifecycleOperation::Inspect,
        expected_sandbox_uid: None,
    };
    assert!(matches!(
        f.runtime
            .manage_sandbox_session(&order.project_id, &order.id, &request)
            .await,
        Err(Error::SandboxSessionNotFound)
    ));
    assert_eq!(runner.posts.load(Ordering::SeqCst), 0);
    assert!(
        f.runtime
            .store
            .workspace(&order.id)
            .await
            .unwrap()
            .is_none()
    );
    f.runtime.workspace(&order.id).await.unwrap();
    assert!(matches!(
        f.runtime
            .manage_sandbox_session(&order.project_id, &order.id, &request)
            .await,
        Err(Error::SandboxSessionNotFound)
    ));
    assert_eq!(runner.posts.load(Ordering::SeqCst), 1);
    let history = f
        .runtime
        .sandbox_operation_history(&order.project_id, &order.id)
        .await
        .unwrap();
    assert_eq!(history[0].state, "rejected");
    request.idempotency_key = "fresh-ensure".into();
    request.operation = SandboxLifecycleOperation::Ensure;
    assert_eq!(
        f.runtime
            .manage_sandbox_session(&order.project_id, &order.id, &request)
            .await
            .unwrap()
            .state,
        SandboxLifecycleState::Ready
    );
    assert_eq!(runner.posts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn lost_session_ack_reconciles_original_signed_receipt_without_another_post() {
    let mut f = setup_artifact(true).await;
    let original = f.runtime.process_runner.clone().unwrap();
    let runner = Arc::new(LifecycleRunner {
        inner: f.runner.clone(),
        replies: Default::default(),
        posts: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
        lose_reply: AtomicBool::new(true),
        reject_next: AtomicBool::new(false),
    });
    f.runtime = f
        .runtime
        .with_registered_process_runner(
            ProcessRunnerConfig::new(
                original.binding.clone(),
                SigningKey::from_pkcs8(&f.control_pkcs8).unwrap(),
                runner.clone(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let order = f.runtime.store.order(&f.input.order_id).await.unwrap();
    let reference = f.input.environment.clone().unwrap();
    assert!(
        f.runtime
            .require_sandbox_session_for_order(&order.project_id, &order.id, &reference)
            .await
            .is_err()
    );
    let request = ManageSandboxSession {
        environment: reference.clone(),
        idempotency_key: "same-request".into(),
        operation: SandboxLifecycleOperation::Ensure,
        expected_sandbox_uid: None,
    };
    assert!(matches!(
        f.runtime
            .manage_sandbox_session(&order.project_id, &order.id, &request)
            .await,
        Err(Error::UncertainEffect)
    ));
    // A stale/unavailable environment observation cannot prevent receipt reads.
    f.runner.mode.store(2, Ordering::Release);
    let reply = f
        .runtime
        .manage_sandbox_session(&order.project_id, &order.id, &request)
        .await
        .unwrap();
    let again = f
        .runtime
        .manage_sandbox_session(&order.project_id, &order.id, &request)
        .await
        .unwrap();
    assert_eq!(reply, again);
    assert!(reply.workspace_preserved);
    assert_eq!(runner.posts.load(Ordering::SeqCst), 1);
    assert_eq!(runner.reads.load(Ordering::SeqCst), 1);
    f.runner.mode.store(0, Ordering::Release);
    f.runtime
        .require_sandbox_session_for_order(&order.project_id, &order.id, &reference)
        .await
        .unwrap();
    assert!(
        f.runtime
            .manage_sandbox_session("foreign-project", &order.id, &request)
            .await
            .is_err()
    );
    assert_eq!(runner.posts.load(Ordering::SeqCst), 1);
    let history = f
        .runtime
        .sandbox_operation_history(&order.project_id, &order.id)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].request, request);
    assert_eq!(history[0].state, "completed");
}

#[tokio::test]
async fn revoked_environment_and_target_allow_original_recovery_and_cleanup_after_rebind() {
    let mut f = setup_artifact(true).await;
    let original = f.runtime.process_runner.clone().unwrap();
    let runner = Arc::new(LifecycleRunner {
        inner: f.runner.clone(),
        replies: Default::default(),
        posts: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
        lose_reply: AtomicBool::new(true),
        reject_next: AtomicBool::new(false),
    });
    f.runtime = f
        .runtime
        .with_registered_process_runner(
            ProcessRunnerConfig::new(
                original.binding.clone(),
                SigningKey::from_pkcs8(&f.control_pkcs8).unwrap(),
                runner.clone(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let order = f.runtime.store.order(&f.input.order_id).await.unwrap();
    let mut request = ManageSandboxSession {
        environment: f.input.environment.clone().unwrap(),
        idempotency_key: "lost-before-revocation".into(),
        operation: SandboxLifecycleOperation::Ensure,
        expected_sandbox_uid: None,
    };
    assert!(matches!(
        f.runtime
            .manage_sandbox_session(&order.project_id, &order.id, &request)
            .await,
        Err(Error::UncertainEffect)
    ));
    let env = f
        .runtime
        .store
        .environment(&order.project_id, &request.environment.environment_id)
        .await
        .unwrap();
    f.runtime
        .store
        .change_environment(
            &order.project_id,
            &env.id,
            env.revision,
            "revoke-after-admission",
            &EnvironmentChange::Revoke,
        )
        .await
        .unwrap();
    let configured = f.runtime.process_runner.as_ref().unwrap();
    let identity = configured.execution_identity();
    let config = ProcessRunnerConfig::new(
        configured.binding.clone(),
        SigningKey::from_pkcs8(&f.control_pkcs8).unwrap(),
        runner.clone(),
    )
    .unwrap();
    f.runtime
        .store
        .set_execution_target_active(&identity, 2, false)
        .await
        .unwrap();
    f.runtime = f
        .runtime
        .with_configured_process_runner(config)
        .await
        .unwrap();
    assert!(
        f.runtime
            .process_runner
            .as_ref()
            .unwrap()
            .registered_target()
            .is_none()
    );
    let recovered = f
        .runtime
        .manage_sandbox_session(&order.project_id, &order.id, &request)
        .await
        .unwrap();
    assert_eq!(recovered.state, SandboxLifecycleState::Ready);
    assert_eq!(runner.posts.load(Ordering::SeqCst), 1);
    assert_eq!(runner.reads.load(Ordering::SeqCst), 1);
    request.idempotency_key = "new-intent-after-revocation".into();
    assert!(matches!(
        f.runtime
            .manage_sandbox_session(&order.project_id, &order.id, &request)
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(runner.posts.load(Ordering::SeqCst), 1);
    request.idempotency_key = "owner-stop-after-revocation".into();
    request.operation = SandboxLifecycleOperation::Stop;
    request.expected_sandbox_uid = recovered.sandbox_uid;
    let stopped = f
        .runtime
        .manage_sandbox_session(&order.project_id, &order.id, &request)
        .await
        .unwrap();
    assert_eq!(stopped.state, SandboxLifecycleState::Stopped);
    assert_eq!(runner.posts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn expired_never_dispatched_intent_can_release_its_gate_after_revocation() {
    let f = setup_artifact(true).await;
    let order = f.runtime.store.order(&f.input.order_id).await.unwrap();
    let workspace = f.runtime.workspace(&order.id).await.unwrap();
    let request = ManageSandboxSession {
        environment: f.input.environment.clone().unwrap(),
        idempotency_key: "expired-before-revocation".into(),
        operation: SandboxLifecycleOperation::Ensure,
        expected_sandbox_uid: None,
    };
    let runner = f.runtime.process_runner.as_ref().unwrap();
    let at = now() - 60_000;
    let frozen = runner
        .control
        .sign(
            "sandbox_lifecycle",
            SandboxLifecycleRequest {
                request_id: uuid::Uuid::new_v4().to_string(),
                runner_id: runner.binding.runner_id.clone(),
                runner_generation: runner.binding.generation,
                adapter_build_sha256: runner.binding.adapter_build_sha256.clone(),
                project_id: order.project_id.clone(),
                workspace_resource_id: workspace.folder.resource_id.clone(),
                environment: request.environment.clone(),
                operation: request.operation,
                expected_sandbox_uid: None,
                issued_at: at,
                expires_at: at + 30_000,
            },
        )
        .unwrap();
    let hash = canonical_hash(&(order.project_id.as_str(), order.id.as_str(), &request));
    f.runtime
        .store
        .prepare_sandbox_operation(
            &order.project_id,
            &order.id,
            &workspace.folder.resource_id,
            &request.idempotency_key,
            &hash,
            request.operation,
            &serde_json::to_value(frozen).unwrap(),
        )
        .await
        .unwrap();
    let env = f
        .runtime
        .store
        .environment(&order.project_id, &request.environment.environment_id)
        .await
        .unwrap();
    f.runtime
        .store
        .change_environment(
            &order.project_id,
            &env.id,
            env.revision,
            "revoke-before-retry",
            &EnvironmentChange::Revoke,
        )
        .await
        .unwrap();
    assert!(matches!(
        f.runtime
            .manage_sandbox_session(&order.project_id, &order.id, &request)
            .await,
        Err(Error::Conflict)
    ));
    let history = f
        .runtime
        .sandbox_operation_history(&order.project_id, &order.id)
        .await
        .unwrap();
    assert_eq!(history[0].state, "expired");
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn definitive_nonadmission_releases_workspace_without_replaying_old_request() {
    let mut f = setup_artifact(true).await;
    let original = f.runtime.process_runner.clone().unwrap();
    let runner = Arc::new(LifecycleRunner {
        inner: f.runner.clone(),
        replies: Default::default(),
        posts: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
        lose_reply: AtomicBool::new(false),
        reject_next: AtomicBool::new(true),
    });
    f.runtime = f
        .runtime
        .with_registered_process_runner(
            ProcessRunnerConfig::new(
                original.binding.clone(),
                SigningKey::from_pkcs8(&f.control_pkcs8).unwrap(),
                runner.clone(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let order = f.runtime.store.order(&f.input.order_id).await.unwrap();
    let mut request = ManageSandboxSession {
        environment: f.input.environment.clone().unwrap(),
        idempotency_key: "not-admitted".into(),
        operation: SandboxLifecycleOperation::Ensure,
        expected_sandbox_uid: None,
    };
    assert!(matches!(
        f.runtime
            .manage_sandbox_session(&order.project_id, &order.id, &request)
            .await,
        Err(Error::WorkspaceBusy)
    ));
    let history = f
        .runtime
        .sandbox_operation_history(&order.project_id, &order.id)
        .await
        .unwrap();
    assert_eq!(history[0].state, "rejected");
    assert!(
        f.runtime
            .manage_sandbox_session(&order.project_id, &order.id, &request)
            .await
            .is_err()
    );
    assert_eq!(runner.posts.load(Ordering::SeqCst), 1);
    request.idempotency_key = "new-explicit-intent".into();
    f.runtime
        .manage_sandbox_session(&order.project_id, &order.id, &request)
        .await
        .unwrap();
    assert_eq!(runner.posts.load(Ordering::SeqCst), 2);
}
