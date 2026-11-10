use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use cowork_contracts::*;
use cowork_domain::{Error, canonical_hash, workflows::*};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::{Store, StoredWorkflow};
use cowork_workflow_contracts::{
    CommitExecutionPlan, OwnerExecutionTargetV2, PrepareOwnerExecutionPlanV2,
    PreparedExecutionPlanV2, RunRequest, WorkflowRunRequest,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct Model {
    requests: Arc<Mutex<Vec<Value>>>,
    expected: Arc<Mutex<BTreeMap<i64, (String, String)>>>,
    foreign: Arc<Mutex<Option<String>>>,
    probes: Arc<AtomicUsize>,
}
async fn models(State(model): State<Model>) -> Json<Value> {
    model.probes.fetch_add(1, Ordering::SeqCst);
    Json(json!({"data":[{"id":"workspace-fixture"}]}))
}
async fn chat(State(model): State<Model>, Json(body): Json<Value>) -> String {
    model.requests.lock().unwrap().push(body.clone());
    let messages = body["messages"].as_array().unwrap();
    let frame = messages
        .iter()
        .filter_map(|m| m["content"].as_str())
        .filter_map(|s| serde_json::from_str::<Value>(s).ok())
        .find(|v| v["workflow_step"].is_string())
        .unwrap();
    let amount = frame["inputs"]["amount"].as_i64().unwrap();
    let (order, expected) = model.expected.lock().unwrap()[&amount].clone();
    let encoded = messages
        .iter()
        .filter_map(|message| message["content"].as_str())
        .find_map(|content| {
            content
                .split_once(
                    "Selected folder capabilities (use these IDs; no other folders are granted): ",
                )
                .map(|(_, value)| value)
        })
        .unwrap_or_else(|| panic!("Missing selected folder metadata in fixture request: {body}"));
    let folders: Vec<FolderGrant> = serde_json::Deserializer::from_str(encoded)
        .into_iter::<Vec<FolderGrant>>()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].order_id.as_deref(), Some(order.as_str()));
    assert!(
        body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["function"]["name"] == "read_text_file")
    );
    let foreign = model.foreign.lock().unwrap().clone();
    let (delta, finish) = if let Some(message) = messages.iter().rev().find(|m| m["role"] == "tool")
    {
        let receipt: Value = serde_json::from_str(message["content"].as_str().unwrap()).unwrap();
        if foreign.is_none() {
            assert_eq!(receipt["content"], expected, "{receipt}");
        } else {
            assert!(
                !message["content"]
                    .as_str()
                    .unwrap()
                    .contains("FOREIGN_PRIVATE_CONTENT")
            );
        }
        (
            json!({"content":json!({"amount":amount}).to_string()}),
            "stop",
        )
    } else {
        let folder = foreign.unwrap_or_else(|| folders[0].id.clone());
        (
            json!({"content":"Read selected file.","tool_calls":[{"index":0,"id":format!("read-{amount}"),"type":"function","function":{"name":"read_text_file","arguments":json!({"folder_id":folder,"path":"note.txt"}).to_string()}}]}),
            "tool_calls",
        )
    };
    let response = json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}],"usage":{"prompt_tokens":10,"completion_tokens":4}});
    format!("data: {response}\n\ndata: [DONE]\n\n")
}
fn definition() -> WorkflowDefinition {
    let mut d: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-iteration.json"
    ))
    .unwrap();
    d.budgets.max_seconds = 180;
    d.budgets.max_total_tokens = 131072;
    d.rights = WorkflowRights {
        current_workspace: true,
        tools: ["read_text_file".into()].into_iter().collect(),
        folders: Default::default(),
    };
    let call = d.iterations.get_mut("loop").unwrap();
    call.rights = d.rights.clone();
    call.budgets = d.budgets.clone();
    let body = &mut call.definition;
    body.rights = d.rights.clone();
    body.budgets = d.budgets.clone();
    body.controls.clear();
    body.roles.push(serde_json::from_value(json!({"id":"worker","instructions":"Read note.txt using the selected folder capability, then return the explicit amount.","provider_id":"model","provider_revision":1,"rights":body.rights,"budgets":body.budgets,"delegate_to":[]})).unwrap());
    body.steps.push(WorkflowStep {
        id: "echo".into(),
        role_id: "worker".into(),
        goal: "Read the current task workspace".into(),
        dependencies: vec![],
        rights: body.rights.clone(),
        budgets: body.budgets.clone(),
        execution: None,
        zero_context: false,
    });
    d
}
struct Fixture {
    temp: tempfile::TempDir,
    runtime: Runtime,
    project: Project,
    workflow: String,
    version: String,
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
    async fn new() -> Self {
        let model = Model::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/models", get(models))
            .route("/chat/completions", post(chat))
            .with_state(model.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let temp = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&temp.path().join("resources.sqlite3"))
            .await
            .unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Portable workflow resources".into(),
            })
            .await
            .unwrap();
        let profile:ProviderProfile=serde_json::from_value(json!({"id":"model","name":"Loopback workspace fixture","kind":"openai_compatible","endpoint":endpoint,"model":"workspace-fixture","revision":1,"context_tokens":16384,"credential_ready":true,"credential_required":false})).unwrap();
        store.insert_provider(&profile).await.unwrap();
        let definition = definition();
        let entry = store
            .create_workflow(&project.id, "portable-source", &definition)
            .await
            .unwrap();
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
            workflow: entry.id,
            version: entry.versions[0].id.clone(),
            definition,
            model,
            server,
        }
    }
    async fn order(&self, name: &str) -> Order {
        self.runtime
            .store
            .create_order(CreateOrder {
                project_id: self.project.id.clone(),
                title: name.into(),
            })
            .await
            .unwrap()
    }
    fn request(&self, order: &Order, values: Value) -> WorkflowRunRequest {
        let mut budgets = self.definition.budgets.clone();
        budgets.max_steps = 32;
        serde_json::from_value(json!({"kind":"workflow","order_id":order.id,"base_revision":order.revision,"idempotency_key":format!("portable-{}",order.id),"prompt":"Read this task's frozen selected workspace.","workflow_id":self.workflow,"version_id":self.version,"access_mode":"read_only","budgets":budgets,"inputs":{"values":values}})).unwrap()
    }
    fn source_file(&self, order: &Order) -> std::path::PathBuf {
        let directory = self
            .temp
            .path()
            .join("broker/workspaces")
            .join(&order.id)
            .join("files")
            .canonicalize()
            .unwrap();
        assert!(directory.starts_with(self.temp.path().canonicalize().unwrap()));
        directory.join("note.txt")
    }
    async fn prepare(
        &self,
        order: &Order,
        values: Value,
    ) -> (WorkflowRunRequest, PreparedExecutionPlanV2) {
        let workspace = self.runtime.workspace(&order.id).await.unwrap();
        let input = self
            .runtime
            .prepare_execution_input(
                &order.id,
                &PrepareExecutionInput {
                    idempotency_key: format!("select-{}", order.id),
                    base_revision: order.revision,
                    folder_ids: vec![workspace.folder.id],
                },
            )
            .await
            .unwrap();
        let request = self.request(order, values);
        let plan = self
            .runtime
            .prepare_execution_plan_v2(&PrepareOwnerExecutionPlanV2 {
                target: OwnerExecutionTargetV2::BrokerController {},
                request: RunRequest::Workflow(request.clone()),
                input: ExecutionInputRef {
                    snapshot_id: input.snapshot.id,
                    sha256: input.sha256,
                    manifest_sha256: input.snapshot.manifest.sha256,
                },
            })
            .await
            .unwrap();
        (request, plan)
    }
    async fn commit(&self, order: &Order, plan: &PreparedExecutionPlanV2) -> RunRecord {
        self.runtime
            .commit_execution_plan_v2(
                &order.id,
                &plan.plan.id,
                &CommitExecutionPlan {
                    sha256: plan.sha256.clone(),
                },
            )
            .await
            .unwrap()
    }
}
fn result(workflow: &StoredWorkflow) -> Value {
    let WorkflowNodeState::Completed { output, .. } = &workflow.progress.nodes()["after"].state
    else {
        panic!()
    };
    serde_json::from_str(output).unwrap()
}

#[tokio::test]
async fn one_published_source_reads_each_current_snapshot_with_real_scoped_file_tools() {
    let f = Fixture::new().await;
    let source_hash = canonical_hash(&f.definition);
    let mut plan_hashes = vec![];
    for (name, values, text) in [
        ("First task", vec![3, 5], "FIRST_FROZEN_CONTENT"),
        ("Second task", vec![7, 9], "SECOND_FROZEN_CONTENT"),
    ] {
        let order = f.order(name).await;
        let workspace = f.runtime.workspace(&order.id).await.unwrap();
        std::fs::write(f.source_file(&order), text).unwrap();
        for value in &values {
            f.model
                .expected
                .lock()
                .unwrap()
                .insert(*value, (order.id.clone(), text.into()));
        }
        let (_, plan) = f.prepare(&order, json!(values)).await;
        // Only the selected immutable input may reach tools, even when the
        // original managed source changes before confirmation and execution.
        std::fs::write(f.source_file(&order), "LATER_SOURCE_CONTENT").unwrap();
        let run = f.commit(&order, &plan).await;
        let frozen = f
            .runtime
            .store
            .stored_workflow(&f.project.id, &run.id)
            .await
            .unwrap();
        let granted = run
            .spec
            .folders
            .iter()
            .find(|grant| grant.order_id.as_deref() == Some(order.id.as_str()))
            .unwrap();
        assert_eq!(
            frozen.plan.scope.workspace_folder.as_ref(),
            Some(&WorkflowWorkspaceBinding::from_grant(granted).unwrap())
        );
        assert_eq!(granted.id, workspace.folder.id);
        assert_ne!(granted.resource_id, workspace.folder.resource_id);
        assert_eq!(frozen.plan.source_sha256, source_hash);
        plan_hashes.push(frozen.plan.sha256.clone());
        let complete = f
            .runtime
            .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(complete.state, RunState::Completed, "{complete:?}");
        let stored = f
            .runtime
            .store
            .stored_workflow(&f.project.id, &run.id)
            .await
            .unwrap();
        assert_eq!(result(&stored), json!({"results":values}));
        stored.validate(&complete).unwrap();
        let mut receipts = 0;
        for (node, agent) in &stored.agents {
            let role = f
                .runtime
                .store
                .workflow_node_spec(&f.project.id, &run.id, node)
                .await
                .unwrap();
            assert_eq!(role.input_workspace, run.spec.input_workspace);
            assert_eq!(role.folders.len(), 1);
            assert_eq!(role.folders[0].id, workspace.folder.id);
            for receipt in agent.turns.iter().flat_map(|turn| &turn.tool_results) {
                assert!(receipt.succeeded);
                assert_eq!(receipt.result["content"], text);
                receipts += 1;
            }
        }
        assert_eq!(receipts, 2);
        assert_eq!(complete.model_attempts.len(), 4);
        let before = f.model.requests.lock().unwrap().len();
        let replay = f.commit(&order, &plan).await;
        assert_eq!(replay.id, run.id);
        assert_eq!(f.model.requests.lock().unwrap().len(), before);
        assert_eq!(
            std::fs::read_to_string(f.source_file(&order)).unwrap(),
            "LATER_SOURCE_CONTENT"
        );
    }
    assert_ne!(plan_hashes[0], plan_hashes[1]);
    assert_eq!(f.model.requests.lock().unwrap().len(), 8);
    assert_eq!(
        canonical_hash(
            &f.runtime
                .store
                .workflow_version(&f.project.id, &f.workflow, &f.version)
                .await
                .unwrap()
                .definition
        ),
        source_hash
    );
}

#[tokio::test]
async fn a_missing_current_workspace_fails_before_provider_probes_or_run_creation() {
    let f = Fixture::new().await;
    let order = f.order("No registered workspace").await;
    let request = f.request(&order, json!([1]));
    assert!(f.runtime.preflight_workflow(&request).await.is_err());
    assert_eq!(f.model.probes.load(Ordering::SeqCst), 0);
    assert!(f.model.requests.lock().unwrap().is_empty());
    assert!(f.runtime.store.runs(&order.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_portable_selection_never_allows_another_orders_managed_folder() {
    let f = Fixture::new().await;
    let order = f.order("Selected task").await;
    let other = f.order("Foreign task").await;
    let selected = f.runtime.workspace(&order.id).await.unwrap();
    let foreign = f.runtime.workspace(&other.id).await.unwrap();
    std::fs::write(f.source_file(&order), "SELECTED_CONTENT").unwrap();
    std::fs::write(f.source_file(&other), "FOREIGN_PRIVATE_CONTENT").unwrap();
    f.model
        .expected
        .lock()
        .unwrap()
        .insert(1, (order.id.clone(), "SELECTED_CONTENT".into()));
    let mut illegal = f.request(&order, json!([1]));
    illegal.folders = vec![foreign.folder.clone()];
    assert!(f.runtime.preflight_workflow(&illegal).await.is_err());
    assert!(f.model.requests.lock().unwrap().is_empty());
    *f.model.foreign.lock().unwrap() = Some(foreign.folder.id.clone());
    let (_, plan) = f.prepare(&order, json!([1])).await;
    let run = f.commit(&order, &plan).await;
    assert!(matches!(
        f.runtime
            .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
            .await,
        Err(Error::Forbidden)
    ));
    let stopped = f.runtime.store.stored_run(&run.id).await.unwrap().record;
    assert_eq!(stopped.state, RunState::Interrupted);
    assert_eq!(stopped.error_code.as_deref(), Some("forbidden"));
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert!(
        stored
            .agents
            .values()
            .flat_map(|agent| &agent.turns)
            .flat_map(|turn| &turn.tool_results)
            .next()
            .is_none()
    );
    assert!(
        stored
            .progress
            .attempts()
            .values()
            .all(|attempt| !matches!(attempt.operation, WorkflowOperation::Tool { .. }))
    );
    assert_eq!(f.model.requests.lock().unwrap().len(), 1);
    assert!(
        stored
            .agents
            .keys()
            .all(|node| stored.progress.nodes()[node].plan.rights.folders
                == [selected.folder.id.clone()].into_iter().collect())
    );
    assert!(
        !serde_json::to_string(&*f.model.requests.lock().unwrap())
            .unwrap()
            .contains("FOREIGN_PRIVATE_CONTENT")
    );
    assert_eq!(
        std::fs::read_to_string(f.source_file(&other)).unwrap(),
        "FOREIGN_PRIVATE_CONTENT"
    );
}

#[tokio::test]
async fn rehashing_private_scope_metadata_cannot_replace_the_runs_actual_grant() {
    let f = Fixture::new().await;
    let order = f.order("Frozen authority").await;
    f.runtime.workspace(&order.id).await.unwrap();
    std::fs::write(f.source_file(&order), "ORIGINAL").unwrap();
    let (_, plan) = f.prepare(&order, json!([1])).await;
    let run = f.commit(&order, &plan).await;
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let mut edited = serde_json::to_value(stored).unwrap();
    edited["plan"]["scope"]["workspace_folder"]["order_id"] = json!("other-task");
    let mut hashable = edited["plan"].clone();
    hashable["sha256"] = json!("");
    let sha = canonical_hash(&hashable);
    edited["plan"]["sha256"] = json!(sha);
    edited["binding"]["plan_sha256"] = json!(sha);
    edited["progress"]["plan_sha256"] = json!(sha);
    let forged: StoredWorkflow = serde_json::from_value(edited).unwrap();
    let mut record = run;
    record.spec.workflow = Some(forged.binding.clone());
    forged.plan.validate().unwrap();
    forged.progress.validate(&forged.plan).unwrap();
    assert!(forged.validate(&record).is_err());
    assert!(f.model.requests.lock().unwrap().is_empty());
}
