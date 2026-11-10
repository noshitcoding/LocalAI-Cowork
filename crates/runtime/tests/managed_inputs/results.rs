//! Real controller/Store/FileBroker rights with a synthetic signed result peer.
//! These fixtures do not prove physical execution or install any profile.
use super::*;
use cowork_runner_protocol::{
    ExecutionState, ManagedOutputAttestation, ManagedReadiness, ManagedReadyRequest,
    ManagedResultContextRequest, ManagedResultManifest, ManagedResultReadContext,
    ManagedResultReadOperation, ManagedResultReadRequest,
};
use std::{sync::Mutex, time::Duration};
#[path = "results/context.rs"]
mod context_tests;
#[path = "results/delivery.rs"]
mod delivery_tests;
#[path = "results/versions.rs"]
mod versions;

struct ResultRunner {
    inner: Arc<Runner>,
    input: cowork_runtime::ManagedInputManifest,
    launch: Signed<LaunchRequest>,
    receipt: Signed<Receipt>,
    manifest: WorkspaceTransferManifest,
    store: Store,
    mode: AtomicUsize,
    reads: AtomicUsize,
    requests: Mutex<Vec<Signed<ManagedResultReadRequest>>>,
    delayed_checks: AtomicUsize,
    contexts: AtomicUsize,
    context_requests: Mutex<Vec<Signed<ManagedResultContextRequest>>>,
    delivery_requests: Mutex<Vec<Signed<cowork_runner_protocol::ManagedResultDeliveryRequest>>>,
    delivery_history_requests:
        Mutex<Vec<Signed<cowork_runner_protocol::ManagedResultDeliveryRequest>>>,
    delivery_accepted: Mutex<Option<Signed<cowork_runner_protocol::ManagedResultDeliveryReceipt>>>,
    delivery_binding: Binding,
    delivery_mode: AtomicUsize,
    retirement_queries: AtomicUsize,
    retirement_state: AtomicUsize,
}
impl ResultRunner {
    fn result(&self) -> ManagedResultManifest {
        let g = &self.input.grant.payload;
        let mode = self.mode.load(Ordering::SeqCst);
        let at = if mode == 8 {
            now()
        } else {
            self.receipt.payload.updated_at
        };
        let mut manifest = self.manifest.clone();
        if mode == 4 {
            manifest.files[0].path = "unapproved/value.txt".into();
            manifest.directories = vec!["unapproved".into()];
            manifest.sha256 = cowork_domain::workspace_transfer::manifest_hash(&manifest);
        }
        let mut value = ManagedOutputAttestation {
            format_version: 1,
            runtime_instance_sha256: "9".repeat(64),
            execution_plan_id: g.execution_plan_id.clone(),
            execution_plan_sha256: g.execution_plan_sha256.clone(),
            process_attempt_id: g.process_attempt_id.clone(),
            run_id: g.run_id.clone(),
            launch_envelope_sha256: cowork_runner_protocol::envelope_hash(&self.launch),
            input_manifest_sha256: g.input_manifest_sha256.clone(),
            output_manifest_sha256: manifest.sha256.clone(),
            execution_receipt_sha256: canonical_hash(&self.receipt),
            physical_record_sha256: "a".repeat(64),
            runner: g.runner.clone(),
            target: g.target.clone(),
            produced_at: at,
            retained_until: at + if mode == 9 { 5000 } else { 60000 },
        };
        match mode {
            1 => value.execution_receipt_sha256 = "0".repeat(64),
            2 => value.runtime_instance_sha256 = "8".repeat(64),
            3 => value.input_manifest_sha256 = "0".repeat(64),
            5 => {
                value.produced_at = at - 60000;
                value.retained_until = at - 1;
            }
            8 => value.retained_until = at + 500,
            _ => {}
        }
        ManagedResultManifest {
            attestation: self.inner.key.sign("managed_output", value).unwrap(),
            manifest,
        }
    }
}
#[async_trait::async_trait]
impl ProcessRunnerAccess for ResultRunner {
    async fn managed_result_delivery_history(
        &self,
        request: Signed<cowork_runner_protocol::ManagedResultDeliveryRequest>,
    ) -> cowork_domain::Result<Signed<cowork_runner_protocol::ManagedResultDeliveryReceipt>> {
        self.fixture_delivery_history(request).await
    }
    async fn managed_result_delivery(
        &self,
        request: Signed<cowork_runner_protocol::ManagedResultDeliveryRequest>,
    ) -> cowork_domain::Result<cowork_runtime::ManagedResultDeliveryResponse> {
        self.fixture_delivery(request).await
    }
    async fn managed_result_retirement(
        &self,
        request: Signed<cowork_runner_protocol::ManagedResultRetirementQuery>,
    ) -> cowork_domain::Result<Signed<cowork_runner_protocol::ManagedResultRetirementReceipt>> {
        self.fixture_retirement(request).await
    }
    async fn managed_result_context(
        &self,
        request: Signed<ManagedResultContextRequest>,
    ) -> cowork_domain::Result<Signed<ManagedResultReadContext>> {
        request
            .verify(
                "managed_result_context_request",
                &self.input.grant.payload.runner.control_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        self.contexts.fetch_add(1, Ordering::SeqCst);
        self.context_requests.lock().unwrap().push(request.clone());
        let mode = self.mode.load(Ordering::SeqCst);
        if mode == 15 {
            return Err(Error::UncertainEffect);
        }
        let mut value = ManagedResultReadContext {
            format_version: 1,
            request: request.payload,
            fence_sha256: "c".repeat(64),
            observed_at: now(),
            expires_at: 0,
        };
        value.expires_at = value
            .request
            .expires_at
            .min(self.result().attestation.payload.retained_until);
        match mode {
            10 => value.request.request_id = uuid::Uuid::new_v4().to_string(),
            12 => value.request.execution_receipt_sha256 = "0".repeat(64),
            13 => value.expires_at = value.observed_at,
            14 => {
                self.store.cancel(&self.launch.payload.run_id).await?;
            }
            16 => {
                value.expires_at = value.observed_at + 10;
                tokio::time::sleep(Duration::from_millis(35)).await;
            }
            17 => {
                self.store
                    .set_execution_target_active(&self.input.grant.payload.runner, 2, false)
                    .await?;
            }
            _ => {}
        }
        let signer = if mode == 11 {
            cowork_runner_protocol::SigningKey::from_pkcs8(
                &cowork_runner_protocol::SigningKey::generate_pkcs8().unwrap(),
            )
            .unwrap()
        } else {
            self.inner.key.clone()
        };
        signer
            .sign("managed_result_context", value)
            .map_err(|_| Error::Forbidden)
    }
    async fn managed_input_ready(
        &self,
        e: &ExecutionEnvironmentRef,
        t: &ExecutionTargetRef,
    ) -> cowork_domain::Result<()> {
        self.inner.managed_input_ready(e, t).await
    }
    async fn managed_result_readiness(
        &self,
        r: ManagedReadyRequest,
    ) -> cowork_domain::Result<Signed<ManagedReadiness>> {
        let at = now();
        self.inner
            .key
            .sign(
                "managed_readiness",
                ManagedReadiness {
                    format_version: 1,
                    request_id: r.request_id.clone(),
                    request_sha256: canonical_hash(&r),
                    runner: self.input.grant.payload.runner.clone(),
                    environment: r.environment,
                    target: r.target,
                    runtime_instance_sha256: "9".repeat(64),
                    observed_at: at,
                    expires_at: at + 15000,
                },
            )
            .map_err(|_| Error::Forbidden)
    }
    async fn managed_result_manifest(
        &self,
        r: Signed<ManagedResultReadRequest>,
    ) -> cowork_domain::Result<ManagedResultManifest> {
        r.verify(
            "managed_result_read",
            &self.input.grant.payload.runner.control_public_key,
        )
        .unwrap();
        assert!(matches!(
            r.payload.operation,
            ManagedResultReadOperation::Manifest
        ));
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(r);
        Ok(self.result())
    }
    async fn managed_result_chunk(
        &self,
        r: Signed<ManagedResultReadRequest>,
        expected: u64,
    ) -> cowork_domain::Result<Vec<u8>> {
        r.verify(
            "managed_result_read",
            &self.input.grant.payload.runner.control_public_key,
        )
        .unwrap();
        assert!(matches!(
            r.payload.operation,
            ManagedResultReadOperation::Chunk { .. }
        ));
        assert_eq!(expected, b"native result".len() as u64);
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(r);
        if self.mode.load(Ordering::SeqCst) == 6 {
            self.store.cancel(&self.launch.payload.run_id).await?;
        }
        if self.mode.load(Ordering::SeqCst) == 7 {
            return Ok(b"corrupt bytes".to_vec());
        }
        Ok(b"native result".to_vec())
    }
    async fn observe(
        &self,
        e: &ExecutionEnvironmentRef,
    ) -> cowork_domain::Result<Signed<EnvironmentObservation>> {
        if self.mode.load(Ordering::SeqCst) == 8 && self.reads.load(Ordering::SeqCst) > 0 {
            self.delayed_checks.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(1100)).await;
        }
        self.inner.observe(e).await
    }
    async fn fence(
        &self,
        r: Signed<cowork_runner_protocol::FenceRequest>,
    ) -> cowork_domain::Result<()> {
        self.inner.fence(r).await
    }
    async fn renew_execution_lease(
        &self,
        r: Signed<cowork_runner_protocol::ExecutionLeaseRenewal>,
    ) -> cowork_domain::Result<()> {
        self.inner.renew_execution_lease(r).await
    }
    async fn launch(
        &self,
        r: Signed<LaunchRequest>,
        s: CancellationToken,
    ) -> cowork_domain::Result<Signed<Receipt>> {
        self.inner.launch(r, s).await
    }
}

async fn prepared(settle: bool) -> (Fixture, Runtime, Arc<ResultRunner>, RunRecord, String) {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (run, token, launch) = f.admitted(&plan).await;
    let source = f
        .runtime
        .prepare_managed_process_input(&token, launch.clone())
        .await
        .unwrap();
    let input = source.manifest().await.unwrap();
    drop(source);
    let result = ProcessResult {
        termination: ProcessTermination::Exited,
        process_id: Some(42),
        exit_code: Some(0),
        signal: None,
        stdout: ProcessCapture {
            bytes: vec![],
            sha256: hash(b""),
            total_bytes: 0,
            truncated: false,
        },
        stderr: ProcessCapture {
            bytes: vec![],
            sha256: hash(b""),
            total_bytes: 0,
            truncated: false,
        },
        duration_ms: 1,
        output_complete: true,
    };
    let receipt = f
        .runner
        .key
        .sign(
            "receipt",
            Receipt {
                request: launch.payload.clone(),
                request_envelope_sha256: cowork_runner_protocol::envelope_hash(&launch),
                state: ExecutionState::Completed {
                    result: result.clone(),
                },
                revision: 3,
                updated_at: now(),
            },
        )
        .unwrap();
    f.runtime
        .store
        .retain_managed_completion_receipt(&receipt)
        .await
        .unwrap();
    if settle {
        f.runtime
            .store
            .finish_process(&run.id, &token, &launch.payload.tool_call_id, result)
            .await
            .unwrap();
    }
    let path = first_chunk(&input.manifest).path;
    let manifest = cowork_domain::workspace_transfer::build(
        input.manifest.directories.clone(),
        vec![WorkspaceTransferFile {
            path,
            size: b"native result".len() as u64,
            sha256: hash(b"native result"),
            chunks: vec![WorkspaceTransferChunk {
                offset: 0,
                size: b"native result".len() as u64,
                sha256: hash(b"native result"),
            }],
        }],
    )
    .unwrap();
    let peer = Arc::new(ResultRunner {
        inner: f.runner.clone(),
        input,
        launch,
        receipt,
        manifest,
        store: f.runtime.store.clone(),
        mode: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
        requests: Mutex::new(Vec::new()),
        delayed_checks: AtomicUsize::new(0),
        contexts: AtomicUsize::new(0),
        context_requests: Mutex::new(Vec::new()),
        delivery_requests: Mutex::new(Vec::new()),
        delivery_history_requests: Mutex::new(Vec::new()),
        delivery_accepted: Mutex::new(None),
        delivery_binding: f.binding.clone(),
        delivery_mode: AtomicUsize::new(0),
        retirement_queries: AtomicUsize::new(0),
        retirement_state: AtomicUsize::new(1),
    });
    let runtime = f
        .runtime
        .clone()
        .with_registered_process_runner(
            ProcessRunnerConfig::new(f.binding.clone(), f.control.clone(), peer.clone()).unwrap(),
        )
        .await
        .unwrap();
    (f, runtime, peer, run, token)
}
fn chunk(result: &ManagedResultManifest) -> ReadWorkspaceTransferChunk {
    let f = &result.manifest.files[0];
    ReadWorkspaceTransferChunk {
        path: f.path.clone(),
        offset: 0,
        expected_sha256: f.chunks[0].sha256.clone(),
    }
}

#[tokio::test]
async fn settled_result_uses_fresh_content_rights_without_reviving_worker() {
    let (f, runtime, peer, run, token) = prepared(true).await;
    let snapshot = f
        .runtime
        .files
        .as_ref()
        .unwrap()
        .snapshot_workspace(&f.runtime.store.stored_run(&run.id).await.unwrap().record)
        .unwrap();
    f.runtime
        .store
        .mutate(&run.id, Some(&token), |r| {
            r.transition(RunState::Completed)?;
            r.lease = None;
            r.checkpoint.completion_pending = false;
            let mut change = Change::event("fixture_completed", json!({}));
            change.workspace_snapshot = Some(snapshot.snapshot.clone());
            Ok(change)
        })
        .await
        .unwrap();
    drop(snapshot);
    let before = f
        .runtime
        .store
        .stored_run(&run.id)
        .await
        .unwrap()
        .record
        .revision;
    let result = runtime
        .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
        .await
        .unwrap();
    assert_eq!(
        runtime
            .managed_result_chunk(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id,
                &result,
                chunk(&result)
            )
            .await
            .unwrap(),
        b"native result"
    );
    let stored = f.runtime.store.stored_run(&run.id).await.unwrap();
    assert!(stored.lease.is_none());
    assert_eq!(stored.record.revision, before);
    assert_eq!(stored.record.state, RunState::Completed);
    {
        let requests = peer.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_ne!(
            requests[0].payload.request_id,
            requests[1].payload.request_id
        );
        for r in requests.iter() {
            assert_eq!(r.payload.format_version, 2);
            assert!(r.payload.context.is_some());
            assert_eq!(
                r.payload.execution_receipt_sha256,
                canonical_hash(&peer.receipt)
            );
            assert!(r.payload.expires_at - r.payload.issued_at <= 30000);
            assert!(r.payload.actor_id.starts_with("installation-owner:"));
        }
    }
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    assert_eq!(
        std::fs::read(f.dir.path().join("originals/value.txt")).unwrap(),
        b"original"
    );
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn observed_receipt_alone_cannot_authorize_a_not_yet_settled_attempt() {
    let (f, runtime, peer, run, _) = prepared(false).await;
    assert!(
        runtime
            .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 0);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn cancellation_prevents_new_result_authority() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    f.runtime.store.cancel(&run.id).await.unwrap();
    assert!(
        runtime
            .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 0);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn target_reactivation_cannot_rebind_old_result_reads() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    f.runtime
        .store
        .set_execution_target_active(&f.identity, 2, false)
        .await
        .unwrap();
    f.runtime
        .store
        .set_execution_target_active(&f.identity, 3, true)
        .await
        .unwrap();
    assert!(
        runtime
            .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 0);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn postread_cancellation_discards_received_result_bytes() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let result = runtime
        .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
        .await
        .unwrap();
    peer.mode.store(6, Ordering::SeqCst);
    assert!(
        runtime
            .managed_result_chunk(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id,
                &result,
                chunk(&result)
            )
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 2);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn original_result_binding_expiry_and_aliases_are_verified_before_return() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    for mode in 1..=5 {
        peer.mode.store(mode, Ordering::SeqCst);
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
    }
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn undeclared_chunks_and_corrupted_content_are_not_result_bytes() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let result = runtime
        .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
        .await
        .unwrap();
    let mut c = chunk(&result);
    c.offset = 1;
    assert!(
        runtime
            .managed_result_chunk(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id,
                &result,
                c
            )
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 1);
    peer.mode.store(7, Ordering::SeqCst);
    assert!(
        runtime
            .managed_result_chunk(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id,
                &result,
                chunk(&result)
            )
            .await
            .is_err()
    );
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn another_order_cannot_use_original_result_identifiers() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    assert!(
        runtime
            .managed_result_manifest(
                "another-order",
                &run.id,
                &peer.launch.payload.process_attempt_id
            )
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 0);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn revoked_original_folder_cannot_authorize_an_existing_result() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let result = runtime
        .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
        .await
        .unwrap();
    let RunRequest::Single(request) = &f.input.request else {
        unreachable!()
    };
    let grant = request.folders.iter().find(|g| g.id == "source").unwrap();
    f.runtime
        .files
        .as_ref()
        .unwrap()
        .revoke_native_folder(&grant.project_id, &grant.id, &grant.revision)
        .unwrap();
    assert!(
        runtime
            .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
            .await
            .is_err()
    );
    assert!(
        runtime
            .managed_result_chunk(
                f.order(),
                &run.id,
                &peer.launch.payload.process_attempt_id,
                &result,
                chunk(&result)
            )
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 1);
    drop(runtime);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn an_older_transport_cannot_read_retained_result_content() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    let config = ProcessRunnerConfig::new(
        f.binding.clone(),
        f.control.clone(),
        Arc::new(LegacyOnly(f.runner.clone())),
    )
    .unwrap();
    let legacy = runtime
        .with_registered_process_runner(config)
        .await
        .unwrap();
    assert!(
        legacy
            .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 0);
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    drop(legacy);
    drop(peer);
    f.close().await;
}

#[tokio::test]
async fn retention_expiring_during_fresh_authorization_is_not_returned() {
    let (f, runtime, peer, run, _) = prepared(true).await;
    peer.mode.store(8, Ordering::SeqCst);
    assert!(
        runtime
            .managed_result_manifest(f.order(), &run.id, &peer.launch.payload.process_attempt_id)
            .await
            .is_err()
    );
    assert_eq!(peer.reads.load(Ordering::SeqCst), 1);
    assert_eq!(peer.delayed_checks.load(Ordering::SeqCst), 1);
    drop(runtime);
    drop(peer);
    f.close().await;
}
