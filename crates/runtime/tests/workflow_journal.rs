use cowork_contracts::*;
use cowork_domain::Error;
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use cowork_workflow_contracts::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

struct Fixture {
    _dir: tempfile::TempDir,
    runtime: Runtime,
    project: Project,
    input: WorkflowRunRequest,
    profile: ProviderProfile,
}
fn budgets() -> Budgets {
    Budgets {
        max_steps: 32,
        max_seconds: 60,
        max_output_tokens: 256,
        max_total_tokens: 131_072,
    }
}
fn definition(profile: &ProviderProfile) -> WorkflowDefinition {
    let rights = WorkflowRights {
        current_workspace: false,
        tools: ["create_artifact", "list_artifacts", "read_artifact"]
            .into_iter()
            .map(String::from)
            .collect(),
        folders: Default::default(),
    };
    WorkflowDefinition {
        documents: Default::default(),
        iterations: Default::default(),
        subflows: Default::default(),
        controls: Default::default(),
        gates: Default::default(),
        dataflow: None,
        format_version: 1,
        name: "Create and verify a result".into(),
        mode: WorkflowMode::Sequential,
        failure: WorkflowFailure::Stop,
        rights: rights.clone(),
        budgets: budgets(),
        roles: vec![WorkflowRole {
            agent_config: None,
            model_binding: None,
            output: None,
            id: "writer".into(),
            instructions: "Create a useful result using only the selected tools.".into(),
            provider_id: profile.id.clone(),
            provider_revision: profile.revision,
            rights: rights.clone(),
            budgets: budgets(),
            delegate_to: Default::default(),
        }],
        steps: vec![WorkflowStep {
            execution: None,
            zero_context: false,
            id: "create".into(),
            role_id: "writer".into(),
            goal: "Produce the requested result.".into(),
            dependencies: vec![],
            rights,
            budgets: budgets(),
        }],
    }
}
async fn setup(context_tokens: u32) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("product.db")).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Product workflow".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Create a deliverable".into(),
        })
        .await
        .unwrap();
    let profile = ProviderProfile {
        generation: Default::default(),
        id: "product-test".into(),
        name: "Product test model".into(),
        kind: ProviderKind::Test,
        endpoint: "".into(),
        model: "test".into(),
        revision: 1,
        context_tokens,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    store.insert_provider(&profile).await.unwrap();
    let library = store
        .create_workflow(&project.id, "create-library", &definition(&profile))
        .await
        .unwrap();
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile: profile.clone(),
            credential_env: None,
        }],
        true,
    )
    .unwrap()
    .with_file_broker(cowork_broker::FileBroker::open(dir.path(), vec![]).unwrap());
    let input = WorkflowRunRequest {
        input_source: None,
        inputs: None,
        agent_config: None,
        native_tools: vec![],
        folders: vec![],
        mcp_tools: vec![],
        environment: None,
        kind: WorkflowRunKind::Workflow,
        order_id: order.id,
        base_revision: order.revision,
        idempotency_key: "start-workflow".into(),
        prompt: "Create a useful text artifact.".into(),
        workflow_id: library.id,
        version_id: library.versions[0].id.clone(),
        access_mode: AccessMode::ConfirmMutations,
        budgets: budgets(),
        expected_binding: None,
    };
    Fixture {
        _dir: dir,
        runtime,
        project,
        input,
        profile,
    }
}

async fn prepare(fixture: &Fixture, intent: &str) -> WorkflowOperation {
    fixture
        .runtime
        .store
        .prepare_workflow_operation(
            &fixture.project.id,
            &PrepareWorkflowOperation {
                intent_id: intent.into(),
                request: WorkflowOperationRequest::Start {
                    request: fixture.input.clone().into(),
                },
                draft_snapshot: None,
            },
        )
        .await
        .unwrap()
}
fn receipt_run(operation: &WorkflowOperation) -> &str {
    assert_eq!(operation.state, WorkflowOperationState::Accepted);
    match operation.receipt.as_ref().unwrap() {
        WorkflowOperationReceipt::Started { run_id, .. } => run_id,
        other => panic!("expected frozen start receipt: {other:?}"),
    }
}
#[derive(Clone)]
struct ProbeBarrier {
    calls: Arc<AtomicUsize>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    fail_first: bool,
}
async fn catalog(
    axum::extract::State(barrier): axum::extract::State<ProbeBarrier>,
) -> axum::Json<serde_json::Value> {
    let first = barrier.calls.fetch_add(1, Ordering::SeqCst) == 0;
    if first {
        barrier.entered.notify_one();
        barrier.release.notified().await;
    }
    axum::Json(if first && barrier.fail_first {
        serde_json::json!({"data":[]})
    } else {
        serde_json::json!({"data":[{"id":"test"}]})
    })
}
async fn barrier_setup(fail_first: bool) -> (Fixture, ProbeBarrier, tokio::task::JoinHandle<()>) {
    let barrier = ProbeBarrier {
        calls: Default::default(),
        entered: Default::default(),
        release: Default::default(),
        fail_first,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let routes = axum::Router::new()
        .route("/models", axum::routing::get(catalog))
        .with_state(barrier.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, routes).await.unwrap();
    });
    let mut fixture = setup(16_384).await;
    let mut profile = fixture.profile.clone();
    // INSERT-only profiles must receive a new identity and pinned publication.
    profile.id = "journal-probe".into();
    profile.kind = ProviderKind::OpenaiCompatible;
    profile.endpoint = endpoint;
    fixture
        .runtime
        .store
        .insert_provider(&profile)
        .await
        .unwrap();
    fixture
        .runtime
        .providers
        .write()
        .await
        .push(ConfiguredProvider {
            profile: profile.clone(),
            credential_env: None,
        });
    let published = fixture
        .runtime
        .store
        .publish_workflow(
            &fixture.project.id,
            &fixture.input.workflow_id,
            1,
            "probe-version",
            &definition(&profile),
        )
        .await
        .unwrap();
    fixture.input.version_id = published.version_id.unwrap();
    fixture.profile = profile;
    (fixture, barrier, server)
}
fn commit_task(
    fixture: &Fixture,
    operation: &WorkflowOperation,
) -> tokio::task::JoinHandle<Result<WorkflowOperation, Error>> {
    let runtime = fixture.runtime.clone();
    let project = fixture.project.id.clone();
    let operation = operation.id.clone();
    tokio::spawn(async move {
        runtime
            .commit_workflow_operation(&project, &operation)
            .await
    })
}
async fn entered(
    task: &mut tokio::task::JoinHandle<Result<WorkflowOperation, Error>>,
    barrier: &ProbeBarrier,
) {
    tokio::select! {
        biased;
        result = task => panic!("commit returned before real GET /models barrier: {result:?}"),
        result = tokio::time::timeout(Duration::from_secs(10), barrier.entered.notified()) => {
            result.expect("the first live catalogue probe must enter the barrier");
        }
    }
}
async fn no_runs(fixture: &Fixture) {
    assert!(
        fixture
            .runtime
            .store
            .runs(&fixture.input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        std::fs::read_dir(fixture._dir.path().join("workspaces"))
            .unwrap()
            .count(),
        0
    );
}
async fn stop_server(server: tokio::task::JoinHandle<()>) {
    server.abort();
    let _ = server.await;
}
#[tokio::test]
async fn journal_discard_during_provider_probe_prevents_the_start_effect() {
    let (fixture, barrier, server) = barrier_setup(false).await;
    let operation = prepare(&fixture, "discard-probe").await;
    let mut task = commit_task(&fixture, &operation);
    entered(&mut task, &barrier).await;
    let discarded = fixture
        .runtime
        .store
        .discard_workflow_operation(&fixture.project.id, &operation.id)
        .await
        .unwrap();
    assert_eq!(discarded.state, WorkflowOperationState::Discarded);
    barrier.release.notify_one();
    let result = task.await.unwrap().unwrap();
    assert_eq!(result.state, WorkflowOperationState::Discarded);
    no_runs(&fixture).await;
    // Tombstone also blocks a direct start using the reserved write key.
    assert!(matches!(
        fixture
            .runtime
            .create_product_workflow_run(&fixture.input)
            .await,
        Err(Error::Conflict)
    ));
    no_runs(&fixture).await;
    stop_server(server).await;
}
#[tokio::test]
async fn journal_revocation_during_provider_probe_is_a_durable_rejection() {
    let (fixture, barrier, server) = barrier_setup(false).await;
    let operation = prepare(&fixture, "revoke-probe").await;
    let mut task = commit_task(&fixture, &operation);
    entered(&mut task, &barrier).await;
    // This completes while the provider response is held: no SQL head lock crosses IO.
    fixture
        .runtime
        .store
        .revoke_workflow(
            &fixture.project.id,
            &fixture.input.workflow_id,
            2,
            "revoke-during-probe",
        )
        .await
        .unwrap();
    barrier.release.notify_one();
    let result = task.await.unwrap().unwrap();
    assert_eq!(result.state, WorkflowOperationState::Rejected);
    // Shared new-run admission rejects a revoked registry head as Forbidden.
    assert_eq!(result.rejection.unwrap().code, "forbidden");
    no_runs(&fixture).await;
    let calls = barrier.calls.load(Ordering::SeqCst);
    assert_eq!(
        fixture
            .runtime
            .commit_workflow_operation(&fixture.project.id, &operation.id)
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Rejected
    );
    assert_eq!(barrier.calls.load(Ordering::SeqCst), calls);
    stop_server(server).await;
}
#[tokio::test]
async fn journal_configured_profile_drift_after_catalogue_probe_stays_pending() {
    let (fixture, barrier, server) = barrier_setup(false).await;
    let operation = prepare(&fixture, "profile-probe").await;
    let mut task = commit_task(&fixture, &operation);
    entered(&mut task, &barrier).await;
    fixture
        .runtime
        .providers
        .write()
        .await
        .iter_mut()
        .find(|p| p.profile.id == fixture.profile.id)
        .unwrap()
        .profile
        .model = "changed-live-profile".into();
    barrier.release.notify_one();
    assert!(matches!(task.await.unwrap(), Err(Error::Provider(code)) if code == "profile_changed"));
    assert_eq!(
        fixture
            .runtime
            .store
            .workflow_operation(&fixture.project.id, &operation.id)
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Pending
    );
    no_runs(&fixture).await;
    fixture
        .runtime
        .providers
        .write()
        .await
        .iter_mut()
        .find(|p| p.profile.id == fixture.profile.id)
        .unwrap()
        .profile = fixture.profile.clone();
    let accepted = fixture
        .runtime
        .commit_workflow_operation(&fixture.project.id, &operation.id)
        .await
        .unwrap();
    assert!(!receipt_run(&accepted).is_empty());
    assert_eq!(
        fixture
            .runtime
            .store
            .runs(&fixture.input.order_id)
            .await
            .unwrap()
            .len(),
        1
    );
    stop_server(server).await;
}
#[tokio::test]
async fn journal_concurrent_accepted_receipt_wins_over_a_late_probe_error() {
    let (fixture, barrier, server) = barrier_setup(true).await;
    let operation = prepare(&fixture, "accepted-probe").await;
    let mut duplicate = commit_task(&fixture, &operation);
    entered(&mut duplicate, &barrier).await;
    let accepted = fixture
        .runtime
        .commit_workflow_operation(&fixture.project.id, &operation.id)
        .await
        .unwrap();
    let accepted_receipt = serde_json::to_value(&accepted.receipt).unwrap();
    barrier.release.notify_one();
    let replay = duplicate.await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&replay.receipt).unwrap(),
        accepted_receipt
    );
    assert_eq!(receipt_run(&replay), receipt_run(&accepted));
    assert_eq!(
        fixture
            .runtime
            .store
            .runs(&fixture.input.order_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(barrier.calls.load(Ordering::SeqCst), 2);
    // Accepted receipt remains usable after live revocation and missing configuration.
    fixture
        .runtime
        .store
        .revoke_workflow(
            &fixture.project.id,
            &fixture.input.workflow_id,
            2,
            "revoke-after-accepted",
        )
        .await
        .unwrap();
    fixture.runtime.providers.write().await.clear();
    let later = fixture
        .runtime
        .commit_workflow_operation(&fixture.project.id, &operation.id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&later.receipt).unwrap(),
        accepted_receipt
    );
    assert_eq!(barrier.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture
            .runtime
            .store
            .discard_workflow_operation(&fixture.project.id, &operation.id)
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Accepted
    );
    stop_server(server).await;
}
#[tokio::test]
async fn journal_provider_failure_keeps_pending_and_frozen_request_can_retry() {
    let (fixture, barrier, server) = barrier_setup(true).await;
    let operation = prepare(&fixture, "retry-probe").await;
    let mut task = commit_task(&fixture, &operation);
    entered(&mut task, &barrier).await;
    barrier.release.notify_one();
    assert!(matches!(task.await.unwrap(), Err(Error::Provider(_))));
    let saved = fixture
        .runtime
        .store
        .workflow_operation(&fixture.project.id, &operation.id)
        .await
        .unwrap();
    assert_eq!(saved.state, WorkflowOperationState::Pending);
    assert_eq!(
        serde_json::to_value(&saved.request).unwrap(),
        serde_json::to_value(&operation.request).unwrap()
    );
    no_runs(&fixture).await;
    assert_eq!(
        fixture
            .runtime
            .commit_workflow_operation(&fixture.project.id, &operation.id)
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Accepted
    );
    stop_server(server).await;
}
#[tokio::test]
async fn journal_library_validation_rejects_only_the_saved_command_and_keeps_draft() {
    let fixture = setup(16_384).await;
    let draft = fixture
        .runtime
        .store
        .save_workflow_draft(
            &fixture.project.id,
            &SaveWorkflowDraft {
                id: "invalid-editor".into(),
                base_revision: 0,
                idempotency_key: "draft-once".into(),
                content: WorkflowDraftContent {
                    editor_format: "workflow-v1".into(),
                    data: serde_json::json!({"unfinished":"role"}),
                    origin: None,
                    archived: false,
                },
            },
        )
        .await
        .unwrap();
    let snapshot = match draft {
        WorkflowDraftSave::Saved { snapshot, .. } => snapshot,
        _ => panic!("first draft is saved"),
    };
    let mut invalid = definition(&fixture.profile);
    invalid.steps[0].role_id = "missing-role".into();
    let operation = fixture
        .runtime
        .store
        .prepare_workflow_operation(
            &fixture.project.id,
            &PrepareWorkflowOperation {
                intent_id: "invalid-publish".into(),
                draft_snapshot: Some(WorkflowDraftSnapshotRef {
                    draft_id: snapshot.draft_id.clone(),
                    snapshot_id: snapshot.id.clone(),
                    sha256: snapshot.sha256.clone(),
                }),
                request: WorkflowOperationRequest::Publish {
                    workflow_id: fixture.input.workflow_id.clone(),
                    request: PublishWorkflow {
                        base_revision: 1,
                        idempotency_key: "invalid-publish-once".into(),
                        definition: invalid,
                    },
                },
            },
        )
        .await
        .unwrap();
    let rejected = fixture
        .runtime
        .commit_workflow_operation(&fixture.project.id, &operation.id)
        .await
        .unwrap();
    assert_eq!(rejected.state, WorkflowOperationState::Rejected);
    assert_eq!(rejected.rejection.unwrap().code, "invalid_request");
    assert_eq!(
        fixture
            .runtime
            .store
            .workflow(&fixture.project.id, &fixture.input.workflow_id)
            .await
            .unwrap()
            .versions
            .len(),
        1
    );
    assert_eq!(
        fixture
            .runtime
            .store
            .workflow_draft_snapshot(&fixture.project.id, &snapshot.draft_id, &snapshot.id)
            .await
            .unwrap()
            .sha256,
        snapshot.sha256
    );
}
