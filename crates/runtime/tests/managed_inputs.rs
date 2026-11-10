//! Real controller/store/broker admission with synthetic signed qualification.
//! No physical isolation or released runtime profile is inferred from this test.
use cowork_broker::{FileBroker, FolderConfig};
use cowork_contracts::*;
use cowork_domain::{Error, canonical_hash, environments::*, hash};
use cowork_runner_protocol::{Binding, LaunchRequest, Receipt, Signed, SigningKey};
use cowork_runtime::{ConfiguredProvider, ProcessRunnerAccess, ProcessRunnerConfig, Runtime};
use cowork_storage::{Change, ProcessAdmission, Store, now};
use cowork_workflow_contracts::*;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;
#[path = "managed_inputs/completions.rs"]
mod completions;
#[path = "managed_inputs/reconciliation.rs"]
mod reconciliation;
#[path = "managed_inputs/results.rs"]
mod results;

struct Runner {
    key: SigningKey,
    observation: EnvironmentObservation,
    legacy_launches: AtomicUsize,
}
impl Runner {
    fn observation(&self) -> EnvironmentObservation {
        EnvironmentObservation {
            observed_at: now(),
            ..self.observation.clone()
        }
    }
}
#[async_trait::async_trait]
impl ProcessRunnerAccess for Runner {
    async fn managed_input_ready(
        &self,
        _: &ExecutionEnvironmentRef,
        _: &ExecutionTargetRef,
    ) -> cowork_domain::Result<()> {
        Ok(())
    }
    async fn observe(
        &self,
        _: &ExecutionEnvironmentRef,
    ) -> cowork_domain::Result<Signed<EnvironmentObservation>> {
        self.key
            .sign("environment_observation", self.observation())
            .map_err(|_| Error::Forbidden)
    }
    async fn fence(
        &self,
        _: Signed<cowork_runner_protocol::FenceRequest>,
    ) -> cowork_domain::Result<()> {
        Ok(())
    }
    async fn renew_execution_lease(
        &self,
        _: Signed<cowork_runner_protocol::ExecutionLeaseRenewal>,
    ) -> cowork_domain::Result<()> {
        Err(Error::Forbidden)
    }
    async fn launch(
        &self,
        _: Signed<LaunchRequest>,
        _: CancellationToken,
    ) -> cowork_domain::Result<Signed<Receipt>> {
        self.legacy_launches.fetch_add(1, Ordering::SeqCst);
        Err(Error::Forbidden)
    }
}

struct Fixture {
    runtime: Runtime,
    runner: Arc<Runner>,
    control: SigningKey,
    binding: Binding,
    identity: ExecutionRunnerIdentity,
    input: PrepareOwnerExecutionPlanV2,
    pool: sqlx::AnyPool,
    dir: tempfile::TempDir,
}

struct LegacyOnly(Arc<Runner>);
#[async_trait::async_trait]
impl ProcessRunnerAccess for LegacyOnly {
    async fn observe(
        &self,
        reference: &ExecutionEnvironmentRef,
    ) -> cowork_domain::Result<Signed<EnvironmentObservation>> {
        self.0.observe(reference).await
    }
    async fn fence(
        &self,
        request: Signed<cowork_runner_protocol::FenceRequest>,
    ) -> cowork_domain::Result<()> {
        self.0.fence(request).await
    }
    async fn renew_execution_lease(
        &self,
        request: Signed<cowork_runner_protocol::ExecutionLeaseRenewal>,
    ) -> cowork_domain::Result<()> {
        self.0.renew_execution_lease(request).await
    }
    async fn launch(
        &self,
        request: Signed<LaunchRequest>,
        stop: CancellationToken,
    ) -> cowork_domain::Result<Signed<Receipt>> {
        self.0.launch(request, stop).await
    }
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let database = std::env::var("COWORK_TEST_MANAGED_INPUT_DATABASE_URL").ok();
        let url = database.clone().unwrap_or_else(|| {
            format!(
                "sqlite:{}?mode=rwc",
                dir.path().join("owner.sqlite").display()
            )
        });
        sqlx::any::install_default_drivers();
        let pool = sqlx::AnyPool::connect(&url).await.unwrap();
        if database.is_some() {
            static DATABASE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            let name: String = sqlx::query_scalar("SELECT current_database()::text")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert!(
                name.strip_prefix("cowork_managed_input_")
                    .is_some_and(|suffix| suffix.len() == 32
                        && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
            );
            if let Some(original) = DATABASE.get() {
                assert_eq!(original, &name);
            } else {
                assert_eq!(
                    sqlx::query_scalar::<_, i64>(
                        "SELECT COUNT(*) FROM pg_tables WHERE schemaname='public'"
                    )
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                    0
                );
                DATABASE.set(name).unwrap();
            }
        }
        let store = Store::connect(&url).await.unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Managed input authority".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Confirmed copy".into(),
            })
            .await
            .unwrap();
        std::fs::create_dir(dir.path().join("originals")).unwrap();
        std::fs::write(dir.path().join("originals/value.txt"), "original").unwrap();
        let broker = FileBroker::open(
            &dir.path().join("broker"),
            vec![FolderConfig {
                id: "source".into(),
                project_id: project.id.clone(),
                name: "Source".into(),
                path: dir.path().join("originals"),
                writable: true,
            }],
        )
        .unwrap();
        let folders = broker.grants(&project.id).unwrap();
        let control = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
        let key = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
        let binding = Binding {
            runner_id: "managed-input-fixture".into(),
            generation: 1,
            adapter_build_sha256: "a".repeat(64),
            control_public_key: control.public_key(),
            runner_public_key: key.public_key(),
        };
        let definition = EnvironmentDefinition {
            format_version: 3,
            network: Some(EnvironmentNetwork::Denied),
            artifact: EnvironmentArtifact::ManagedLinuxPackage {
                package_sha256: "b".repeat(64),
                rootfs_manifest_sha256: "c".repeat(64),
                gvisor_manifest_sha256: "d".repeat(64),
                maximum_files: 4096,
                output_bytes: 8192,
            },
            programs: BTreeMap::from([("probe".into(), "bin/probe".into())]),
            packages: vec![],
            limits: EnvironmentLimits {
                cpu_millis: 1000,
                memory_bytes: 256 * 1024 * 1024,
                processes: 32,
                scratch_bytes: 32 * 1024 * 1024,
            },
        };
        let environment = store
            .create_environment(&project.id, "Synthetic qualification", "create")
            .await
            .unwrap();
        let staged = store
            .change_environment(
                &project.id,
                &environment.id,
                1,
                "stage",
                &EnvironmentChange::Stage {
                    definition: definition.clone(),
                },
            )
            .await
            .unwrap();
        let version = staged.version_id.unwrap();
        let qualification = EnvironmentQualification {
            runner_id: binding.runner_id.clone(),
            runner_generation: 1,
            adapter_build_sha256: binding.adapter_build_sha256.clone(),
            artifact_sha256: definition.artifact.sha256().into(),
            definition_sha256: canonical_hash(&definition),
            report_sha256: "e".repeat(64),
            capabilities: definition.required_capabilities(),
            verified_at: now(),
        };
        store
            .change_environment(
                &project.id,
                &environment.id,
                2,
                "qualify",
                &EnvironmentChange::Qualify {
                    version_id: version.clone(),
                    qualification: qualification.clone(),
                },
            )
            .await
            .unwrap();
        let observation = EnvironmentObservation {
            runner_id: binding.runner_id.clone(),
            runner_generation: 1,
            adapter_build_sha256: binding.adapter_build_sha256.clone(),
            artifact_sha256: qualification.artifact_sha256,
            definition_sha256: qualification.definition_sha256,
            capabilities: qualification.capabilities,
            observed_at: now(),
        };
        store
            .change_environment(
                &project.id,
                &environment.id,
                3,
                "activate",
                &EnvironmentChange::Activate {
                    version_id: version,
                    observation: observation.clone(),
                },
            )
            .await
            .unwrap();
        let pin = store
            .pin_environment(&project.id, &environment.id, &observation)
            .await
            .unwrap();
        let profile: ProviderProfile = serde_json::from_value(json!({"id":"fixture","name":"Fixture","kind":"test","endpoint":"","model":"fixture","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true})).unwrap();
        store.insert_provider(&profile).await.unwrap();
        let runner = Arc::new(Runner {
            key,
            observation,
            legacy_launches: AtomicUsize::new(0),
        });
        let config =
            ProcessRunnerConfig::new(binding.clone(), control.clone(), runner.clone()).unwrap();
        let identity = config.execution_identity();
        store.register_execution_target(&identity, 0).await.unwrap();
        store
            .set_execution_target_active(&identity, 1, true)
            .await
            .unwrap();
        let runtime = Runtime::new(
            store,
            vec![ConfiguredProvider {
                profile,
                credential_env: None,
            }],
            true,
        )
        .unwrap()
        .with_file_broker(broker)
        .with_registered_process_runner(config)
        .await
        .unwrap();
        runtime.workspace(&order.id).await.unwrap();
        let selected = runtime
            .prepare_execution_input(
                &order.id,
                &PrepareExecutionInput {
                    idempotency_key: "capture".into(),
                    base_revision: order.revision,
                    folder_ids: vec!["source".into()],
                },
            )
            .await
            .unwrap();
        let request = serde_json::from_value(json!({"order_id":order.id,"base_revision":order.revision,"idempotency_key":"confirmed","prompt":"Use a separate copy",
            "provider_id":"fixture","provider_revision":1,"access_mode":"autonomous","budgets":Budgets::default(),"folders":folders,"environment":pin.reference()})).unwrap();
        Self {
            runtime,
            runner,
            control,
            binding,
            identity,
            input: PrepareOwnerExecutionPlanV2 {
                target: OwnerExecutionTargetV2::RegisteredRunner {},
                request: RunRequest::Single(request),
                input: ExecutionInputRef {
                    snapshot_id: selected.snapshot.id,
                    sha256: selected.sha256,
                    manifest_sha256: selected.snapshot.manifest.sha256,
                },
            },
            pool,
            dir,
        }
    }

    async fn prepare(&self) -> PreparedExecutionPlanV2 {
        self.runtime
            .prepare_execution_plan_v2(&self.input)
            .await
            .unwrap()
    }
    fn order(&self) -> &str {
        let RunRequest::Single(r) = &self.input.request else {
            unreachable!()
        };
        &r.order_id
    }
    async fn admitted(
        &self,
        plan: &PreparedExecutionPlanV2,
    ) -> (RunRecord, String, Signed<LaunchRequest>) {
        let (run, token, request, permit) = self.admitted_with_permit(plan).await;
        let _ = permit.into_attempt();
        (run, token, request)
    }
    async fn admitted_with_permit(
        &self,
        plan: &PreparedExecutionPlanV2,
    ) -> (
        RunRecord,
        String,
        Signed<LaunchRequest>,
        cowork_storage::ProcessLaunchPermit,
    ) {
        let run = self
            .runtime
            .commit_execution_plan_v2(
                self.order(),
                &plan.plan.id,
                &CommitExecutionPlan {
                    sha256: plan.sha256.clone(),
                },
            )
            .await
            .unwrap();
        // A later confirmed-copy edit is distinct from both original and initial snapshot.
        let call = ToolCall {
            id: "copy-edit".into(),
            name: "write_text_file".into(),
            arguments: json!({"folder_id":"source","path":"value.txt","content":"derived copy","expected_sha256":hash(b"original")}),
        };
        let mut write = self
            .runtime
            .files
            .as_ref()
            .unwrap()
            .prepare_write(&run, &call)
            .unwrap();
        write.apply().unwrap();
        drop(write);
        let claim = self.runtime.store.claim().await.unwrap().unwrap();
        assert_eq!(claim.record.id, run.id);
        let token = claim.lease.unwrap().token;
        let tool = ToolCall {
            id: "program-once".into(),
            name: "execute_program".into(),
            arguments: json!({"program":"probe","arguments":["value"],"timeout_seconds":5}),
        };
        self.runtime
            .store
            .mutate(&run.id, Some(&token), |r| {
                r.checkpoint.pending_tools = vec![tool.clone()];
                Ok(Change::event("fixture_model_proposal", json!({})))
            })
            .await
            .unwrap();
        let permit = match self
            .runtime
            .store
            .admit_process(&run.id, &token, &tool, &self.runner.observation())
            .await
            .unwrap()
        {
            ProcessAdmission::Launch(permit) => permit,
            _ => panic!("First real admission must own the permit"),
        };
        let attempt = permit.attempt();
        let current = self.runtime.store.stored_run(&run.id).await.unwrap();
        let issued = now();
        let request = LaunchRequest {
            format_version: 1,
            runner_id: self.binding.runner_id.clone(),
            runner_generation: self.binding.generation,
            adapter_build_sha256: self.binding.adapter_build_sha256.clone(),
            process_attempt_id: attempt.id.clone(),
            run_id: run.id.clone(),
            tool_call_id: tool.id.clone(),
            request_sha256: attempt.request_sha256.clone(),
            worker_fence_sha256: hash(token.as_bytes()),
            lease_epoch: current.record.revision as u64,
            environment: attempt.environment.clone(),
            workspace_resource_id: attempt.workspace_resource_id.clone(),
            invocation: cowork_domain::processes::process_invocation(&tool).unwrap(),
            issued_at: issued,
            expires_at: (issued + 30_000).min(current.lease.unwrap().until),
            execution_deadline: issued + 60_000,
        };
        (
            run,
            token,
            self.control.sign("launch", request).unwrap(),
            permit,
        )
    }
    async fn close(self) {
        loop {
            self.pool.close().await;
            if self.pool.size() == 0 {
                break;
            }
            eprintln!("Fixture raw pool received a late returning connection during close");
            tokio::task::yield_now().await;
        }
        self.runtime.store.close().await;
        let dir = self.dir;
        drop(self.runtime);
        drop(self.pool);
        drop(self.runner);
        #[cfg(windows)]
        {
            // tempfile wraps and hides raw_os_error. Use its same std cleanup
            // operation directly so only native sharing/lock errors can retry.
            let path = dir.keep();
            assert_eq!(
                path.parent().unwrap().canonicalize().unwrap(),
                std::env::temp_dir().canonicalize().unwrap()
            );
            if let Err(error) = std::fs::remove_dir_all(&path) {
                finish_windows_fixture_cleanup(&path, error).await.unwrap();
            }
        }
        #[cfg(not(windows))]
        dir.close().unwrap();
    }
}

// Windows can deny deletion after our pools and handles have closed. Retry only
// the exact remaining fixture database, and keep every other cleanup error fatal.
#[cfg(windows)]
async fn finish_windows_fixture_cleanup(
    path: &std::path::Path,
    initial: std::io::Error,
) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    use std::time::{Duration, Instant};

    if !matches!(initial.raw_os_error(), Some(32 | 33)) {
        return Err(initial);
    }
    let started = Instant::now();
    eprintln!("Fixture cleanup at {}: {initial}", path.display());
    loop {
        let directory = std::fs::symlink_metadata(path)?;
        if !directory.is_dir() || directory.file_attributes() & 0x400 != 0 {
            return Err(initial);
        }
        let entries = std::fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
        if entries.len() != 1 || entries[0].file_name() != "owner.sqlite" {
            return Err(initial);
        }
        let database = path.join("owner.sqlite");
        let metadata = std::fs::symlink_metadata(&database)?;
        if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
            return Err(initial);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        if started.elapsed() >= Duration::from_secs(2) {
            return Err(initial);
        }
        match std::fs::remove_file(&database) {
            Ok(()) => {
                std::fs::remove_dir(path)?;
                eprintln!("Fixture cleanup completed after {:?}", started.elapsed());
                return Ok(());
            }
            Err(error) if matches!(error.raw_os_error(), Some(32 | 33)) => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(windows)]
#[tokio::test]
async fn a_windows_fixture_cleanup_handles_native_sharing_violation() {
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::Duration;

    let path = tempfile::tempdir().unwrap().keep();
    let database = path.join("owner.sqlite");
    std::fs::write(&database, b"owned fixture").unwrap();
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&database)
        .unwrap();
    let error = std::fs::remove_dir_all(&path).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(32));
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(handle);
    });
    finish_windows_fixture_cleanup(&path, error).await.unwrap();
    release.await.unwrap();
    assert!(!path.exists());

    let path = tempfile::tempdir().unwrap().keep();
    let database = path.join("owner.sqlite");
    std::fs::write(&database, b"owned fixture").unwrap();
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&database)
        .unwrap();
    let error = std::fs::remove_dir_all(&path).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(32));
    std::fs::write(path.join("unexpected.txt"), b"keep this evidence").unwrap();
    assert!(finish_windows_fixture_cleanup(&path, error).await.is_err());
    assert_eq!(
        std::fs::read(path.join("unexpected.txt")).unwrap(),
        b"keep this evidence"
    );
    assert!(database.is_file());
    drop(handle);
    std::fs::remove_file(database).unwrap();
    std::fs::remove_file(path.join("unexpected.txt")).unwrap();
    std::fs::remove_dir(path).unwrap();
}

fn first_chunk(manifest: &WorkspaceTransferManifest) -> ReadWorkspaceTransferChunk {
    let file = manifest
        .files
        .iter()
        .find(|f| f.path.ends_with("/value.txt"))
        .unwrap();
    ReadWorkspaceTransferChunk {
        path: file.path.clone(),
        offset: file.chunks[0].offset,
        expected_sha256: file.chunks[0].sha256.clone(),
    }
}

#[tokio::test]
async fn confirmed_copy_mints_exact_transfer_and_replay_cannot_launch_again() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    assert!(matches!(
        plan.plan.target,
        ExecutionDestinationV2::ManagedRunner { .. }
    ));
    let (run, token, request) = f.admitted(&plan).await;
    std::fs::write(f.dir.path().join("originals/value.txt"), "changed original").unwrap();
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request.clone())
        .await
        .unwrap();
    let manifest = source.manifest().await.unwrap();
    assert_eq!(manifest.grant.payload.execution_plan_id, plan.plan.id);
    assert_eq!(manifest.grant.payload.execution_plan_sha256, plan.sha256);
    assert!(manifest.grant.payload.expires_at <= request.payload.expires_at);
    assert_eq!(
        source
            .chunk(first_chunk(&manifest.manifest))
            .await
            .unwrap()
            .bytes,
        b"derived copy"
    );
    let replay = f
        .runtime
        .prepare_managed_process_input(&token, request.clone())
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(&replay.manifest().await.unwrap().grant),
        canonical_hash(&manifest.grant)
    );
    let tool = f
        .runtime
        .store
        .stored_run(&run.id)
        .await
        .unwrap()
        .checkpoint
        .in_flight
        .unwrap();
    assert!(matches!(
        f.runtime
            .store
            .admit_process(&run.id, &token, &tool, &f.runner.observation())
            .await
            .unwrap(),
        ProcessAdmission::Existing(_)
    ));
    assert!(
        f.runner
            .managed_launch(request, source.clone(), CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    if let Some(path) = std::env::var_os("COWORK_TEST_EXPORT_MANAGED_INPUT_FIXTURE") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({"prepare":f.input,"prepared":plan,"run":run}))
                .unwrap(),
        )
        .unwrap();
    }
    drop(source);
    drop(replay);
    f.close().await;
}

#[tokio::test]
async fn original_untransferred_permit_settles_preparation_failure_and_revokes_retained_input() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (run, token, request, permit) = f.admitted_with_permit(&plan).await;
    let tool = permit.attempt().tool.clone();
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request.clone())
        .await
        .unwrap();
    let manifest = source.manifest().await.unwrap();
    let settled = permit
        .reject_before_transport(&token, "input_preparation_failed")
        .await
        .unwrap();
    assert!(matches!(
        settled.state,
        ProcessAttemptState::ReconciledNotStarted { .. }
    ));
    let current = f.runtime.store.stored_run(&run.id).await.unwrap();
    assert!(current.checkpoint.in_flight.is_none());
    assert!(current.checkpoint.pending_tools.is_empty());
    assert!(source.manifest().await.is_err());
    assert!(source.chunk(first_chunk(&manifest.manifest)).await.is_err());
    assert!(
        f.runtime
            .prepare_managed_process_input(&token, request)
            .await
            .is_err()
    );
    let ProcessAdmission::Existing(replay) = f
        .runtime
        .store
        .admit_process(&run.id, &token, &tool, &f.runner.observation())
        .await
        .unwrap()
    else {
        panic!("A settled attempt cannot mint a second permit");
    };
    assert_eq!(replay, settled);
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    drop(source);
    f.close().await;
}

#[tokio::test]
async fn a_signed_label_does_not_replace_confirmed_plan_and_exact_process_intent() {
    let f = Fixture::new().await;
    let mut broker = f.input.clone();
    broker.target = OwnerExecutionTargetV2::BrokerController {};
    assert!(f.runtime.prepare_execution_plan_v2(&broker).await.is_err());
    let plan = f.prepare().await;
    let (_, token, original) = f.admitted(&plan).await;
    let changes: [fn(&mut LaunchRequest); 6] = [
        |r| r.invocation.arguments.push("unconfirmed".into()),
        |r| r.workspace_resource_id.push('x'),
        |r| r.worker_fence_sha256 = "f".repeat(64),
        |r| r.process_attempt_id.push('x'),
        |r| r.environment.qualification_sha256 = "1".repeat(64),
        |r| r.lease_epoch += 100,
    ];
    for change in changes {
        let mut altered = original.payload.clone();
        change(&mut altered);
        assert!(
            f.runtime
                .prepare_managed_process_input(&token, f.control.sign("launch", altered).unwrap())
                .await
                .is_err()
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM owner_managed_input_transfers")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let mut unconfirmed = original.payload.clone();
    unconfirmed.run_id = "unconfirmed-run".into();
    assert!(
        f.runtime
            .prepare_managed_process_input(&token, f.control.sign("launch", unconfirmed).unwrap())
            .await
            .is_err()
    );
    f.close().await;
}

async fn recheck_stopped_source(cancel: bool) {
    let f = Fixture::new().await;
    let p = f.prepare().await;
    let (run, token, request) = f.admitted(&p).await;
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request)
        .await
        .unwrap();
    let original = source.manifest().await.unwrap();
    let chunk = first_chunk(&original.manifest);
    if cancel {
        f.runtime.store.cancel(&run.id).await.unwrap();
    } else {
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
    }
    assert!(source.manifest().await.is_err());
    assert!(source.chunk(chunk).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_input_transfers")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        1
    );
    drop(source);
    f.close().await;
}

#[tokio::test]
async fn every_source_read_rechecks_target_revision_after_reactivation() {
    recheck_stopped_source(false).await;
}

#[tokio::test]
async fn every_source_read_rechecks_worker_cancellation() {
    recheck_stopped_source(true).await;
}

#[tokio::test]
async fn retained_chunk_corruption_has_no_original_or_mutable_copy_fallback() {
    let f = Fixture::new().await;
    let p = f.prepare().await;
    let (_, token, request) = f.admitted(&p).await;
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request)
        .await
        .unwrap();
    let original = source.manifest().await.unwrap();
    let chunk = first_chunk(&original.manifest);
    let file = original
        .manifest
        .files
        .iter()
        .find(|file| file.path == chunk.path)
        .unwrap();
    std::fs::write(
        f.dir.path().join("broker/versions").join(&file.sha256),
        "corrupt copy",
    )
    .unwrap();
    assert!(source.chunk(chunk).await.is_err());
    assert_eq!(
        std::fs::read(f.dir.path().join("originals/value.txt")).unwrap(),
        b"original"
    );
    drop(source);
    f.close().await;
}

#[tokio::test]
async fn resource_revocation_blocks_manifest_and_chunk_without_rebinding_a_copy() {
    let f = Fixture::new().await;
    let p = f.prepare().await;
    let (_, token, request) = f.admitted(&p).await;
    let source = f
        .runtime
        .prepare_managed_process_input(&token, request)
        .await
        .unwrap();
    let original = source.manifest().await.unwrap();
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
    assert!(source.manifest().await.is_err());
    assert!(source.chunk(first_chunk(&original.manifest)).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_input_transfers")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        1
    );
    drop(source);
    f.close().await;
}

#[tokio::test]
async fn new_admission_rechecks_target_and_does_not_widen_readonly_aliases() {
    let f = Fixture::new().await;
    let mut readonly = f.input.clone();
    let RunRequest::Single(request) = &mut readonly.request else {
        unreachable!()
    };
    request.folders[0].writable = false;
    assert!(
        matches!(f.runtime.prepare_execution_plan_v2(&readonly).await, Err(Error::Validation(message)) if message=="managed_snapshot_scope_not_supported")
    );
    let p = f.prepare().await;
    f.runtime
        .store
        .set_execution_target_active(&f.identity, 2, false)
        .await
        .unwrap();
    assert!(
        f.runtime
            .commit_execution_plan_v2(
                f.order(),
                &p.plan.id,
                &CommitExecutionPlan { sha256: p.sha256 }
            )
            .await
            .is_err()
    );
    assert!(f.runtime.store.runs(f.order()).await.unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
async fn an_older_transport_cannot_prepare_a_ready_managed_plan() {
    let f = Fixture::new().await;
    let config = ProcessRunnerConfig::new(
        f.binding.clone(),
        f.control.clone(),
        Arc::new(LegacyOnly(f.runner.clone())),
    )
    .unwrap();
    let legacy = f
        .runtime
        .clone()
        .with_registered_process_runner(config)
        .await
        .unwrap();
    assert!(legacy.prepare_execution_plan_v2(&f.input).await.is_err());
    assert!(f.runtime.store.runs(f.order()).await.unwrap().is_empty());
    assert_eq!(f.runner.legacy_launches.load(Ordering::SeqCst), 0);
    drop(legacy);
    f.close().await;
}
