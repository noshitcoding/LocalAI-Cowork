//! In-process signed fixture transport with real owned child execution. Synthetic
//! qualification reports exercise bindings; they do not qualify platform isolation.
use super::*;
mod controller_crash;
mod sandbox_sessions;
use cowork_domain::environments::*;
use cowork_runner_protocol::{Admission, Broker};
use sqlx::any::AnyPoolOptions;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
};

struct FixtureRunner {
    store: Store,
    broker: Broker,
    signer: SigningKey,
    observation: EnvironmentObservation,
    directory: std::path::PathBuf,
    mode: AtomicU8,
    launches: AtomicUsize,
    renewals: AtomicUsize,
    renewal_requests: tokio::sync::Mutex<Vec<Signed<ExecutionLeaseRenewal>>>,
    supervisors: tokio::sync::Mutex<Vec<tokio::task::JoinHandle<Result<Signed<Receipt>>>>>,
}
#[async_trait::async_trait]
impl ProcessRunnerAccess for FixtureRunner {
    async fn observe(&self, _: &ExecutionEnvironmentRef) -> Result<Signed<EnvironmentObservation>> {
        let mut observation = self.observation.clone();
        observation.observed_at = now();
        if self.mode.load(Ordering::Acquire) == 2 {
            observation.observed_at -= 31_000;
        }
        self.signer
            .sign("environment_observation", observation)
            .map_err(|_| Error::Forbidden)
    }
    async fn fence(&self, request: Signed<FenceRequest>) -> Result<()> {
        if self.mode.load(Ordering::Acquire) == 7 {
            return Err(Error::Provider(
                "fixture_fence_unavailable_before_transport".into(),
            ));
        }
        if self.mode.load(Ordering::Acquire) == 8 {
            self.store.cancel(&request.payload.run_id).await?;
        }
        self.broker
            .fence(request)
            .await
            .map_err(|_| Error::Forbidden)
    }
    async fn renew_execution_lease(&self, request: Signed<ExecutionLeaseRenewal>) -> Result<()> {
        if self.mode.load(Ordering::Acquire) == 6 {
            return Err(Error::Provider(
                "fixture_renewal_transport_unavailable".into(),
            ));
        }
        self.broker
            .renew_execution_lease(request.clone())
            .await
            .map_err(|_| Error::Forbidden)?;
        self.renewal_requests.lock().await.push(request);
        self.renewals.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    async fn launch(
        &self,
        signed: Signed<LaunchRequest>,
        stop: CancellationToken,
    ) -> Result<Signed<Receipt>> {
        self.launches.fetch_add(1, Ordering::AcqRel);
        let admission = self
            .broker
            .admit(signed)
            .await
            .map_err(|_| Error::Forbidden)?;
        let request = match admission {
            Admission::Existing(receipt) => return Ok(receipt),
            Admission::Launch(permit) => self
                .broker
                .dispatch(permit)
                .await
                .map_err(|_| Error::Forbidden)?,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let end = CancellationToken::new();
        let watcher = {
            let cancel = cancel.clone();
            let end = end.clone();
            let broker = self.broker.clone();
            let request = request.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = end.cancelled() => break,
                        _ = stop.cancelled() => { cancel.store(true,Ordering::Release); break; },
                        _ = tokio::time::sleep(Duration::from_millis(20)) => {},
                    }
                    if !broker.execution_allowed(&request).await.unwrap_or(false) {
                        cancel.store(true, Ordering::Release);
                        break;
                    }
                }
            })
        };
        let marker = self.directory.join("effect.txt");
        let mut environment = BTreeMap::from([(
            "COWORK_TEST_EFFECT_PATH".into(),
            marker.to_string_lossy().into_owned(),
        )]);
        if request.invocation.arguments.contains(&"wait".into()) {
            environment.insert("COWORK_TEST_WAIT".into(), "1".into());
        }
        if request.invocation.arguments.contains(&"hold".into()) {
            environment.insert("COWORK_TEST_WAIT".into(), "hold".into());
        }
        let launch = cowork_executor::TrustedLaunch {
            program: std::env::current_exe().unwrap(),
            arguments: vec![
                "--exact".into(),
                "processes::tests::trusted_runner_child_effect".into(),
                "--nocapture".into(),
            ],
            directory: self.directory.clone(),
            environment,
            timeout: Duration::from_secs(request.invocation.timeout_seconds),
            capture_bytes_per_stream: 65_536,
        };
        // The adapter owns this supervisor independently of the transport future.
        // Error/corrupt replies below deliberately leave the real child running until
        // the product cancellation/fence path stops and reaps that exact process.
        let broker = self.broker.clone();
        let observed_request = request.clone();
        let supervisor = tokio::spawn(async move {
            let outcome =
                tokio::task::spawn_blocking(move || cowork_executor::supervise(&launch, &cancel))
                    .await
                    .unwrap();
            end.cancel();
            watcher.await.unwrap();
            let outcome = outcome.map_err(|_| Error::UncertainEffect)?;
            let capture = |c: cowork_executor::Capture| ProcessCapture {
                sha256: hash(&c.bytes),
                bytes: c.bytes,
                total_bytes: c.total_bytes,
                truncated: c.truncated,
            };
            let result = ProcessResult {
                termination: match outcome.termination {
                    cowork_executor::Termination::Exited => ProcessTermination::Exited,
                    cowork_executor::Termination::Canceled => ProcessTermination::Canceled,
                    cowork_executor::Termination::TimedOut => ProcessTermination::TimedOut,
                },
                process_id: outcome.process_id,
                exit_code: outcome.exit_code,
                signal: outcome.signal,
                stdout: capture(outcome.stdout),
                stderr: capture(outcome.stderr),
                duration_ms: outcome.duration_ms,
                output_complete: outcome.output_complete,
            };
            broker
                .complete(
                    &observed_request.process_attempt_id,
                    &observed_request.request_sha256,
                    result,
                )
                .await
                .map_err(|_| Error::UncertainEffect)
        });
        if matches!(self.mode.load(Ordering::Acquire), 3..=5) {
            self.supervisors.lock().await.push(supervisor);
            tokio::time::timeout(Duration::from_secs(5), async {
                while !marker.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| Error::UncertainEffect)?;
            if self.mode.load(Ordering::Acquire) == 3 {
                return Err(Error::Provider(
                    "fixture_transport_error_with_live_child".into(),
                ));
            }
            if self.mode.load(Ordering::Acquire) == 5 {
                return std::future::pending().await;
            }
            return self
                .broker
                .receipt(&request.process_attempt_id)
                .await
                .map_err(|_| Error::UncertainEffect);
        }
        let receipt = supervisor.await.map_err(|_| Error::UncertainEffect)??;
        if self.mode.load(Ordering::Acquire) == 1 {
            return Err(Error::Provider("fixture_lost_result_ack".into()));
        }
        Ok(receipt)
    }
}
#[test]
fn trusted_runner_child_effect() {
    let Some(path) = std::env::var_os("COWORK_TEST_EFFECT_PATH") else {
        return;
    };
    use std::io::Write;
    let mut marker = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    marker.write_all(b"once").unwrap();
    marker.sync_all().unwrap();
    println!("original-bytes-once");
    if let Ok(wait) = std::env::var("COWORK_TEST_WAIT") {
        std::thread::sleep(Duration::from_secs(if wait == "hold" { 45 } else { 10 }));
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    runtime: Runtime,
    runner: Arc<FixtureRunner>,
    input: CreateRun,
    control_pkcs8: Vec<u8>,
}
async fn setup() -> Fixture {
    setup_artifact(false).await
}
async fn setup_artifact(kata: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = if let Ok(url) = std::env::var("COWORK_PROCESS_TARGET_TEST_DATABASE_URL") {
        assert!(
            url.rsplit('/')
                .next()
                .is_some_and(|name| name.starts_with("cowork_execution_target_runtime_")),
            "Use a new dedicated target/runtime qualification database"
        );
        Store::connect(&url).await.unwrap()
    } else {
        Store::sqlite(&dir.path().join("product.sqlite3"))
            .await
            .unwrap()
    };
    let project = store
        .create_project(CreateProject {
            name: "Signed process product fixture".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Execute once".into(),
        })
        .await
        .unwrap();
    let runner_key = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let control_pkcs8 = SigningKey::generate_pkcs8().unwrap();
    let control_key = SigningKey::from_pkcs8(&control_pkcs8).unwrap();
    let binding = Binding {
        runner_id: "signed-fixture-runner".into(),
        generation: 1,
        adapter_build_sha256: "b".repeat(64),
        control_public_key: control_key.public_key(),
        runner_public_key: runner_key.public_key(),
    };
    let mut definition = EnvironmentDefinition {
        format_version: 1,
        network: None,
        artifact: EnvironmentArtifact::LinuxImage {
            reference: format!("test.invalid/fixture@sha256:{}", "a".repeat(64)),
        },
        programs: BTreeMap::from([("fixture".into(), "bin/fixture".into())]),
        packages: vec![],
        limits: EnvironmentLimits {
            cpu_millis: 1000,
            memory_bytes: 256 * 1024 * 1024,
            processes: 32,
            scratch_bytes: 64 * 1024 * 1024,
        },
    };
    if kata {
        definition.artifact = EnvironmentArtifact::LinuxKataImage {
            reference: format!("test.invalid/fixture@sha256:{}", "a".repeat(64)),
        };
    }
    let environment = store
        .create_environment(&project.id, "Synthetic fixture qualification", "create")
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
        report_sha256: "c".repeat(64),
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
        runner_id: qualification.runner_id.clone(),
        runner_generation: 1,
        adapter_build_sha256: qualification.adapter_build_sha256.clone(),
        artifact_sha256: qualification.artifact_sha256.clone(),
        definition_sha256: qualification.definition_sha256.clone(),
        capabilities: qualification.capabilities.clone(),
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
    let profile = ProviderProfile {
        generation: Default::default(),
        id: "test".into(),
        name: "Unused test provider".into(),
        kind: ProviderKind::Test,
        endpoint: "".into(),
        model: "test".into(),
        revision: 1,
        context_tokens: 32768,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    let broker = cowork_broker::FileBroker::open(&dir.path().join("files"), vec![]).unwrap();
    let base = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        true,
    )
    .unwrap()
    .with_file_broker(broker);
    let child_dir = dir.path().join("owned-child");
    std::fs::create_dir(&child_dir).unwrap();
    sqlx::any::install_default_drivers();
    let runner_path = dir.path().join("runner.sqlite3");
    let url = format!(
        "sqlite:{}?mode=rwc",
        percent_encoding::utf8_percent_encode(
            runner_path.to_str().unwrap(),
            percent_encoding::NON_ALPHANUMERIC
        )
    );
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("PRAGMA synchronous=FULL")
        .execute(&pool)
        .await
        .unwrap();
    let runner = Arc::new(FixtureRunner {
        store: base.store.clone(),
        broker: Broker::open(pool, binding.clone(), runner_key.clone())
            .await
            .unwrap(),
        signer: runner_key,
        observation,
        directory: child_dir,
        mode: AtomicU8::new(0),
        launches: AtomicUsize::new(0),
        renewals: AtomicUsize::new(0),
        renewal_requests: tokio::sync::Mutex::new(vec![]),
        supervisors: tokio::sync::Mutex::new(vec![]),
    });
    let config = ProcessRunnerConfig::new(binding, control_key, runner.clone()).unwrap();
    let identity = config.execution_identity();
    base.store
        .register_execution_target(&identity, 0)
        .await
        .unwrap();
    base.store
        .set_execution_target_active(&identity, 1, true)
        .await
        .unwrap();
    let runtime = base.with_registered_process_runner(config).await.unwrap();
    let input = CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        environment: Some(pin.reference()),
        mcp_tools: vec![],
        budgets: Budgets::default(),
        order_id: order.id,
        base_revision: 1,
        idempotency_key: "once".into(),
        prompt: "Registered fixture".into(),
        provider_id: "test".into(),
        provider_revision: 1,
        access_mode: AccessMode::Autonomous,
        folders: vec![],
    };
    Fixture {
        _dir: dir,
        runtime,
        runner,
        input,
        control_pkcs8,
    }
}
async fn pending(f: &Fixture, wait: bool) -> (String, String, ToolCall) {
    let run = f.runtime.create_run(&f.input).await.unwrap();
    let claim = f.runtime.store.claim().await.unwrap().unwrap();
    assert_eq!(run.id, claim.record.id);
    let token = claim.lease.unwrap().token;
    let tool = ToolCall {
        id: "actual-fixture-call".into(),
        name: "execute_program".into(),
        arguments: json!({"program":"fixture","arguments":if wait {vec!["wait"]} else {vec![]},"timeout_seconds":15}),
    };
    f.runtime
        .store
        .mutate(&run.id, Some(&token), |r| {
            r.checkpoint.pending_tools = vec![tool.clone()];
            Ok(Change::event("fixture_model_response", json!({})))
        })
        .await
        .unwrap();
    (run.id, token, tool)
}

#[tokio::test]
async fn product_selection_freezes_environment_and_executes_real_child_once() {
    let f = setup().await;
    let flight = f.runtime.preflight(&f.input).await.unwrap();
    assert_eq!(flight.environment, f.input.environment);
    assert!(flight.allowed_tools.contains(&"execute_program".into()));
    let (run, token, tool) = pending(&f, false).await;
    f.runtime
        .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
        .await
        .unwrap();
    let records = f.runtime.store.process_attempts(&run).await.unwrap();
    assert_eq!(records.len(), 1);
    let ProcessAttemptState::Completed { result } = &records[0].state else {
        panic!("completed result expected")
    };
    assert_eq!(result.exit_code, Some(0));
    assert!(String::from_utf8_lossy(&result.stdout.bytes).contains("original-bytes-once"));
    assert_eq!(
        std::fs::read(f.runner.directory.join("effect.txt")).unwrap(),
        b"once"
    );
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 1);
    let stored = f.runtime.store.stored_run(&run).await.unwrap();
    assert!(stored.checkpoint.in_flight.is_none());
    assert!(stored.checkpoint.pending_tools.is_empty());
}

#[tokio::test]
async fn failed_preparation_settles_only_the_original_untransferred_attempt() {
    let f = setup().await;
    f.runner.mode.store(7, Ordering::Release);
    let (run, token, tool) = pending(&f, false).await;
    assert!(matches!(
        f.runtime
            .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
            .await,
        Err(Error::Provider(_))
    ));
    let attempts = f.runtime.store.process_attempts(&run).await.unwrap();
    assert_eq!(attempts.len(), 1);
    assert!(matches!(
        attempts[0].state,
        ProcessAttemptState::ReconciledNotStarted { .. }
    ));
    let stored = f.runtime.store.stored_run(&run).await.unwrap();
    assert!(stored.checkpoint.in_flight.is_none());
    assert!(stored.checkpoint.pending_tools.is_empty());
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 0);
    assert!(!f.runner.directory.join("effect.txt").exists());
    // Observation of the settled attempt never grants another launch capability.
    f.runner.mode.store(0, Ordering::Release);
    assert!(matches!(
        f.runtime
            .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
            .await,
        Err(Error::UncertainEffect)
    ));
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 0);
    assert_eq!(
        f.runtime.store.process_attempts(&run).await.unwrap(),
        attempts
    );
}

#[tokio::test]
async fn cancellation_during_preparation_keeps_a_proven_nonstart_canceled() {
    let f = setup().await;
    f.runner.mode.store(8, Ordering::Release);
    let (run, token, tool) = pending(&f, false).await;
    assert!(
        f.runtime
            .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
            .await
            .is_err()
    );
    let stored = f.runtime.store.stored_run(&run).await.unwrap();
    assert_eq!(stored.record.state, RunState::Canceled);
    assert!(stored.lease.is_none());
    assert!(stored.checkpoint.in_flight.is_none());
    assert!(stored.checkpoint.pending_tools.is_empty());
    let attempts = f.runtime.store.process_attempts(&run).await.unwrap();
    assert_eq!(attempts.len(), 1);
    assert!(matches!(
        attempts[0].state,
        ProcessAttemptState::ReconciledNotStarted { .. }
    ));
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 0);
    assert!(!f.runner.directory.join("effect.txt").exists());
}

#[tokio::test]
async fn lost_result_ack_retains_uncertainty_and_never_relaunches() {
    let f = setup().await;
    f.runner.mode.store(1, Ordering::Release);
    let (run, token, tool) = pending(&f, false).await;
    assert!(
        f.runtime
            .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
            .await
            .is_err()
    );
    let before = f.runtime.store.process_attempts(&run).await.unwrap();
    assert!(matches!(
        before[0].state,
        ProcessAttemptState::LaunchCommitted
    ));
    assert!(matches!(
        f.runtime
            .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
            .await,
        Err(Error::UncertainEffect)
    ));
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 1);
    assert_eq!(
        std::fs::read(f.runner.directory.join("effect.txt")).unwrap(),
        b"once"
    );
    let after = f.runtime.store.process_attempts(&run).await.unwrap();
    assert_eq!(before, after);
    assert_eq!(
        f.runtime
            .store
            .stored_run(&run)
            .await
            .unwrap()
            .checkpoint
            .in_flight,
        Some(tool)
    );
}

#[tokio::test]
async fn stale_or_revoked_environment_denies_product_start_without_launch() {
    let f = setup().await;
    f.runner.mode.store(2, Ordering::Release);
    assert!(matches!(
        f.runtime.create_run(&f.input).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 0);
    f.runner.mode.store(0, Ordering::Release);
    let reference = f.input.environment.as_ref().unwrap();
    let project = f
        .runtime
        .store
        .order(&f.input.order_id)
        .await
        .unwrap()
        .project_id;
    f.runtime
        .store
        .change_environment(
            &project,
            &reference.environment_id,
            4,
            "revoke",
            &EnvironmentChange::Revoke,
        )
        .await
        .unwrap();
    assert!(matches!(
        f.runtime.preflight(&f.input).await,
        Err(Error::Forbidden)
    ));
    assert!(!f.runner.directory.join("effect.txt").exists());
}

#[tokio::test]
async fn cancellation_waits_for_actual_owned_child_termination_before_settling() {
    let f = setup().await;
    let (run, token, tool) = pending(&f, true).await;
    let execution = {
        let runtime = f.runtime.clone();
        let run = run.clone();
        let token = token.clone();
        let tool = tool.clone();
        tokio::spawn(async move {
            runtime
                .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f.runner.directory.join("effect.txt").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    f.runtime.store.cancel(&run).await.unwrap();
    execution.await.unwrap().unwrap();
    assert_eq!(
        f.runtime.store.stored_run(&run).await.unwrap().record.state,
        RunState::Canceled
    );
    let records = f.runtime.store.process_attempts(&run).await.unwrap();
    let ProcessAttemptState::Completed { result } = &records[0].state else {
        panic!("termination receipt required")
    };
    assert_eq!(result.termination, ProcessTermination::Canceled);
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn registered_target_revocation_and_reactivation_do_not_upgrade_a_live_runtime() {
    let f = setup().await;
    let config = f.runtime.process_runner.as_ref().unwrap();
    let identity = config.execution_identity();
    assert_eq!(
        f.runtime
            .execution_target_binding()
            .await
            .unwrap()
            .unwrap()
            .1,
        identity
    );
    f.runtime.preflight(&f.input).await.unwrap();
    f.runtime
        .store
        .set_execution_target_active(&identity, 2, false)
        .await
        .unwrap();
    assert!(f.runtime.execution_target_binding().await.is_err());
    assert!(matches!(
        f.runtime.preflight(&f.input).await,
        Err(Error::Forbidden)
    ));
    f.runtime
        .store
        .set_execution_target_active(&identity, 3, true)
        .await
        .unwrap();
    assert!(matches!(
        f.runtime.preflight(&f.input).await,
        Err(Error::Forbidden)
    ));
    let current = ProcessRunnerConfig::new(
        config.binding.clone(),
        SigningKey::from_pkcs8(&f.control_pkcs8).unwrap(),
        f.runner.clone(),
    )
    .unwrap();
    let runtime = f
        .runtime
        .clone()
        .with_registered_process_runner(current)
        .await
        .unwrap();
    assert_eq!(
        runtime
            .execution_target_binding()
            .await
            .unwrap()
            .unwrap()
            .0
            .revision,
        4
    );
    runtime.preflight(&f.input).await.unwrap();
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn a_rebound_runtime_cannot_commit_a_plan_from_an_old_target_revision() {
    let f = setup().await;
    let plan = f
        .runtime
        .prepare_execution_plan(&cowork_workflow_contracts::RunRequest::Single(
            f.input.clone(),
        ))
        .await
        .unwrap();
    let config = f.runtime.process_runner.as_ref().unwrap();
    let identity = config.execution_identity();
    f.runtime
        .store
        .set_execution_target_active(&identity, 2, false)
        .await
        .unwrap();
    f.runtime
        .store
        .set_execution_target_active(&identity, 3, true)
        .await
        .unwrap();
    let config = ProcessRunnerConfig::new(
        config.binding.clone(),
        SigningKey::from_pkcs8(&f.control_pkcs8).unwrap(),
        f.runner.clone(),
    )
    .unwrap();
    let current = f
        .runtime
        .clone()
        .with_registered_process_runner(config)
        .await
        .unwrap();
    assert!(matches!(
        current
            .commit_execution_plan(
                &plan.plan.order_id,
                &plan.plan.id,
                &cowork_workflow_contracts::CommitExecutionPlan {
                    sha256: plan.sha256
                }
            )
            .await,
        Err(Error::ExecutionPlanChanged)
    ));
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn accepted_native_plan_keeps_its_original_receipt_after_target_revocation() {
    let f = setup().await;
    let plan = f
        .runtime
        .prepare_execution_plan(&cowork_workflow_contracts::RunRequest::Single(
            f.input.clone(),
        ))
        .await
        .unwrap();
    let confirm = cowork_workflow_contracts::CommitExecutionPlan {
        sha256: plan.sha256,
    };
    let original = f
        .runtime
        .commit_execution_plan(&plan.plan.order_id, &plan.plan.id, &confirm)
        .await
        .unwrap();
    let identity = f
        .runtime
        .process_runner
        .as_ref()
        .unwrap()
        .execution_identity();
    f.runtime
        .store
        .set_execution_target_active(&identity, 2, false)
        .await
        .unwrap();
    f.runtime.store.cancel(&original.id).await.unwrap();
    let replay = f
        .runtime
        .commit_execution_plan(&plan.plan.order_id, &plan.plan.id, &confirm)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(original).unwrap(),
        serde_json::to_value(replay).unwrap()
    );
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn concurrent_target_revoke_and_native_plan_commit_preserve_serial_authority() {
    for _ in 0..8 {
        let f = setup().await;
        let plan = f
            .runtime
            .prepare_execution_plan(&cowork_workflow_contracts::RunRequest::Single(
                f.input.clone(),
            ))
            .await
            .unwrap();
        let identity = f
            .runtime
            .process_runner
            .as_ref()
            .unwrap()
            .execution_identity();
        let confirm = cowork_workflow_contracts::CommitExecutionPlan {
            sha256: plan.sha256.clone(),
        };
        let (accepted, revoked) = tokio::join!(
            f.runtime
                .commit_execution_plan(&plan.plan.order_id, &plan.plan.id, &confirm),
            f.runtime
                .store
                .set_execution_target_active(&identity, 2, false)
        );
        assert_eq!(revoked.unwrap().state, ExecutionTargetState::Revoked);
        let saved = f
            .runtime
            .store
            .execution_plan(&plan.plan.order_id, &plan.plan.id)
            .await
            .unwrap();
        let original = f.runtime.store.execution_plan_run(&saved).await.unwrap();
        match accepted {
            Ok(run) => assert_eq!(
                serde_json::to_value(original.unwrap()).unwrap(),
                serde_json::to_value(run).unwrap()
            ),
            Err(Error::Forbidden | Error::ExecutionPlanChanged) => assert!(original.is_none()),
            Err(error) => panic!("Unexpected target/admission race result: {error}"),
        }
        assert!(f.runtime.preflight(&f.input).await.is_err());
        assert_eq!(f.runner.launches.load(Ordering::Acquire), 0);
    }
}

#[tokio::test]
async fn an_unbound_config_cannot_bypass_the_registered_target_gate() {
    let f = setup().await;
    let binding = f.runtime.process_runner.as_ref().unwrap().binding.clone();
    let config = ProcessRunnerConfig::new(
        binding,
        SigningKey::from_pkcs8(&f.control_pkcs8).unwrap(),
        f.runner.clone(),
    )
    .unwrap();
    let runtime = f.runtime.with_process_runner(config);
    assert!(matches!(
        runtime.preflight(&f.input).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn configured_inactive_target_keeps_control_plane_available_without_execution_authority() {
    let f = setup().await;
    let original = f.runtime.process_runner.as_ref().unwrap();
    let identity = original.execution_identity();
    f.runtime
        .store
        .set_execution_target_active(&identity, 2, false)
        .await
        .unwrap();
    let config = ProcessRunnerConfig::new(
        original.binding.clone(),
        SigningKey::from_pkcs8(&f.control_pkcs8).unwrap(),
        f.runner.clone(),
    )
    .unwrap();
    let rebound = f
        .runtime
        .clone()
        .with_configured_process_runner(config)
        .await
        .unwrap();
    assert!(rebound.execution_target_binding().await.unwrap().is_none());
    assert!(
        rebound
            .process_runner
            .as_ref()
            .unwrap()
            .registered_target()
            .is_none()
    );
    assert!(rebound.store.order(&f.input.order_id).await.is_ok());
    assert!(matches!(
        rebound.preflight(&f.input).await,
        Err(Error::Forbidden)
    ));
    f.runtime
        .store
        .set_execution_target_active(&identity, 3, true)
        .await
        .unwrap();
    assert!(matches!(
        rebound.preflight(&f.input).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn target_revocation_fences_and_reaps_the_actual_running_child() {
    let f = setup().await;
    let (run, token, tool) = pending(&f, true).await;
    let execution = {
        let runtime = f.runtime.clone();
        let run = run.clone();
        let token = token.clone();
        let tool = tool.clone();
        tokio::spawn(async move {
            runtime
                .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f.runner.directory.join("effect.txt").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let identity = f
        .runtime
        .process_runner
        .as_ref()
        .unwrap()
        .execution_identity();
    f.runtime
        .store
        .set_execution_target_active(&identity, 2, false)
        .await
        .unwrap();
    let _outcome = tokio::time::timeout(Duration::from_secs(10), execution)
        .await
        .unwrap()
        .unwrap();
    let records = f.runtime.store.process_attempts(&run).await.unwrap();
    let ProcessAttemptState::Completed { result } = &records[0].state else {
        panic!("physical termination receipt required after target revocation")
    };
    assert_eq!(result.termination, ProcessTermination::Canceled);
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 1);
    assert!(
        f.runtime
            .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn transport_errors_and_incomplete_receipts_cancel_real_child_and_retain_uncertainty() {
    for mode in [3, 4] {
        let f = setup().await;
        f.runner.mode.store(mode, Ordering::Release);
        let (run, token, tool) = pending(&f, true).await;
        assert!(matches!(
            f.runtime
                .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
                .await,
            Err(Error::UncertainEffect)
        ));
        let supervisor = f.runner.supervisors.lock().await.pop().unwrap();
        let receipt = tokio::time::timeout(Duration::from_secs(5), supervisor)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let ExecutionState::Completed { result } = &receipt.payload.state else {
            panic!("physical receipt required");
        };
        assert_eq!(result.termination, ProcessTermination::Canceled);
        assert!(
            !f.runner
                .broker
                .execution_allowed(&receipt.payload.request)
                .await
                .unwrap()
        );
        let attempts = f.runtime.store.process_attempts(&run).await.unwrap();
        assert!(matches!(
            attempts[0].state,
            ProcessAttemptState::LaunchCommitted
        ));
        assert_eq!(
            f.runtime
                .store
                .stored_run(&run)
                .await
                .unwrap()
                .checkpoint
                .in_flight,
            Some(tool)
        );
        assert_eq!(f.runner.launches.load(Ordering::Acquire), 1);
        assert_eq!(
            std::fs::read(f.runner.directory.join("effect.txt")).unwrap(),
            b"once"
        );
    }
}

#[tokio::test]
async fn long_process_renews_lease_without_extending_original_run_deadline() {
    let f = setup().await;
    let (run, token, tool) = pending(&f, true).await;
    f.runtime
        .store
        .mutate(&run, Some(&token), |stored| {
            stored.lease.as_mut().unwrap().until = now() + 5_000;
            Ok(Change::default())
        })
        .await
        .unwrap();
    let before = f.runtime.store.stored_run(&run).await.unwrap();
    f.runtime
        .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
        .await
        .unwrap();
    let after = f.runtime.store.stored_run(&run).await.unwrap();
    assert!(after.lease.as_ref().unwrap().until > before.lease.as_ref().unwrap().until);
    assert_eq!(
        canonical_hash(&after.record.spec),
        canonical_hash(&before.record.spec)
    );
    assert_eq!(after.record.started_at, before.record.started_at);
    let attempts = f.runtime.store.process_attempts(&run).await.unwrap();
    let ProcessAttemptState::Completed { result } = &attempts[0].state else {
        panic!("physical receipt required");
    };
    assert_eq!(result.termination, ProcessTermination::Exited);
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 1);
    assert!(f.runner.renewals.load(Ordering::Acquire) >= 2);
}

#[tokio::test]
async fn dropping_product_request_cancels_independently_owned_actual_child() {
    let f = setup().await;
    f.runner.mode.store(5, Ordering::Release);
    let (run, token, tool) = pending(&f, true).await;
    let execution = {
        let runtime = f.runtime.clone();
        let run = run.clone();
        let tool = tool.clone();
        tokio::spawn(async move {
            runtime
                .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f.runner.directory.join("effect.txt").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    execution.abort();
    assert!(execution.await.unwrap_err().is_cancelled());
    let supervisor = f.runner.supervisors.lock().await.pop().unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(5), supervisor)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ExecutionState::Completed { result } = receipt.payload.state else {
        panic!("physical receipt required");
    };
    assert_eq!(result.termination, ProcessTermination::Canceled);
    let attempts = f.runtime.store.process_attempts(&run).await.unwrap();
    assert!(matches!(
        attempts[0].state,
        ProcessAttemptState::LaunchCommitted
    ));
    assert_eq!(
        f.runtime
            .store
            .stored_run(&run)
            .await
            .unwrap()
            .checkpoint
            .in_flight,
        Some(tool)
    );
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn failed_signed_lease_renewal_stops_actual_child_before_original_deadline() {
    let f = setup().await;
    f.runner.mode.store(6, Ordering::Release);
    let (run, token, tool) = pending(&f, true).await;
    f.runtime
        .store
        .mutate(&run, Some(&token), |stored| {
            stored.lease.as_mut().unwrap().until = now() + 5_000;
            Ok(Change::default())
        })
        .await
        .unwrap();
    let before = f.runtime.store.stored_run(&run).await.unwrap();
    f.runtime
        .invoke_process_tool(&run, &token, &tool, &CancellationToken::new())
        .await
        .unwrap();
    let attempts = f.runtime.store.process_attempts(&run).await.unwrap();
    let ProcessAttemptState::Completed { result } = &attempts[0].state else {
        panic!("actual termination receipt required");
    };
    assert_eq!(result.termination, ProcessTermination::Canceled);
    assert!(result.process_id.is_some());
    assert!(result.duration_ms < 10_000);
    assert_eq!(f.runner.renewals.load(Ordering::Acquire), 0);
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 1);
    let after = f.runtime.store.stored_run(&run).await.unwrap();
    assert_eq!(
        canonical_hash(&before.record.spec),
        canonical_hash(&after.record.spec)
    );
    assert_eq!(before.record.started_at, after.record.started_at);
    assert!(after.checkpoint.in_flight.is_none());
}

#[test]
fn absent_environment_keeps_legacy_serialization_and_catalog() {
    let input:CreateRun=serde_json::from_value(json!({"order_id":"o","base_revision":1,"idempotency_key":"k","prompt":"p","provider_id":"m","provider_revision":1})).unwrap();
    assert!(input.environment.is_none());
    assert!(
        serde_json::to_value(&input)
            .unwrap()
            .get("environment")
            .is_none()
    );
}
