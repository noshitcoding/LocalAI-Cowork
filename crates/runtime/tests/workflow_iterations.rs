use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use cowork_contracts::*;
use cowork_domain::{Error, canonical_hash, workflows::*};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use cowork_workflow_contracts::{
    CommitExecutionPlan, OwnerExecutionTargetV2, PrepareOwnerExecutionPlanV2,
    PreparedExecutionPlanV2, RunRequest, WorkflowRunRequest,
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Model {
    requests: Arc<Mutex<Vec<Value>>>,
    active: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
    malformed: bool,
    hold_two: bool,
}
async fn models() -> Json<Value> {
    Json(json!({"data":[{"id":"iteration-fixture"}]}))
}
async fn chat(State(model): State<Model>, Json(body): Json<Value>) -> String {
    model.requests.lock().unwrap().push(body.clone());
    let active = model.active.fetch_add(1, Ordering::SeqCst) + 1;
    model.maximum.fetch_max(active, Ordering::SeqCst);
    if model.hold_two {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if model.requests.lock().unwrap().len() >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    let frame = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .find(|value| value["workflow_step"].is_string())
        .unwrap();
    let value = frame["inputs"]["amount"].as_i64().unwrap();
    let output = if model.malformed && value == -3 {
        json!({"amount":"invalid integer"})
    } else {
        json!({"amount":value*2})
    };
    model.active.fetch_sub(1, Ordering::SeqCst);
    let reply = json!({"choices":[{"index":0,"delta":{"content":output.to_string()},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":32}});
    format!("data: {reply}\n\ndata: [DONE]\n\n")
}
fn definition(model: bool) -> WorkflowDefinition {
    let mut d: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-iteration.json"
    ))
    .unwrap();
    d.budgets.max_total_tokens = 131072;
    d.budgets.max_seconds = 180;
    d.iterations.get_mut("loop").unwrap().budgets = d.budgets.clone();
    let WorkflowDataType::Object { fields } = &mut d.dataflow.as_mut().unwrap().input else {
        panic!()
    };
    fields.insert("private_note".into(), WorkflowDataType::Text);
    let body = &mut d.iterations.get_mut("loop").unwrap().definition;
    body.budgets = d.budgets.clone();
    if model {
        body.controls.clear();
        body.roles=vec![serde_json::from_value(json!({"id":"worker","instructions":"Use only the explicit item input.","provider_id":"model","provider_revision":1,"rights":{"tools":[],"folders":[]},"budgets":body.budgets,"delegate_to":[]})).unwrap()];
        body.steps = vec![WorkflowStep {
            id: "echo".into(),
            role_id: "worker".into(),
            goal: "Double one integer".into(),
            dependencies: vec![],
            rights: WorkflowRights::default(),
            budgets: body.budgets.clone(),
            execution: None,
            zero_context: false,
        }];
    }
    d
}
struct Fixture {
    temp: tempfile::TempDir,
    runtime: Runtime,
    project: Project,
    input: WorkflowRunRequest,
    definition: WorkflowDefinition,
    model: Model,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new(
        mut definition: WorkflowDefinition,
        values: Value,
        malformed: bool,
        hold_two: bool,
    ) -> Self {
        let model = Model {
            requests: Arc::new(Mutex::new(vec![])),
            active: Arc::new(AtomicUsize::new(0)),
            maximum: Arc::new(AtomicUsize::new(0)),
            malformed,
            hold_two,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/models", get(models))
            .route("/chat/completions", post(chat))
            .with_state(model.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&temp.path().join("iterations.sqlite3"))
            .await
            .unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Public list qualification".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Ordered list results".into(),
            })
            .await
            .unwrap();
        let profile:ProviderProfile=serde_json::from_value(json!({"id":"model","name":"Loopback iteration fixture","kind":"openai_compatible","endpoint":endpoint,"model":"iteration-fixture","revision":1,"context_tokens":16384,"credential_ready":true,"credential_required":false})).unwrap();
        store.insert_provider(&profile).await.unwrap();
        if malformed {
            definition.iterations.get_mut("loop").unwrap().max_parallel = 1;
        }
        let entry = store
            .create_workflow(&project.id, "publish-list", &definition)
            .await
            .unwrap();
        let mut run_budgets = definition.budgets.clone();
        run_budgets.max_steps = 32;
        let input=serde_json::from_value(json!({"kind":"workflow","order_id":order.id,"base_revision":order.revision,"idempotency_key":"public-list-run","prompt":"Run the published list using selected inputs.","workflow_id":entry.id,"version_id":entry.versions[0].id,"access_mode":"read_only","budgets":run_budgets,"inputs":{"values":values,"private_note":"UNBOUND_LIST_PRIVATE_MARKER"}})).unwrap();
        let runtime = Runtime::new(
            store,
            vec![ConfiguredProvider {
                profile,
                credential_env: None,
            }],
            false,
        )
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&temp.path().join("broker"), vec![]).unwrap(),
        );
        Self {
            temp,
            runtime,
            project,
            input,
            definition,
            model,
            server,
        }
    }
    async fn prepare(&self) -> PreparedExecutionPlanV2 {
        let workspace = self.runtime.workspace(&self.input.order_id).await.unwrap();
        let request = self.input.clone();
        self.runtime
            .preflight_workflow(&request)
            .await
            .expect("public list preflight");
        let selected = self
            .runtime
            .prepare_execution_input(
                &self.input.order_id,
                &PrepareExecutionInput {
                    idempotency_key: "original-list-input".into(),
                    base_revision: self.input.base_revision,
                    folder_ids: vec![workspace.folder.id],
                },
            )
            .await
            .unwrap();
        self.runtime
            .prepare_execution_plan_v2(&PrepareOwnerExecutionPlanV2 {
                target: OwnerExecutionTargetV2::BrokerController {},
                request: RunRequest::Workflow(request),
                input: ExecutionInputRef {
                    snapshot_id: selected.snapshot.id,
                    sha256: selected.sha256,
                    manifest_sha256: selected.snapshot.manifest.sha256,
                },
            })
            .await
            .unwrap()
    }
    async fn start(&self) -> (RunRecord, PreparedExecutionPlanV2) {
        let prepared = self.prepare().await;
        assert!(self.model.requests.lock().unwrap().is_empty());
        let run = self
            .runtime
            .commit_execution_plan_v2(
                &self.input.order_id,
                &prepared.plan.id,
                &CommitExecutionPlan {
                    sha256: prepared.sha256.clone(),
                },
            )
            .await
            .unwrap();
        assert!(run.spec.input_workspace.is_some());
        (run, prepared)
    }
    async fn raw(&self) -> sqlx::SqlitePool {
        sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(self.temp.path().join("iterations.sqlite3")),
        )
        .await
        .unwrap()
    }
    async fn reopen(&self) -> Runtime {
        let store = Store::sqlite(&self.temp.path().join("iterations.sqlite3"))
            .await
            .unwrap();
        Runtime::new(store, vec![], false)
            .unwrap()
            .with_file_broker(
                cowork_broker::FileBroker::open(&self.temp.path().join("broker"), vec![]).unwrap(),
            )
    }
}
fn output(workflow: &cowork_storage::StoredWorkflow, id: &str) -> Value {
    let WorkflowNodeState::Completed { output, .. } = &workflow.progress.nodes()[id].state else {
        panic!()
    };
    serde_json::from_str(output).unwrap()
}

#[tokio::test]
async fn published_list_uses_one_plan_copy_and_three_real_typed_model_requests() {
    let f = Fixture::new(definition(true), json!([7, -3, 7]), false, true).await;
    let (run, prepared) = f.start().await;
    let frozen = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert!(frozen.plan.iteration_definition);
    assert_eq!(frozen.plan.source_sha256, canonical_hash(&f.definition));
    assert_eq!(frozen.plan.roles.len(), 1);
    assert_eq!(frozen.plan.profiles.len(), 1);
    let complete = f
        .runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(complete.state, RunState::Completed, "{complete:?}");
    assert_eq!(complete.spec.input_workspace, run.spec.input_workspace);
    let requests = f.model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    assert_eq!(f.model.maximum.load(Ordering::SeqCst), 2);
    assert!(
        !serde_json::to_string(&requests)
            .unwrap()
            .contains("UNBOUND_LIST_PRIVATE_MARKER")
    );
    let mut values: Vec<_> = requests
        .iter()
        .map(|body| {
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|message| message["content"].as_str())
                .filter_map(|text| serde_json::from_str::<Value>(text).ok())
                .find(|frame| frame["workflow_step"].is_string())
                .unwrap()["inputs"]["amount"]
                .as_i64()
                .unwrap()
        })
        .collect();
    values.sort();
    assert_eq!(values, vec![-3, 7, 7]);
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(stored.plan.sha256, frozen.plan.sha256);
    assert_eq!(stored.progress.iteration_manifests()["loop"].items.len(), 3);
    assert_eq!(output(&stored, "after"), json!({"results":[14,-6,14]}));
    assert_eq!(complete.model_attempts.len(), 3);
    assert_eq!(stored.progress.charged_tokens(), 396);
    assert_eq!(stored.progress.attempts().len(), 8);
    let view = f
        .runtime
        .store
        .workflow_run_view(&f.project.id, &run.id)
        .await
        .unwrap();
    let groups: Vec<_> = view
        .nodes
        .iter()
        .filter(|node| {
            node.scope_id.as_deref() == Some("loop")
                && node.kind == Some(cowork_workflow_contracts::WorkflowNodeKind::Subflow)
        })
        .map(|node| node.id.as_str())
        .collect();
    assert_eq!(
        groups,
        stored.progress.iteration_manifests()["loop"]
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>()
    );
    stored.validate(&complete).unwrap();
    for node in stored.agents.keys() {
        assert_eq!(
            f.runtime
                .store
                .workflow_node_spec(&f.project.id, &run.id, node)
                .await
                .unwrap()
                .input_workspace,
            run.spec.input_workspace
        );
    }
    let replay = f
        .runtime
        .commit_execution_plan_v2(
            &f.input.order_id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap();
    assert_eq!(replay.id, run.id);
    assert_eq!(f.model.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn a_bad_item_result_stops_the_list_without_dropping_it_or_sending_the_next_item() {
    let f = Fixture::new(definition(true), json!([7, -3, 7]), true, false).await;
    let (run, _) = f.start().await;
    let complete = f
        .runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(complete.state, RunState::Failed);
    assert_eq!(f.model.requests.lock().unwrap().len(), 2);
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert!(matches!(
        stored.progress.nodes()["loop"].state,
        WorkflowNodeState::Blocked
    ));
    assert!(matches!(
        stored.progress.nodes()["after"].state,
        WorkflowNodeState::Blocked
    ));
    assert_eq!(stored.progress.iteration_manifests()["loop"].items.len(), 3);
    stored.validate(&complete).unwrap();
}

#[tokio::test]
async fn unconfirmed_list_model_answer_survives_reopen_without_another_request() {
    let mut definition = definition(true);
    definition.iterations.get_mut("loop").unwrap().max_parallel = 1;
    let f = Fixture::new(definition, json!([7, -3]), false, false).await;
    let (run, _) = f.start().await;
    let db = f.raw().await;
    sqlx::query("CREATE TRIGGER reject_list_answer BEFORE INSERT ON events WHEN json_extract(NEW.payload,'$.kind')='workflow_model_completed' BEGIN SELECT RAISE(ABORT,'owned lost list answer'); END").execute(&db).await.unwrap();
    assert!(
        f.runtime
            .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(f.model.requests.lock().unwrap().len(), 1);
    let old = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let turn = old
        .agents
        .values()
        .flat_map(|agent| &agent.turns)
        .next()
        .unwrap()
        .clone();
    assert!(turn.response.is_none());
    assert_eq!(old.progress.charged_tokens(), 16384);
    db.close().await;
    f.runtime.store.close().await;
    let next = f.reopen().await;
    let reopened = next
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&reopened), canonical_hash(&old));
    assert!(matches!(
        next.execute_workflow_model_turn(
            &f.project.id,
            &run.id,
            "no-new-lease",
            &turn.claim,
            &turn.id,
            &CancellationToken::new()
        )
        .await,
        Err(Error::UncertainEffect)
    ));
    assert!(
        next.run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(f.model.requests.lock().unwrap().len(), 1);
    next.store.close().await;
}

#[tokio::test]
async fn public_list_wait_reopens_with_the_original_capture_deadline_and_wake() {
    let mut definition = definition(false);
    let iteration = definition.iterations.get_mut("loop").unwrap();
    iteration.max_parallel = 1;
    iteration
        .definition
        .controls
        .get_mut("echo")
        .unwrap()
        .operation = WorkflowControlOperation::Wait {
        seconds_input: "amount".into(),
    };
    iteration
        .definition
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("echo")
        .unwrap()
        .output = wait_output_type();
    iteration.result.pointer = "/wake_at".into();
    let f = Fixture::new(definition, json!([2, 3]), false, false).await;
    let (run, prepared) = f.start().await;
    let waiting = f
        .runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(waiting.state, RunState::WaitingForTimer);
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let first_wake = stored.progress.next_wake_at().unwrap();
    let loop_start = stored.progress.nodes()["loop"].started_at;
    let manifest = canonical_hash(&stored.progress.iteration_manifests()["loop"]);
    f.runtime.store.close().await;
    let next = f.reopen().await;
    let original = next
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(original.progress.next_wake_at(), Some(first_wake));
    assert_eq!(original.progress.nodes()["loop"].started_at, loop_start);
    let mut wakes = vec![];
    loop {
        let graph = next
            .store
            .stored_workflow(&f.project.id, &run.id)
            .await
            .unwrap();
        let Some(wake) = graph.progress.next_wake_at() else {
            break;
        };
        wakes.push(wake);
        let remaining = (wake * 1000).saturating_sub(cowork_storage::now());
        if remaining > 0 {
            tokio::time::sleep(Duration::from_millis(remaining as u64 + 40)).await;
        }
        assert_eq!(next.store.wake_workflow_timers().await.unwrap(), 1);
        let current = next
            .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(current.started_at, waiting.started_at);
        assert!(wakes.len() <= 2);
        if current.state == RunState::Completed {
            break;
        }
        assert_eq!(current.state, RunState::WaitingForTimer);
    }
    let final_run = next.store.stored_run(&run.id).await.unwrap().record;
    assert_eq!(final_run.state, RunState::Completed);
    let final_graph = next
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(
        manifest,
        canonical_hash(&final_graph.progress.iteration_manifests()["loop"])
    );
    assert_eq!(final_graph.progress.nodes()["loop"].started_at, loop_start);
    assert_eq!(output(&final_graph, "loop"), json!(wakes));
    assert_eq!(final_run.model_attempts.len(), 0);
    assert_eq!(final_graph.progress.charged_tokens(), 0);
    assert_eq!(final_graph.progress.attempts().len(), 6);
    let replay = next
        .commit_execution_plan_v2(
            &f.input.order_id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap();
    assert_eq!(replay.id, run.id);
    assert!(f.model.requests.lock().unwrap().is_empty());
    next.store.close().await;
}
