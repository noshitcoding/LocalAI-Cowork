use cowork_contracts::*;
use cowork_domain::{Error, canonical_hash};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::{Store, library_config::LibraryOwner};
use cowork_workflow_contracts::{CommitExecutionPlan, ExecutionProcessAccess, RunRequest};

async fn fixture() -> (tempfile::TempDir, Runtime, CreateRun) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("plans.sqlite"))
        .await
        .unwrap();
    let (runtime, request) = fixture_store(store).await;
    (dir, runtime, request)
}

async fn fixture_store(store: Store) -> (Runtime, CreateRun) {
    let project = store
        .create_project(CreateProject {
            name: "Execution review".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "Reviewed task".into(),
        })
        .await
        .unwrap();
    let profile = ProviderProfile {
        id: "fixture".into(),
        name: "Fixture".into(),
        kind: ProviderKind::Test,
        endpoint: "".into(),
        model: "fixture".into(),
        revision: 1,
        context_tokens: 32768,
        credential_required: false,
        credential_ready: true,
        generation: Default::default(),
        capabilities: Default::default(),
    };
    store.insert_provider(&profile).await.unwrap();
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        true,
    )
    .unwrap();
    let request = CreateRun {
        order_id: order.id,
        base_revision: order.revision,
        idempotency_key: "reviewed-start".into(),
        prompt: "Produce a result".into(),
        provider_id: "fixture".into(),
        provider_revision: 1,
        access_mode: AccessMode::ConfirmMutations,
        budgets: Budgets::default(),
        folders: vec![],
        native_tools: vec![],
        mcp_tools: vec![],
        environment: None,
        output: None,
        model_binding: None,
        agent_config: None,
    };
    (runtime, request)
}

#[tokio::test]
#[ignore = "Requires a fresh private cowork_execution_plan_<uuid> database"]
async fn postgres_execution_plans_serialize_confirmation_and_retirement() {
    let url = std::env::var("COWORK_TEST_EXECUTION_PLAN_POSTGRES").expect("private PostgreSQL URL");
    sqlx::any::install_default_drivers();
    let pool = sqlx::AnyPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database.strip_prefix("cowork_execution_plan_").is_some_and(
            |suffix| suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        )
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pg_tables WHERE schemaname='public'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    pool.close().await;
    let (runtime, request) = fixture_store(Store::connect(&url).await.unwrap()).await;
    let input = RunRequest::Single(request.clone());
    let prepared = runtime.prepare_execution_plan(&input).await.unwrap();
    let commit = CommitExecutionPlan {
        sha256: prepared.sha256.clone(),
    };
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let (runtime, request, prepared, commit) = (
            runtime.clone(),
            request.clone(),
            prepared.clone(),
            commit.clone(),
        );
        tasks.spawn(async move {
            runtime
                .commit_execution_plan(&request.order_id, &prepared.plan.id, &commit)
                .await
        });
    }
    let mut original = None;
    while let Some(result) = tasks.join_next().await {
        let run = result.unwrap().unwrap();
        let value = serde_json::to_value(&run).unwrap();
        if let Some(ref original) = original {
            assert_eq!(&value, original);
        } else {
            original = Some(value);
        }
    }
    let original = original.unwrap();
    assert_eq!(
        runtime.store.runs(&request.order_id).await.unwrap().len(),
        1
    );
    runtime
        .store
        .cancel(original["id"].as_str().unwrap())
        .await
        .unwrap();
    let project = prepared.plan.project_id.clone();
    for index in 0..12 {
        let order = runtime
            .store
            .create_order(CreateOrder {
                project_id: project.clone(),
                title: format!("Retirement race {index}"),
            })
            .await
            .unwrap();
        let next = CreateRun {
            order_id: order.id.clone(),
            base_revision: order.revision,
            idempotency_key: format!("race-{index}"),
            ..request.clone()
        };
        let input = RunRequest::Single(next);
        let plan = runtime.prepare_execution_plan(&input).await.unwrap();
        let confirm = CommitExecutionPlan {
            sha256: plan.sha256,
        };
        let (accepted, retired) = tokio::join!(
            runtime.commit_execution_plan(&order.id, &plan.plan.id, &confirm),
            runtime.store.retire_execution_request(&input),
        );
        match (accepted, retired) {
            (Ok(run), Err(Error::Conflict)) => {
                assert_eq!(runtime.store.runs(&order.id).await.unwrap().len(), 1);
                runtime.store.cancel(&run.id).await.unwrap();
            }
            (Err(Error::ExecutionPlanChanged), Ok(receipt)) => {
                assert!(receipt.retired);
                assert!(runtime.store.runs(&order.id).await.unwrap().is_empty());
                assert!(
                    runtime
                        .commit_execution_plan(&order.id, &plan.plan.id, &confirm)
                        .await
                        .is_err()
                );
            }
            (accepted, retired) => panic!(
                "Confirmation/retirement diverged: accepted={}, retired={}",
                accepted.is_ok(),
                retired.is_ok()
            ),
        }
    }
    runtime.providers.write().await.clear();
    assert_eq!(
        serde_json::to_value(
            runtime
                .commit_execution_plan(&request.order_id, &prepared.plan.id, &commit)
                .await
                .unwrap()
        )
        .unwrap(),
        original
    );
    runtime.store.close().await;
}

#[tokio::test]
async fn prepare_is_durable_without_creating_a_run_and_binds_original_request() {
    let (_dir, runtime, request) = fixture().await;
    let input = RunRequest::Single(request.clone());
    let prepared = runtime.prepare_execution_plan(&input).await.unwrap();
    assert_eq!(prepared.sha256, canonical_hash(&prepared.plan));
    assert_eq!(prepared.plan.request_sha256, canonical_hash(&input));
    assert_eq!(
        prepared.plan.boundary.process_access,
        ExecutionProcessAccess::BrokerOnly
    );
    assert!(!prepared.plan.boundary.input_snapshot_bound);
    assert!(
        runtime
            .store
            .runs(&request.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    let replay = runtime.prepare_execution_plan(&input).await.unwrap();
    assert_eq!(replay.plan.id, prepared.plan.id);
    assert_eq!(replay.sha256, prepared.sha256);
    let mut changed = request;
    changed.prompt = "A substituted request".into();
    assert!(matches!(
        runtime
            .prepare_execution_plan(&RunRequest::Single(changed))
            .await,
        Err(Error::Conflict)
    ));
}

#[tokio::test]
async fn wrong_fingerprint_and_changed_settings_cannot_start() {
    let (_dir, runtime, request) = fixture().await;
    let prepared = runtime
        .prepare_execution_plan(&RunRequest::Single(request.clone()))
        .await
        .unwrap();
    let wrong = CommitExecutionPlan {
        sha256: "0".repeat(64),
    };
    assert!(matches!(
        runtime
            .commit_execution_plan(&request.order_id, &prepared.plan.id, &wrong)
            .await,
        Err(Error::ExecutionPlanChanged)
    ));
    let prefs = runtime
        .store
        .library_preferences(LibraryOwner::Local)
        .await
        .unwrap();
    let mut value = prefs.value;
    value.system_prompt = "Different authoritative instructions".into();
    runtime
        .store
        .library_write_preferences(
            LibraryOwner::Local,
            &WritePreferences {
                idempotency_key: "change-after-review".into(),
                expected_revision: prefs.revision,
                value,
            },
        )
        .await
        .unwrap();
    let commit = CommitExecutionPlan {
        sha256: prepared.sha256,
    };
    assert!(matches!(
        runtime
            .commit_execution_plan(&request.order_id, &prepared.plan.id, &commit)
            .await,
        Err(Error::ExecutionPlanChanged)
    ));
    assert!(
        runtime
            .store
            .runs(&request.order_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn concurrent_commits_create_one_run_and_replay_the_exact_initial_ack() {
    let (_dir, runtime, request) = fixture().await;
    let prepared = runtime
        .prepare_execution_plan(&RunRequest::Single(request.clone()))
        .await
        .unwrap();
    let commit = CommitExecutionPlan {
        sha256: prepared.sha256.clone(),
    };
    let (first, second) = tokio::join!(
        runtime.commit_execution_plan(&request.order_id, &prepared.plan.id, &commit),
        runtime.commit_execution_plan(&request.order_id, &prepared.plan.id, &commit),
    );
    let first = first.unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(second.unwrap()).unwrap()
    );
    assert_eq!(
        runtime.store.runs(&request.order_id).await.unwrap().len(),
        1
    );
    runtime.store.cancel(&first.id).await.unwrap();
    runtime.providers.write().await.clear();
    let replay = runtime
        .commit_execution_plan(&request.order_id, &prepared.plan.id, &commit)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&replay).unwrap(),
        serde_json::to_value(&first).unwrap()
    );
    assert_eq!(replay.state, RunState::Queued);
    assert_ne!(
        runtime
            .store
            .stored_run(&first.id)
            .await
            .unwrap()
            .record
            .state,
        RunState::Queued
    );
}

#[tokio::test]
async fn preparation_and_original_receipt_survive_database_reopen() {
    let (dir, runtime, request) = fixture().await;
    let input = RunRequest::Single(request.clone());
    let prepared = runtime.prepare_execution_plan(&input).await.unwrap();
    let commit = CommitExecutionPlan {
        sha256: prepared.sha256.clone(),
    };
    let accepted = runtime
        .commit_execution_plan(&request.order_id, &prepared.plan.id, &commit)
        .await
        .unwrap();
    runtime.store.close().await;
    let store = Store::sqlite(&dir.path().join("plans.sqlite"))
        .await
        .unwrap();
    let reopened = Runtime::new(store, vec![], true).unwrap();
    let view = reopened.prepare_execution_plan(&input).await.unwrap();
    assert_eq!(view.committed_run_id.as_deref(), Some(accepted.id.as_str()));
    assert_eq!(view.sha256, prepared.sha256);
    let replay = reopened
        .commit_execution_plan(&request.order_id, &prepared.plan.id, &commit)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(accepted).unwrap()
    );
}

#[tokio::test]
async fn another_order_cannot_inspect_or_commit_a_plan() {
    let (_dir, runtime, request) = fixture().await;
    let prepared = runtime
        .prepare_execution_plan(&RunRequest::Single(request))
        .await
        .unwrap();
    let commit = CommitExecutionPlan {
        sha256: prepared.sha256,
    };
    assert!(matches!(
        runtime
            .inspect_execution_plan("another-order", &prepared.plan.id)
            .await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        runtime
            .commit_execution_plan("another-order", &prepared.plan.id, &commit)
            .await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn receipt_failure_rolls_back_run_order_and_plan_together() {
    let (dir, runtime, request) = fixture().await;
    let prepared = runtime
        .prepare_execution_plan(&RunRequest::Single(request.clone()))
        .await
        .unwrap();
    let database = dir.path().join("plans.sqlite");
    let encoded = percent_encoding::utf8_percent_encode(
        database.to_str().unwrap(),
        percent_encoding::NON_ALPHANUMERIC,
    );
    let db = sqlx::AnyPool::connect(&format!("sqlite:{encoded}?mode=rw"))
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_plan_receipt BEFORE UPDATE OF run_id ON owner_execution_plans BEGIN SELECT RAISE(ABORT, 'fixture receipt failure'); END")
        .execute(&db).await.unwrap();
    let commit = CommitExecutionPlan {
        sha256: prepared.sha256.clone(),
    };
    assert!(matches!(
        runtime
            .commit_execution_plan(&request.order_id, &prepared.plan.id, &commit)
            .await,
        Err(Error::Storage(_))
    ));
    assert!(
        runtime
            .store
            .runs(&request.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        runtime
            .store
            .order(&request.order_id)
            .await
            .unwrap()
            .revision,
        request.base_revision
    );
    assert!(
        runtime
            .inspect_execution_plan(&request.order_id, &prepared.plan.id)
            .await
            .unwrap()
            .committed_run_id
            .is_none()
    );
    sqlx::query("DROP TRIGGER fail_plan_receipt")
        .execute(&db)
        .await
        .unwrap();
    runtime
        .commit_execution_plan(&request.order_id, &prepared.plan.id, &commit)
        .await
        .unwrap();
    assert_eq!(
        runtime.store.runs(&request.order_id).await.unwrap().len(),
        1
    );
    db.close().await;
}

#[tokio::test]
async fn retiring_a_plan_fences_late_commits_without_losing_the_draft() {
    let (_dir, runtime, request) = fixture().await;
    let prepared = runtime
        .prepare_execution_plan(&RunRequest::Single(request.clone()))
        .await
        .unwrap();
    let commit = CommitExecutionPlan {
        sha256: prepared.sha256.clone(),
    };
    let before = runtime.store.order(&request.order_id).await.unwrap();
    let retired = runtime
        .retire_execution_plan(&request.order_id, &prepared.plan.id, &commit)
        .await
        .unwrap();
    assert!(retired.retired);
    assert!(matches!(
        runtime
            .commit_execution_plan(&request.order_id, &prepared.plan.id, &commit)
            .await,
        Err(Error::ExecutionPlanChanged)
    ));
    assert!(
        runtime
            .store
            .runs(&request.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        runtime
            .store
            .order(&request.order_id)
            .await
            .unwrap()
            .revision,
        before.revision
    );
}

#[tokio::test]
async fn an_accepted_plan_cannot_be_retired_as_if_it_had_never_run() {
    let (_dir, runtime, request) = fixture().await;
    let prepared = runtime
        .prepare_execution_plan(&RunRequest::Single(request.clone()))
        .await
        .unwrap();
    let commit = CommitExecutionPlan {
        sha256: prepared.sha256.clone(),
    };
    let accepted = runtime
        .commit_execution_plan(&request.order_id, &prepared.plan.id, &commit)
        .await
        .unwrap();
    assert!(matches!(
        runtime
            .retire_execution_plan(&request.order_id, &prepared.plan.id, &commit)
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        runtime
            .commit_execution_plan(&request.order_id, &prepared.plan.id, &commit)
            .await
            .unwrap()
            .id,
        accepted.id
    );
}

#[tokio::test]
async fn expired_preparation_cannot_create_a_run() {
    let (_dir, runtime, request) = fixture().await;
    let input = RunRequest::Single(request.clone());
    let view = runtime.prepare_execution_plan(&input).await.unwrap();
    let mut saved = runtime
        .store
        .execution_plan(&request.order_id, &view.plan.id)
        .await
        .unwrap();
    saved.plan.id = cowork_storage::id();
    saved.plan.idempotency_key = "expired-plan".into();
    let RunRequest::Single(original) = &mut saved.request else {
        unreachable!()
    };
    original.idempotency_key = saved.plan.idempotency_key.clone();
    saved.plan.request_sha256 = canonical_hash(&saved.request);
    saved.plan.created_at -= 300_001;
    saved.plan.expires_at -= 300_001;
    saved.sha256 = canonical_hash(&saved.plan);
    runtime.store.retain_execution_plan(&saved).await.unwrap();
    assert!(matches!(
        runtime
            .commit_execution_plan(
                &request.order_id,
                &saved.plan.id,
                &CommitExecutionPlan {
                    sha256: saved.sha256
                }
            )
            .await,
        Err(Error::ExecutionPlanExpired)
    ));
    assert!(
        runtime
            .store
            .runs(&request.order_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn workflow_plan_uses_the_same_atomic_acceptance_and_frozen_binding() {
    use cowork_domain::workflows::*;
    use cowork_workflow_contracts::{WorkflowRunKind, WorkflowRunRequest};
    let (_dir, runtime, request) = fixture().await;
    let project = runtime
        .store
        .order(&request.order_id)
        .await
        .unwrap()
        .project_id;
    let rights = WorkflowRights {
        current_workspace: false,
        tools: ["read_artifact".to_owned()].into_iter().collect(),
        folders: Default::default(),
    };
    let budgets = Budgets::default();
    let definition = WorkflowDefinition {
        documents: Default::default(),
        iterations: Default::default(),
        subflows: Default::default(),
        controls: Default::default(),
        gates: Default::default(),
        dataflow: None,
        format_version: 1,
        name: "Reviewed workflow".into(),
        mode: WorkflowMode::Sequential,
        failure: WorkflowFailure::Stop,
        rights: rights.clone(),
        budgets: budgets.clone(),
        roles: vec![WorkflowRole {
            id: "reader".into(),
            instructions: "Read selected evidence".into(),
            provider_id: "fixture".into(),
            provider_revision: 1,
            rights: rights.clone(),
            budgets: budgets.clone(),
            delegate_to: Default::default(),
            agent_config: None,
            model_binding: None,
            output: None,
        }],
        steps: vec![WorkflowStep {
            id: "read".into(),
            role_id: "reader".into(),
            goal: "Inspect evidence".into(),
            dependencies: vec![],
            rights,
            budgets: budgets.clone(),
            execution: None,
            zero_context: false,
        }],
    };
    let library = runtime
        .store
        .create_workflow(&project, "reviewed-workflow", &definition)
        .await
        .unwrap();
    let input = RunRequest::Workflow(WorkflowRunRequest {
        input_source: None,
        inputs: None,
        kind: WorkflowRunKind::Workflow,
        order_id: request.order_id.clone(),
        base_revision: request.base_revision,
        idempotency_key: "workflow-plan".into(),
        prompt: request.prompt,
        workflow_id: library.id,
        version_id: library.versions[0].id.clone(),
        access_mode: AccessMode::ConfirmMutations,
        budgets,
        native_tools: vec![],
        mcp_tools: vec![],
        folders: vec![],
        environment: None,
        expected_binding: None,
        agent_config: None,
    });
    let prepared = runtime.prepare_execution_plan(&input).await.unwrap();
    assert!(
        runtime
            .store
            .runs(&request.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    let run = runtime
        .commit_execution_plan(
            &request.order_id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap();
    let cowork_workflow_contracts::RunPreflight::Workflow(flight) = prepared.plan.preflight else {
        unreachable!()
    };
    assert_eq!(run.spec.workflow, Some(flight.workflow));
    assert!(run.spec.provider.is_none());
}

#[tokio::test]
async fn retiring_an_unacknowledged_preparation_prevents_a_late_prepare() {
    let (_dir, runtime, request) = fixture().await;
    let input = RunRequest::Single(request.clone());
    let receipt = runtime
        .store
        .retire_execution_request(&input)
        .await
        .unwrap();
    assert!(receipt.retired);
    assert!(matches!(
        runtime.prepare_execution_plan(&input).await,
        Err(Error::Conflict)
    ));
    assert!(
        runtime
            .store
            .runs(&request.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        runtime
            .store
            .retire_execution_request(&input)
            .await
            .unwrap()
            .retired
    );
}

#[tokio::test]
async fn retiring_by_original_request_also_fences_an_already_prepared_plan() {
    let (_dir, runtime, request) = fixture().await;
    let input = RunRequest::Single(request.clone());
    let prepared = runtime.prepare_execution_plan(&input).await.unwrap();
    runtime
        .store
        .retire_execution_request(&input)
        .await
        .unwrap();
    assert!(matches!(
        runtime
            .commit_execution_plan(
                &request.order_id,
                &prepared.plan.id,
                &CommitExecutionPlan {
                    sha256: prepared.sha256
                }
            )
            .await,
        Err(Error::ExecutionPlanChanged)
    ));
}
