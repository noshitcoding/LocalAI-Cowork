#[path = "workflows/input_cases.rs"]
mod input_cases;
#[path = "workflows/input_draft_cases.rs"]
mod input_draft_cases;
#[path = "workflows/journal_cases.rs"]
mod journal_cases;
#[path = "workflows/legacy_fixture.rs"]
mod legacy_fixture;
#[path = "workflows/pipeline_cases.rs"]
mod pipeline_cases;
#[path = "workflows/product_cases.rs"]
mod product_cases;
#[path = "workflows/resume_cases.rs"]
mod resume_cases;
#[path = "workflows/tool_cases.rs"]
mod tool_cases;
use cowork_contracts::*;
use cowork_domain::{Error, hash, workflows::*};
use cowork_storage::{
    Change, CreateWorkflowRun, Store, WorkflowCommand, WorkflowCommandResult, id, now,
};
use serde_json::json;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use std::collections::BTreeSet;

struct Fixture {
    request: CreateWorkflowRun,
    spec: RunSpec,
    definition: WorkflowDefinition,
}
fn budgets() -> Budgets {
    Budgets {
        max_steps: 8,
        max_seconds: 60,
        max_output_tokens: 256,
        max_total_tokens: 4096,
    }
}
async fn setup(store: &Store) -> Fixture {
    let project = store
        .create_project(CreateProject {
            name: "Workflow persistence fixture".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Parallel result".into(),
        })
        .await
        .unwrap();
    let provider_id = id();
    store
        .insert_provider(&ProviderProfile {
            id: provider_id.clone(),
            name: "Synthetic model".into(),
            kind: ProviderKind::Test,
            endpoint: String::new(),
            model: "explicit-fixture".into(),
            revision: 1,
            context_tokens: 2048,
            credential_required: false,
            credential_ready: true,
            generation: Default::default(),
            capabilities: Default::default(),
        })
        .await
        .unwrap();
    let rights = WorkflowRights {
        current_workspace: false,
        tools: BTreeSet::from(["fixture_effect".into()]),
        folders: BTreeSet::new(),
    };
    let definition = WorkflowDefinition {
        documents: Default::default(),
        iterations: Default::default(),
        subflows: Default::default(),
        controls: Default::default(),
        gates: Default::default(),
        dataflow: None,
        format_version: 1,
        name: "Produce and verify".into(),
        mode: WorkflowMode::Parallel { max_parallel: 2 },
        failure: WorkflowFailure::ContinueIndependent,
        rights: rights.clone(),
        budgets: budgets(),
        roles: vec![WorkflowRole {
            agent_config: None,
            model_binding: None,
            output: None,
            id: "author".into(),
            instructions: "Create the result".into(),
            provider_id,
            provider_revision: 1,
            rights: rights.clone(),
            budgets: budgets(),
            delegate_to: BTreeSet::new(),
        }],
        steps: [
            ("a", vec![]),
            ("b", vec![]),
            ("c", vec!["a".into(), "b".into()]),
        ]
        .into_iter()
        .map(|(id, dependencies)| WorkflowStep {
            execution: None,
            zero_context: false,
            id: id.into(),
            role_id: "author".into(),
            goal: format!("Result for {id}"),
            dependencies,
            rights: rights.clone(),
            budgets: budgets(),
        })
        .collect(),
    };
    let library = store
        .create_workflow(&project.id, "create", &definition)
        .await
        .unwrap();
    let request = CreateWorkflowRun {
        order_id: order.id.clone(),
        base_revision: order.revision,
        idempotency_key: "start".into(),
        workflow_id: library.id,
        version_id: library.versions[0].id.clone(),
        max_parallel: 2,
    };
    let spec = RunSpec {
        input_workspace: None,
        agent_config: None,
        model_binding: None,
        output: None,
        workflow: None,
        mcp_tools: vec![],
        environment: None,
        order_id: order.id,
        project_id: project.id,
        prompt: "Complete the requested result".into(),
        provider: None,
        file_action: None,
        access_mode: AccessMode::ConfirmMutations,
        executor: "local-v2".into(),
        policy_revision: 1,
        allowed_tools: vec!["fixture_effect".into()],
        budgets: budgets(),
        folders: vec![],
    };
    Fixture {
        request,
        spec,
        definition,
    }
}
async fn raw(path: &std::path::Path) -> SqlitePool {
    SqlitePool::connect_with(
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
    .await
    .unwrap()
}
async fn claim_node(
    store: &Store,
    f: &Fixture,
    run: &RunRecord,
    lease: &str,
    node: &str,
) -> WorkflowClaim {
    let before = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let WorkflowCommandResult::Claimed(claim) = store
        .workflow_command(
            &f.spec.project_id,
            &run.id,
            lease,
            before.progress.revision(),
            WorkflowCommand::Claim {
                node_id: node.into(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("claim")
    };
    claim
}
async fn admit(
    store: &Store,
    f: &Fixture,
    run: &RunRecord,
    lease: &str,
    claim: &WorkflowClaim,
    attempt: &str,
    operation: WorkflowOperation,
) -> WorkflowCommandResult {
    let before = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    store
        .workflow_command(
            &f.spec.project_id,
            &run.id,
            lease,
            before.progress.revision(),
            WorkflowCommand::Admit {
                claim: claim.clone(),
                id: attempt.into(),
                operation,
            },
        )
        .await
        .unwrap()
}
fn model() -> WorkflowOperation {
    WorkflowOperation::Model {
        reserved_tokens: None,
        request_sha256: "a".repeat(64),
    }
}
async fn contract(store: &Store) {
    Box::pin(worker_queue_contract(store)).await;
    Box::pin(lifecycle_contract(store)).await;
    Box::pin(concurrent_admission_contract(store, store)).await;
    Box::pin(concurrent_registry_contract(store)).await;
    Box::pin(model_journal_contract(store)).await;
    Box::pin(tool_cases::contract(store)).await;
    Box::pin(product_cases::contract(store)).await;
}
async fn lifecycle_contract(store: &Store) {
    let f = setup(store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    assert!(run.spec.provider.is_none());
    assert!(run.spec.workflow.is_some());
    assert_eq!(run.last_sequence, 2);
    assert!(
        store.claim().await.unwrap().is_none(),
        "ordinary single-agent worker must not accept workflow"
    );
    assert_eq!(
        store
            .create_workflow_run(&f.request, f.spec.clone())
            .await
            .unwrap()
            .id,
        run.id
    );
    let mut changed = f.spec.clone();
    changed.access_mode = AccessMode::ReadOnly;
    assert!(matches!(
        store.create_workflow_run(&f.request, changed).await,
        Err(Error::Conflict)
    ));
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = &parent.lease.as_ref().unwrap().token;
    let a = claim_node(store, &f, &run, lease, "a").await;
    let b = claim_node(store, &f, &run, lease, "b").await;
    assert!(matches!(
        admit(store, &f, &run, lease, &a, "m1", model()).await,
        WorkflowCommandResult::Admission(WorkflowAdmission::Dispatch(_))
    ));
    assert!(matches!(
        admit(store, &f, &run, lease, &b, "m2", model()).await,
        WorkflowCommandResult::Admission(WorkflowAdmission::Dispatch(_))
    ));
    let detail = store.detail(&run.id).await.unwrap();
    assert_eq!(detail.token_budget.charged_tokens, 4096);
    assert_eq!(detail.run.steps, 2);
    assert_eq!(detail.run.model_attempts.len(), 2);
    let state = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert!(matches!(
        store
            .workflow_command(
                &f.spec.project_id,
                &run.id,
                "old-lease",
                state.progress.revision(),
                WorkflowCommand::Settle {
                    claim: a.clone(),
                    id: "m1".into(),
                    outcome: WorkflowOutcome::Measured {
                        input_tokens: 128,
                        output_tokens: 128,
                    },
                }
            )
            .await,
        Err(Error::StaleLease)
    ));
    store
        .workflow_command(
            &f.spec.project_id,
            &run.id,
            lease,
            state.progress.revision(),
            WorkflowCommand::Settle {
                claim: a,
                id: "m1".into(),
                outcome: WorkflowOutcome::Measured {
                    input_tokens: 128,
                    output_tokens: 128,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .detail(&run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        2304
    );
    assert!(matches!(
        store
            .workflow_command(
                &f.spec.project_id,
                &run.id,
                "expired",
                0,
                WorkflowCommand::Admit {
                    claim: b,
                    id: "m2".into(),
                    operation: model()
                }
            )
            .await
            .unwrap(),
        WorkflowCommandResult::Admission(WorkflowAdmission::Existing(_))
    ));
    let export = Box::pin(store.export_order(&f.request.order_id))
        .await
        .unwrap();
    assert_eq!(export.format_version, 13);
    assert_eq!(export.data.workflows.len(), 1);
    let archive = &export.data.workflows[0];
    assert_eq!(archive.binding, run.spec.workflow.clone().unwrap());
    assert_eq!(
        archive.progress_sha256,
        cowork_domain::canonical_hash(&archive.progress)
    );
    let foreign = store
        .create_project(CreateProject {
            name: "Unrelated".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        store.stored_workflow(&foreign.id, &run.id).await,
        Err(Error::NotFound)
    ));
}
#[tokio::test]
async fn sqlite_workflow_contract() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    Box::pin(contract(&store)).await;
    store.close().await;
}
#[tokio::test]
#[ignore = "Requires a dedicated empty COWORK_TEST_POSTGRES database"]
async fn postgres_workflow_contract() {
    let url = std::env::var("COWORK_TEST_POSTGRES").unwrap();
    let store = Store::connect(&url).await.unwrap();
    Box::pin(contract(&store)).await;
    Box::pin(postgres_lock_wait_rechecks_original_lease(&store, &url)).await;
    Box::pin(postgres_initial_claim_must_be_live_at_commit(&store, &url)).await;
    Box::pin(tool_cases::postgres_approval_lease_fence(&store, &url)).await;
    store.close().await;
}

#[tokio::test]
async fn library_versions_revoke_replays_and_restart_preserve_immutable_definitions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::sqlite(&path).await.unwrap();
    let f = setup(&store).await;
    let original = store
        .workflow_version(
            &f.spec.project_id,
            &f.request.workflow_id,
            &f.request.version_id,
        )
        .await
        .unwrap();
    let mut next = f.definition.clone();
    next.name = "Reviewed version".into();
    let publication = store
        .publish_workflow(
            &f.spec.project_id,
            &f.request.workflow_id,
            1,
            "publish",
            &next,
        )
        .await
        .unwrap();
    assert_eq!(publication.revision, 2);
    store
        .revoke_workflow(&f.spec.project_id, &f.request.workflow_id, 2, "revoke")
        .await
        .unwrap();
    assert_eq!(
        store
            .publish_workflow(
                &f.spec.project_id,
                &f.request.workflow_id,
                1,
                "publish",
                &next
            )
            .await
            .unwrap()
            .version_id,
        publication.version_id
    );
    assert!(
        store
            .workflow(&f.spec.project_id, &f.request.workflow_id)
            .await
            .unwrap()
            .revoked
    );
    assert!(matches!(
        store.create_workflow_run(&f.request, f.spec.clone()).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(store.order(&f.request.order_id).await.unwrap().revision, 1);
    assert!(
        store
            .messages(&f.request.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        store
            .workflow_version(
                &f.spec.project_id,
                &f.request.workflow_id,
                &f.request.version_id
            )
            .await
            .unwrap()
            .reference
            .sha256,
        original.reference.sha256
    );
    assert!(
        store
            .create_workflow(&f.spec.project_id, "create", &f.definition)
            .await
            .unwrap()
            .revoked
    );
    next.name = "Changed replay".into();
    assert!(matches!(
        store
            .publish_workflow(
                &f.spec.project_id,
                &f.request.workflow_id,
                1,
                "publish",
                &next
            )
            .await,
        Err(Error::Conflict)
    ));
}
#[tokio::test]
async fn missing_or_stale_models_roll_back_order_consumption() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::sqlite(&path).await.unwrap();
    let f = setup(&store).await;
    let pool = raw(&path).await;
    let provider = &f.definition.roles[0].provider_id;
    let value: String = sqlx::query_scalar("SELECT payload FROM providers WHERE id=$1")
        .bind(provider)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut profile: ProviderProfile = serde_json::from_str(&value).unwrap();
    profile.revision = 2;
    sqlx::query("UPDATE providers SET payload=$1 WHERE id=$2")
        .bind(serde_json::to_string(&profile).unwrap())
        .bind(provider)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store.create_workflow_run(&f.request, f.spec.clone()).await,
        Err(Error::Conflict)
    ));
    assert_eq!(store.order(&f.request.order_id).await.unwrap().revision, 1);
    assert!(
        store
            .messages(&f.request.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("DELETE FROM providers WHERE id=$1")
        .bind(provider)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store.create_workflow_run(&f.request, f.spec.clone()).await,
        Err(Error::Provider(_))
    ));
    let mut missing = f.definition.clone();
    missing.roles[0].provider_id = "unconnected-model".into();
    store
        .create_workflow(&f.spec.project_id, "save-unconnected", &missing)
        .await
        .unwrap();
    pool.close().await;
}
#[tokio::test]
async fn failed_event_commit_returns_no_dispatch_and_no_budget_charge() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::sqlite(&path).await.unwrap();
    let f = setup(&store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = &parent.lease.as_ref().unwrap().token;
    let a = claim_node(&store, &f, &run, lease, "a").await;
    let before = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let sequence = store
        .stored_run(&run.id)
        .await
        .unwrap()
        .record
        .last_sequence;
    let pool = raw(&path).await;
    sqlx::query("CREATE TRIGGER reject_workflow_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT,'controlled receipt failure'); END").execute(&pool).await.unwrap();
    assert!(
        store
            .workflow_command(
                &f.spec.project_id,
                &run.id,
                lease,
                before.progress.revision(),
                WorkflowCommand::Admit {
                    claim: a.clone(),
                    id: "m1".into(),
                    operation: model()
                }
            )
            .await
            .is_err()
    );
    let after = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert_eq!(after.progress.revision(), before.progress.revision());
    assert!(after.progress.attempts().is_empty());
    assert_eq!(
        store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .last_sequence,
        sequence
    );
    assert_eq!(
        store
            .detail(&run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        0
    );
    sqlx::query("DROP TRIGGER reject_workflow_event")
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        admit(&store, &f, &run, lease, &a, "m1", model()).await,
        WorkflowCommandResult::Admission(WorkflowAdmission::Dispatch(_))
    ));
    pool.close().await;
}
async fn concurrent_admission_contract(store: &Store, second: &Store) {
    let f = setup(store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let a = claim_node(store, &f, &run, &lease, "a").await;
    let before = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let sequence = store
        .stored_run(&run.id)
        .await
        .unwrap()
        .record
        .last_sequence;
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..12 {
        let store = if index % 2 == 0 {
            store.clone()
        } else {
            second.clone()
        };
        let p = f.spec.project_id.clone();
        let run = run.id.clone();
        let lease = lease.clone();
        let a = a.clone();
        let expected = before.progress.revision();
        tasks.spawn(async move {
            store
                .workflow_command(
                    &p,
                    &run,
                    &lease,
                    expected,
                    WorkflowCommand::Admit {
                        claim: a,
                        id: "same".into(),
                        operation: model(),
                    },
                )
                .await
        });
    }
    let mut dispatches = 0;
    while let Some(r) = tasks.join_next().await {
        match r.unwrap() {
            Ok(WorkflowCommandResult::Admission(WorkflowAdmission::Dispatch(_))) => dispatches += 1,
            Ok(WorkflowCommandResult::Admission(WorkflowAdmission::Existing(_)))
            | Err(Error::Conflict) => {}
            other => panic!("unexpected admission result: {other:?}"),
        }
    }
    assert_eq!(dispatches, 1);
    assert_eq!(
        store
            .stored_workflow(&f.spec.project_id, &run.id)
            .await
            .unwrap()
            .progress
            .attempts()
            .len(),
        1
    );
    assert_eq!(
        store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .last_sequence,
        sequence + 1
    );
}
#[tokio::test]
async fn revocation_denies_new_actions_but_accepts_exact_existing_settlement() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let f = setup(&store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let a = claim_node(&store, &f, &run, &lease, "a").await;
    admit(&store, &f, &run, &lease, &a, "m1", model()).await;
    store
        .revoke_workflow(&f.spec.project_id, &f.request.workflow_id, 1, "revoke")
        .await
        .unwrap();
    let before = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert!(matches!(
        store
            .workflow_command(
                &f.spec.project_id,
                &run.id,
                &lease,
                before.progress.revision(),
                WorkflowCommand::Claim {
                    node_id: "b".into()
                }
            )
            .await,
        Err(Error::Forbidden)
    ));
    store
        .workflow_command(
            &f.spec.project_id,
            &run.id,
            &lease,
            before.progress.revision(),
            WorkflowCommand::Settle {
                claim: a,
                id: "m1".into(),
                outcome: WorkflowOutcome::Unmeasured {
                    input_tokens: None,
                    output_tokens: None,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .detail(&run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        2048
    );
}
#[tokio::test]
async fn corrupt_checkpoint_or_version_is_rejected_without_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::sqlite(&path).await.unwrap();
    let f = setup(&store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let pool = raw(&path).await;
    let value: String = sqlx::query_scalar("SELECT payload FROM workflow_runs WHERE run_id=$1")
        .bind(&run.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&value).unwrap();
    value["progress"]["revision"] = 99.into();
    sqlx::query("UPDATE workflow_runs SET payload=$1 WHERE run_id=$2")
        .bind(value.to_string())
        .bind(&run.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        store
            .stored_workflow(&f.spec.project_id, &run.id)
            .await
            .is_err()
    );
    assert!(store.claim().await.unwrap().is_none());
    let value: String = sqlx::query_scalar("SELECT payload FROM workflow_versions WHERE id=$1")
        .bind(&f.request.version_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&value).unwrap();
    value["definition"]["name"] = "forged".into();
    sqlx::query("UPDATE workflow_versions SET payload=$1 WHERE id=$2")
        .bind(value.to_string())
        .bind(&f.request.version_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        store
            .workflow_version(
                &f.spec.project_id,
                &f.request.workflow_id,
                &f.request.version_id
            )
            .await
            .is_err()
    );
    pool.close().await;
}
#[tokio::test]
async fn expired_parent_lease_preserves_unknown_action_and_blocks_resume_or_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::sqlite(&path).await.unwrap();
    let f = setup(&store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let a = claim_node(&store, &f, &run, &lease, "a").await;
    admit(
        &store,
        &f,
        &run,
        &lease,
        &a,
        "unsafe",
        WorkflowOperation::Tool {
            tool_id: "fixture_effect".into(),
            arguments_sha256: hash(b"exact fixture"),
        },
    )
    .await;
    store
        .mutate(&run.id, Some(&lease), |r| {
            r.lease.as_mut().unwrap().until = now() - 1;
            Ok(Change::event("fixture_expired", json!({})))
        })
        .await
        .unwrap();
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(store.recover_expired_leases().await.unwrap(), 1);
    let after = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert_eq!(after.progress.state(), WorkflowState::Interrupted);
    assert_eq!(after.progress.attempts().len(), 1);
    assert!(after.progress.attempts()["unsafe"].outcome.is_none());
    assert!(store.cancel(&run.id).await.is_err());
    assert!(store.resume(&run.id).await.is_err());
    assert!(matches!(
        store
            .workflow_command(
                &f.spec.project_id,
                &run.id,
                "no-lease",
                0,
                WorkflowCommand::Admit {
                    claim: a,
                    id: "unsafe".into(),
                    operation: WorkflowOperation::Tool {
                        tool_id: "fixture_effect".into(),
                        arguments_sha256: hash(b"exact fixture")
                    }
                }
            )
            .await
            .unwrap(),
        WorkflowCommandResult::Admission(WorkflowAdmission::Existing(_))
    ));
}

// A synthetic actor verifies admission/checkpoint durability across a hard crash.
// It deliberately does not claim to qualify the shared IO broker or a sandbox.
#[tokio::test]
#[ignore = "Synthetic child; invoked by workflow_admission_survives_hard_process_end"]
async fn workflow_process_fixture() {
    use std::io::Write;
    let directory = std::path::PathBuf::from(
        std::env::var_os("COWORK_WORKFLOW_FIXTURE").expect("parent fixture directory"),
    );
    let store = Store::sqlite(&directory.join("workflow.sqlite3"))
        .await
        .unwrap();
    let f = setup(&store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let claim = claim_node(&store, &f, &run, &lease, "a").await;
    let operation = WorkflowOperation::Tool {
        tool_id: "fixture_effect".into(),
        arguments_sha256: hash(b"synthetic process proof"),
    };
    let admission = admit(
        &store,
        &f,
        &run,
        &lease,
        &claim,
        "unsafe",
        operation.clone(),
    )
    .await;
    assert!(matches!(
        admission,
        WorkflowCommandResult::Admission(WorkflowAdmission::Dispatch(_))
    ));
    let mut effect = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join("synthetic-effect.txt"))
        .unwrap();
    effect.write_all(b"one admitted synthetic action").unwrap();
    effect.sync_all().unwrap();
    drop(effect);
    // Expire only this disposable fixture's lease so recovery does not wait 15s.
    store
        .mutate(&run.id, Some(&lease), |r| {
            r.lease.as_mut().unwrap().until = now() - 1;
            Ok(Change::event("fixture_expired", json!({})))
        })
        .await
        .unwrap();
    let checkpoint = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let confirmed = json!({"run":run,"claim":claim,"operation":operation,
        "progress_sha256":cowork_domain::canonical_hash(&checkpoint.progress)});
    let mut marker = std::fs::File::create(directory.join("confirmed.json")).unwrap();
    marker
        .write_all(&serde_json::to_vec(&confirmed).unwrap())
        .unwrap();
    marker.sync_all().unwrap();
    drop(marker);
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
}

#[test]
fn workflow_admission_survives_hard_process_end() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "workflow_process_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("COWORK_WORKFLOW_FIXTURE", temp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Child(command.spawn().unwrap());
    let until = Instant::now() + Duration::from_secs(15);
    let confirmed: serde_json::Value = loop {
        if let Ok(bytes) = std::fs::read(temp.path().join("confirmed.json"))
            && let Ok(record) = serde_json::from_slice(&bytes)
        {
            break record;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child exited before acknowledgement"
        );
        assert!(
            Instant::now() < until,
            "child did not acknowledge admission"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let run: RunRecord = serde_json::from_value(confirmed["run"].clone()).unwrap();
        let claim: WorkflowClaim = serde_json::from_value(confirmed["claim"].clone()).unwrap();
        let operation: WorkflowOperation =
            serde_json::from_value(confirmed["operation"].clone()).unwrap();
        let store = Store::sqlite(&temp.path().join("workflow.sqlite3"))
            .await
            .unwrap();
        let initial = store
            .stored_workflow(&run.spec.project_id, &run.id)
            .await
            .unwrap();
        assert_eq!(
            cowork_domain::canonical_hash(&initial.progress),
            confirmed["progress_sha256"].as_str().unwrap()
        );
        assert!(initial.progress.attempts()["unsafe"].outcome.is_none());
        assert_eq!(store.recover_expired_leases().await.unwrap(), 1);
        let after = store
            .stored_workflow(&run.spec.project_id, &run.id)
            .await
            .unwrap();
        assert_eq!(after.progress.state(), WorkflowState::Interrupted);
        assert_eq!(after.progress.attempts().len(), 1);
        assert!(after.progress.attempts()["unsafe"].outcome.is_none());
        assert_eq!(
            store.detail(&run.id).await.unwrap().run.state,
            RunState::Interrupted
        );
        assert!(store.cancel(&run.id).await.is_err());
        assert!(store.resume(&run.id).await.is_err());
        assert!(store.claim().await.unwrap().is_none());
        let replay = store
            .workflow_command(
                &run.spec.project_id,
                &run.id,
                "dead-worker",
                0,
                WorkflowCommand::Admit {
                    claim,
                    id: "unsafe".into(),
                    operation,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            replay,
            WorkflowCommandResult::Admission(WorkflowAdmission::Existing(_))
        ));
        assert_eq!(
            std::fs::read(temp.path().join("synthetic-effect.txt")).unwrap(),
            b"one admitted synthetic action"
        );
        assert_eq!(
            store
                .export_order(&run.spec.order_id)
                .await
                .unwrap()
                .data
                .workflows
                .len(),
            1
        );
        store.close().await;
    });
}

#[tokio::test]
async fn concurrent_same_admission_yields_one_dispatch_and_no_duplicate_event() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("concurrent.sqlite3");
    let first = Store::sqlite(&path).await.unwrap();
    let second = Store::sqlite(&path).await.unwrap();
    concurrent_admission_contract(&first, &second).await;
    second.close().await;
    first.close().await;
}
async fn concurrent_registry_contract(store: &Store) {
    let f = setup(store).await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let store = store.clone();
        let project = f.spec.project_id.clone();
        let definition = f.definition.clone();
        tasks.spawn(async move {
            store
                .create_workflow(&project, "shared-create", &definition)
                .await
        });
    }
    let mut library_id = None;
    let mut version_id = None;
    while let Some(result) = tasks.join_next().await {
        let entry = result.unwrap().unwrap();
        assert_eq!(entry.revision, 1);
        assert_eq!(entry.versions.len(), 1);
        assert_eq!(library_id.get_or_insert(entry.id.clone()), &entry.id);
        assert_eq!(
            version_id.get_or_insert(entry.versions[0].id.clone()),
            &entry.versions[0].id
        );
    }
    let workflow = library_id.unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let store = store.clone();
        let project = f.spec.project_id.clone();
        let workflow = workflow.clone();
        let mut definition = f.definition.clone();
        definition.name = "Shared publication".into();
        tasks.spawn(async move {
            store
                .publish_workflow(&project, &workflow, 1, "shared-publish", &definition)
                .await
        });
    }
    let mut published_id = None;
    while let Some(result) = tasks.join_next().await {
        let publication = result.unwrap().unwrap();
        assert_eq!(publication.revision, 2);
        let published = publication.version_id.unwrap();
        assert_eq!(published_id.get_or_insert(published.clone()), &published);
    }
    for i in 0..2 {
        let store = store.clone();
        let project = f.spec.project_id.clone();
        let workflow = workflow.clone();
        let mut definition = f.definition.clone();
        definition.name = format!("Contender {i}");
        tasks.spawn(async move {
            store
                .publish_workflow(
                    &project,
                    &workflow,
                    2,
                    &format!("contender-{i}"),
                    &definition,
                )
                .await
        });
    }
    let mut successes = 0;
    let mut conflicts = 0;
    while let Some(result) = tasks.join_next().await {
        match result.unwrap() {
            Ok(value) => {
                assert_eq!(value.revision, 3);
                successes += 1;
            }
            Err(Error::Conflict) => conflicts += 1,
            _ => panic!("unexpected publication result"),
        }
    }
    assert_eq!((successes, conflicts), (1, 1));
    let head = store.workflow(&f.spec.project_id, &workflow).await.unwrap();
    assert_eq!(head.revision, 3);
    assert_eq!(head.versions.len(), 3);
    store
        .revoke_workflow(&f.spec.project_id, &workflow, 3, "terminal-revoke")
        .await
        .unwrap();
    let mut definition = f.definition;
    definition.name = "Shared publication".into();
    let replay = store
        .publish_workflow(
            &f.spec.project_id,
            &workflow,
            1,
            "shared-publish",
            &definition,
        )
        .await
        .unwrap();
    assert_eq!(replay.version_id, published_id);
    assert!(
        store
            .workflow(&f.spec.project_id, &workflow)
            .await
            .unwrap()
            .revoked
    );
}
#[tokio::test]
async fn failed_publication_receipt_rolls_back_head_and_new_version() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("library.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let f = setup(&store).await;
    let pool = raw(&path).await;
    sqlx::query("CREATE TRIGGER reject_publication BEFORE INSERT ON workflow_changes BEGIN SELECT RAISE(ABORT,'controlled publication failure'); END").execute(&pool).await.unwrap();
    let mut next = f.definition.clone();
    next.name = "New version".into();
    assert!(
        store
            .publish_workflow(
                &f.spec.project_id,
                &f.request.workflow_id,
                1,
                "publish",
                &next
            )
            .await
            .is_err()
    );
    let unchanged = store
        .workflow(&f.spec.project_id, &f.request.workflow_id)
        .await
        .unwrap();
    assert_eq!(unchanged.revision, 1);
    assert_eq!(unchanged.versions.len(), 1);
    assert!(!unchanged.revoked);
    sqlx::query("DROP TRIGGER reject_publication")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .publish_workflow(
                &f.spec.project_id,
                &f.request.workflow_id,
                1,
                "publish",
                &next
            )
            .await
            .unwrap()
            .revision,
        2
    );
    pool.close().await;
}

async fn postgres_lock_wait_rechecks_original_lease(store: &Store, url: &str) {
    let f = setup(store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let claim = claim_node(store, &f, &run, &lease, "a").await;
    let before = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let pool = sqlx::PgPool::connect(url).await.unwrap();
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("UPDATE workflows SET revision=revision WHERE id=$1")
        .bind(&f.request.workflow_id)
        .execute(&mut *lock)
        .await
        .unwrap();
    store
        .mutate(&run.id, Some(&lease), |r| {
            r.lease.as_mut().unwrap().until = now() + 400;
            Ok(Change::event("fixture_short_lease", json!({})))
        })
        .await
        .unwrap();
    let owned = store.clone();
    let project = f.spec.project_id.clone();
    let run_id = run.id.clone();
    let expected = before.progress.revision();
    let task = tokio::spawn(async move {
        owned
            .workflow_command(
                &project,
                &run_id,
                &lease,
                expected,
                WorkflowCommand::Admit {
                    claim,
                    id: "blocked-model".into(),
                    operation: model(),
                },
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !task.is_finished(),
        "admission must wait for the held library lock"
    );
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    lock.commit().await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(Error::StaleLease)));
    let after = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert_eq!(after.progress.revision(), before.progress.revision());
    assert!(after.progress.attempts().is_empty());
    assert_eq!(
        store
            .detail(&run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        0
    );
    pool.close().await;
}

async fn model_journal_contract(store: &Store) {
    use cowork_storage::{WorkflowModelAdmission, WorkflowModelResponse};
    let mut f = setup(store).await;
    // Parent has two selected capability references; this step may see only one.
    // This is a no-IO storage contract, not authorization to a real file broker.
    f.spec.folders = ["selected", "excluded"]
        .into_iter()
        .map(|name| FolderGrant {
            id: name.into(),
            project_id: f.spec.project_id.clone(),
            name: name.into(),
            writable: false,
            revision: "fixture".into(),
            resource_id: name.into(),
            order_id: None,
        })
        .collect();
    let mut definition = f.definition.clone();
    definition.rights.folders = BTreeSet::from(["selected".into(), "excluded".into()]);
    definition.roles[0].rights.folders = definition.rights.folders.clone();
    definition.steps[0].rights.folders = BTreeSet::from(["selected".into()]);
    let publication = store
        .publish_workflow(
            &f.spec.project_id,
            &f.request.workflow_id,
            1,
            "scoped-folders",
            &definition,
        )
        .await
        .unwrap();
    f.request.version_id = publication.version_id.unwrap();
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let claim = claim_node(store, &f, &run, &lease, "a").await;
    let (revision, context) = store
        .workflow_model_context(&f.spec.project_id, &run.id, &claim, json!([]))
        .await
        .unwrap();
    assert_eq!(context.spec.folders.len(), 1);
    assert_eq!(context.spec.folders[0].id, "selected");
    let system_frames: Vec<_> = context
        .messages
        .iter()
        .filter(|m| m["role"] == "system")
        .collect();
    assert_eq!(system_frames.len(), 2);
    assert!(
        system_frames[1]["content"]
            .as_str()
            .unwrap()
            .contains("selected")
    );
    assert!(
        !system_frames[1]["content"]
            .as_str()
            .unwrap()
            .contains("excluded")
    );
    let mut forged = context.clone();
    forged.spec.provider.as_mut().unwrap().model = "different-model".into();
    assert!(matches!(
        store
            .begin_workflow_model(
                &f.spec.project_id,
                &run.id,
                &lease,
                revision,
                &claim,
                "request",
                &forged
            )
            .await,
        Err(Error::Conflict)
    ));
    assert!(
        store
            .workflow_model_turn(&f.spec.project_id, &run.id, "request")
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        store
            .begin_workflow_model(
                &f.spec.project_id,
                &run.id,
                &lease,
                revision,
                &claim,
                "request",
                &context
            )
            .await
            .unwrap(),
        WorkflowModelAdmission::Dispatch(_)
    ));
    assert!(matches!(
        store
            .begin_workflow_model(
                &f.spec.project_id,
                &run.id,
                "stale",
                0,
                &claim,
                "request",
                &context
            )
            .await
            .unwrap(),
        WorkflowModelAdmission::Existing(_)
    ));
    let reply = WorkflowModelResponse {
        provider_continuation: None,
        received_cost_metadata: None,
        content: "Confirmed agent answer".into(),
        tools: vec![],
        usage: Usage {
            input_tokens: Some(100),
            output_tokens: Some(32),
            ..Default::default()
        },
        rejection_code: None,
    };
    let saved = store
        .finish_workflow_model(
            &f.spec.project_id,
            &run.id,
            &lease,
            &claim,
            "request",
            reply.clone(),
        )
        .await
        .unwrap();
    assert_eq!(saved.response.as_ref().unwrap().content, reply.content);
    assert_eq!(
        store
            .detail(&run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        132
    );
    store
        .finish_workflow_model(
            &f.spec.project_id,
            &run.id,
            "stale",
            &claim,
            "request",
            reply.clone(),
        )
        .await
        .unwrap();
    let mut changed = reply.clone();
    changed.content = "Changed replay".into();
    assert!(matches!(
        store
            .finish_workflow_model(
                &f.spec.project_id,
                &run.id,
                &lease,
                &claim,
                "request",
                changed
            )
            .await,
        Err(Error::Conflict)
    ));
    let after = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    store
        .workflow_command(
            &f.spec.project_id,
            &run.id,
            &lease,
            after.progress.revision(),
            WorkflowCommand::Complete {
                claim,
                output: reply.content,
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .finish_workflow_run(&f.spec.project_id, &run.id, &lease)
            .await
            .is_err()
    );
    assert!(
        store
            .mutate(&run.id, Some(&lease), |r| {
                r.transition(RunState::Completed)?;
                Ok(Change::event("forged_completion", json!({})))
            })
            .await
            .is_err()
    );
    for node in ["b", "c"] {
        let claim = claim_node(store, &f, &run, &lease, node).await;
        let (revision, context) = store
            .workflow_model_context(&f.spec.project_id, &run.id, &claim, json!([]))
            .await
            .unwrap();
        let attempt = format!("request-{node}");
        store
            .begin_workflow_model(
                &f.spec.project_id,
                &run.id,
                &lease,
                revision,
                &claim,
                &attempt,
                &context,
            )
            .await
            .unwrap();
        let reply = WorkflowModelResponse {
            provider_continuation: None,
            received_cost_metadata: None,
            content: format!("Final {node}"),
            tools: vec![],
            usage: Usage {
                input_tokens: Some(100),
                output_tokens: Some(32),
                ..Default::default()
            },
            rejection_code: None,
        };
        store
            .finish_workflow_model(
                &f.spec.project_id,
                &run.id,
                &lease,
                &claim,
                &attempt,
                reply.clone(),
            )
            .await
            .unwrap();
        let before = store
            .stored_workflow(&f.spec.project_id, &run.id)
            .await
            .unwrap();
        store
            .workflow_command(
                &f.spec.project_id,
                &run.id,
                &lease,
                before.progress.revision(),
                WorkflowCommand::Complete {
                    claim,
                    output: reply.content,
                },
            )
            .await
            .unwrap();
    }
    let finished = store
        .finish_workflow_run(&f.spec.project_id, &run.id, &lease)
        .await
        .unwrap();
    assert_eq!(finished.record.state, RunState::Completed);
    assert!(finished.lease.is_none());
    assert_eq!(
        store
            .finish_workflow_run(&f.spec.project_id, &run.id, "old-lease")
            .await
            .unwrap()
            .record
            .revision,
        finished.record.revision
    );
    assert_eq!(
        store
            .messages(&f.request.order_id)
            .await
            .unwrap()
            .iter()
            .filter(|m| m.role == "assistant")
            .count(),
        1
    );
    assert_eq!(
        store
            .messages(&f.request.order_id)
            .await
            .unwrap()
            .last()
            .unwrap()
            .content,
        "Final c"
    );
    let archive = store.export_order(&run.spec.order_id).await.unwrap();
    assert_eq!(archive.format_version, 13);
    assert!(
        archive.data.workflows[0].agent_contexts.as_ref().unwrap()["a"]["turns"][0]["response"]
            .is_object()
    );
}

async fn worker_queue_contract(store: &Store) {
    let mut f = setup(store).await;
    f.spec.folders = vec![FolderGrant {
        id: id(),
        project_id: f.spec.project_id.clone(),
        name: "Queue fixture".into(),
        writable: true,
        revision: "a".repeat(64),
        resource_id: id(),
        order_id: None,
    }];
    let first = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    assert!(store.claim().await.unwrap().is_none());
    let (a, b) = tokio::join!(store.claim_for_worker(), store.claim_for_worker());
    let claims: Vec<_> = [a.unwrap(), b.unwrap()].into_iter().flatten().collect();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].record.id, first.id);
    assert_eq!(claims[0].record.state, RunState::Running);
    let lease = claims[0].lease.as_ref().unwrap().token.clone();
    let mut request = f.request.clone();
    request.base_revision = store.order(&request.order_id).await.unwrap().revision;
    request.idempotency_key = "waiting-workflow".into();
    let second = store
        .create_workflow_run(&request, f.spec.clone())
        .await
        .unwrap();
    assert!(store.claim_for_worker().await.unwrap().is_none());
    let waiting = store.stored_run(&second.id).await.unwrap();
    assert_eq!(waiting.record.state, RunState::WaitingForExecutor);
    assert!(waiting.record.started_at.is_none() && waiting.lease.is_none());
    store
        .revoke_workflow(
            &f.spec.project_id,
            &f.request.workflow_id,
            1,
            "revoke-waiting",
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .claim_workflow_run(&f.spec.project_id, &second.id)
            .await,
        Err(Error::WorkflowRevoked)
    ));
    assert!(store.claim_for_worker().await.unwrap().is_none());
    let denied = store.stored_run(&second.id).await.unwrap();
    assert_eq!(denied.record.state, RunState::Failed);
    assert_eq!(
        denied.record.error_code.as_deref(),
        Some("workflow_revoked")
    );
    assert!(denied.record.started_at.is_none() && denied.record.model_attempts.is_empty());
    let graph = store
        .stored_workflow(&f.spec.project_id, &second.id)
        .await
        .unwrap();
    assert_eq!(graph.progress.state(), WorkflowState::Ready);
    assert_eq!(graph.progress.revision(), 0);
    assert!(graph.progress.attempts().is_empty() && graph.agents.is_empty());
    let terminal = store
        .finish_workflow_run(&f.spec.project_id, &second.id, "read-only")
        .await
        .unwrap();
    assert_eq!(terminal.record.revision, denied.record.revision);
    assert!(store.claim_for_worker().await.unwrap().is_none());
    assert_eq!(
        store.stored_run(&second.id).await.unwrap().record.revision,
        denied.record.revision
    );
    assert_eq!(
        store
            .events(&second.id, 0)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.kind == "workflow_start_rejected")
            .count(),
        1
    );
    assert!(
        store
            .claim_workflow_run(&f.spec.project_id, &first.id)
            .await
            .is_err()
    );
    assert!(store.claim_for_worker().await.unwrap().is_none());
    store.cancel(&first.id).await.unwrap();
    store
        .mutate(&first.id, Some(&lease), |r| {
            r.transition(RunState::Canceled)?;
            r.lease = None;
            Ok(Change::event("fixture_canceled", json!({})))
        })
        .await
        .unwrap();
    let fresh = setup(store).await;
    let third = store
        .create_workflow_run(&fresh.request, fresh.spec.clone())
        .await
        .unwrap();
    assert_eq!(
        store.claim_for_worker().await.unwrap().unwrap().record.id,
        third.id
    );
    let parent = store.stored_run(&third.id).await.unwrap();
    store.cancel(&third.id).await.unwrap();
    store
        .mutate(&third.id, Some(&parent.lease.unwrap().token), |r| {
            r.transition(RunState::Canceled)?;
            r.lease = None;
            Ok(Change::event("fixture_canceled", json!({})))
        })
        .await
        .unwrap();
}
#[tokio::test]
async fn missing_workflow_binding_cannot_enter_the_single_agent_worker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::sqlite(&path).await.unwrap();
    let f = setup(&store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let pool = raw(&path).await;
    let payload: String = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
        .bind(&run.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    payload["record"]["spec"]
        .as_object_mut()
        .unwrap()
        .remove("workflow");
    sqlx::query("UPDATE runs SET payload=$1 WHERE id=$2")
        .bind(payload.to_string())
        .bind(&run.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store.claim_for_worker().await,
        Err(Error::Forbidden)
    ));
    let parent = store.stored_run(&run.id).await.unwrap();
    assert_eq!(parent.record.state, RunState::Queued);
    assert!(parent.lease.is_none());
    assert_eq!(parent.record.last_sequence, 2);
    assert!(store.claim().await.unwrap().is_none());
    pool.close().await;
    store.close().await;
}

async fn postgres_initial_claim_must_be_live_at_commit(store: &Store, url: &str) {
    let f = setup(store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let pool = sqlx::PgPool::connect(url).await.unwrap();
    let mut lock = pool.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    sqlx::query("UPDATE workflows SET revision=revision WHERE id=$1")
        .bind(&f.request.workflow_id)
        .execute(&mut *lock)
        .await
        .unwrap();
    let owned = store.clone();
    let project = f.spec.project_id.clone();
    let run_id = run.id.clone();
    let task = tokio::spawn(async move { owned.claim_workflow_run(&project, &run_id).await });
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let blocked:i64=sqlx::query_scalar("SELECT COUNT(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE workflows%' AND $1=ANY(pg_blocking_pids(pid))")
                .bind(blocker).fetch_one(&pool).await.unwrap();
            if blocked>0 {break;}
            assert!(!task.is_finished());
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    // Exercise the real newly minted 15-second lease, not an altered fixture lease.
    tokio::time::sleep(std::time::Duration::from_millis(15_200)).await;
    assert!(!task.is_finished());
    lock.commit().await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(Error::StaleLease)));
    let parent = store.stored_run(&run.id).await.unwrap();
    assert_eq!(parent.record.state, RunState::Queued);
    assert!(parent.lease.is_none() && parent.record.started_at.is_none());
    assert_eq!(parent.record.last_sequence, 2);
    let graph = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert_eq!(graph.progress.revision(), 0);
    assert!(graph.progress.attempts().is_empty());
    let next = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert!(next.lease.as_ref().unwrap().until > now());
    store.cancel(&run.id).await.unwrap();
    store
        .mutate(&run.id, Some(&next.lease.unwrap().token), |r| {
            r.transition(RunState::Canceled)?;
            r.lease = None;
            Ok(Change::event("fixture_canceled", json!({})))
        })
        .await
        .unwrap();
    pool.close().await;
}
