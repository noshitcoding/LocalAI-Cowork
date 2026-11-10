use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use cowork_contracts::*;
use cowork_domain::{canonical_hash, workflows::*};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use cowork_workflow_contracts::{CommitExecutionPlan, RunRequest, WorkflowRunRequest};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct Model {
    requests: Arc<Mutex<Vec<Value>>>,
    probes: Arc<AtomicUsize>,
    malformed: bool,
}
async fn models(State(model): State<Model>) -> Json<Value> {
    model.probes.fetch_add(1, Ordering::SeqCst);
    Json(json!({"data":[{"id":"typed-fixture-model"}]}))
}
async fn chat(State(model): State<Model>, Json(body): Json<Value>) -> String {
    model.requests.lock().unwrap().push(body.clone());
    let frame = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .find(|value| value["workflow_step"].is_string())
        .unwrap();
    let output = if frame["workflow_step"] == "extract"
        || frame["output_schema"]["properties"]["invoice_id"].is_object()
    {
        if model.malformed {
            json!({"invoice_id":"INV-001","total":"42.5"})
        } else {
            json!({"invoice_id":"INV-001","total":42.5})
        }
    } else {
        json!({"summary":"INV-001: EUR 42.5"})
    };
    let reply = json!({"choices":[{"index":0,"delta":{"content":output.to_string()},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":32}});
    format!("data: {reply}\n\ndata: [DONE]\n\n")
}
struct Fixture {
    temp: tempfile::TempDir,
    runtime: Runtime,
    project: Project,
    input: WorkflowRunRequest,
    model: Model,
    server: tokio::task::JoinHandle<()>,
}

fn subflow_definition() -> WorkflowDefinition {
    let mut body: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-dataflow.json"
    ))
    .unwrap();
    let root_input = body.dataflow.as_ref().unwrap().input.clone();
    let WorkflowDataType::Object { fields } = &mut body.dataflow.as_mut().unwrap().input else {
        panic!()
    };
    fields.remove("private_note");
    serde_json::from_value(json!({
        "format_version":4,"name":"Frozen model subflow","mode":{"mode":"parallel","max_parallel":4},"failure":"stop",
        "rights":{"tools":[],"folders":[]},"budgets":body.budgets,"roles":[],"steps":[],
        "controls":{"after":{"name":"Use the returned summary","dependencies":["invoke"],"operation":{"kind":"compose"}}},
        "subflows":{"invoke":{"name":"Invoice summary copy","dependencies":[],"definition":body,"result":{"step_id":"report","pointer":""},"rights":{"tools":[],"folders":[]},"budgets":body.budgets}},
        "dataflow":{"input":root_input,"steps":{
            "invoke":{"inputs":{
                "path":{"value_type":{"type":"text"},"value":{"source":"input","pointer":"/path"}},
                "sha256":{"value_type":{"type":"text"},"value":{"source":"input","pointer":"/sha256"}},
                "currency":{"value_type":{"type":"text"},"value":{"source":"input","pointer":"/currency"}}
            },"output":{"type":"object","fields":{"summary":{"type":"text"}}}},
            "after":{"inputs":{"summary":{"value_type":{"type":"text"},"value":{"source":"step","step_id":"invoke","pointer":"/summary"}}},"output":{"type":"object","fields":{"summary":{"type":"text"}}}}
        }}
    })).unwrap()
}

async fn subflow_request(f: &Fixture) -> (WorkflowDefinition, WorkflowRunRequest) {
    let definition = subflow_definition();
    let entry = f
        .runtime
        .store
        .create_workflow(&f.project.id, "public-model-subflow", &definition)
        .await
        .unwrap();
    let mut request = f.input.clone();
    request.workflow_id = entry.id;
    request.version_id = entry.versions[0].id.clone();
    request.idempotency_key = "public-subflow-run".into();
    (definition, request)
}

#[tokio::test]
async fn a_published_subflow_resolves_nested_profiles_and_sends_only_bound_inputs() {
    let f = fixture(false).await;
    let (definition, input) = subflow_request(&f).await;
    let workspace = f.runtime.workspace(&input.order_id).await.unwrap();
    let selected = f
        .runtime
        .prepare_execution_input(
            &input.order_id,
            &PrepareExecutionInput {
                idempotency_key: "subflow-input-copy".into(),
                base_revision: input.base_revision,
                folder_ids: vec![workspace.folder.id.clone()],
            },
        )
        .await
        .unwrap();
    let prepared = f
        .runtime
        .prepare_execution_plan_v2(&cowork_workflow_contracts::PrepareOwnerExecutionPlanV2 {
            target: cowork_workflow_contracts::OwnerExecutionTargetV2::BrokerController {},
            request: RunRequest::Workflow(input.clone()),
            input: ExecutionInputRef {
                snapshot_id: selected.snapshot.id,
                sha256: selected.sha256,
                manifest_sha256: selected.snapshot.manifest.sha256,
            },
        })
        .await
        .unwrap();
    assert!(f.model.requests.lock().unwrap().is_empty());
    let run = f
        .runtime
        .commit_execution_plan_v2(
            &input.order_id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256.clone(),
            },
        )
        .await
        .unwrap();
    assert!(run.spec.input_workspace.is_some());
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(graph.plan.source_sha256, canonical_hash(&definition));
    assert_eq!(
        run.spec.workflow.as_ref().unwrap().definition_sha256,
        canonical_hash(&definition)
    );
    assert!(graph.plan.composition_definition);
    assert_eq!(graph.plan.profiles.len(), 1);
    let complete = f
        .runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(complete.state, RunState::Completed, "{complete:?}");
    let requests = f.model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert!(
        !serde_json::to_string(&requests)
            .unwrap()
            .contains("UNBOUND_MUST_NOT_ENTER_MODEL_CONTEXT")
    );
    let frames: Vec<Value> = requests
        .iter()
        .map(|request| {
            request["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|m| m["content"].as_str())
                .filter_map(|s| serde_json::from_str::<Value>(s).ok())
                .find(|v| v["workflow_step"].is_string())
                .unwrap()
        })
        .collect();
    assert_eq!(
        frames[0]["inputs"],
        json!({"file":"invoice.pdf","expected_sha256":cowork_domain::hash(b"fixture")})
    );
    assert_eq!(
        frames[1]["inputs"],
        json!({"amount":42.5,"invoice":"INV-001","currency":"EUR"})
    );
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let WorkflowNodeState::Completed { output, .. } = &graph.progress.nodes()["after"].state else {
        panic!()
    };
    assert_eq!(
        serde_json::from_str::<Value>(output).unwrap(),
        json!({"summary":"INV-001: EUR 42.5"})
    );
    assert_eq!(graph.progress.attempts().len(), 4);
    assert_eq!(complete.model_attempts.len(), 2);
    let view = f
        .runtime
        .store
        .workflow_run_view(&f.project.id, &run.id)
        .await
        .unwrap();
    assert!(view.nodes.iter().any(|node| node.id == "invoke"
        && node.kind == Some(cowork_workflow_contracts::WorkflowNodeKind::Subflow)));
    assert!(
        view.nodes
            .iter()
            .filter(|node| node.role_id.is_some())
            .all(|node| node.scope_id.as_deref() == Some("invoke"))
    );
    let replay = f
        .runtime
        .commit_execution_plan_v2(
            &input.order_id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap();
    assert_eq!(replay.id, run.id);
    assert_eq!(f.model.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_malformed_nested_model_result_never_reaches_the_next_model_or_return() {
    let f = fixture(true).await;
    let (_, input) = subflow_request(&f).await;
    let prepared = f
        .runtime
        .prepare_execution_plan(&RunRequest::Workflow(input.clone()))
        .await
        .unwrap();
    let run = f
        .runtime
        .commit_execution_plan(
            &input.order_id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap();
    let complete = f
        .runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(complete.state, RunState::Failed, "{complete:?}");
    assert_eq!(f.model.requests.lock().unwrap().len(), 1);
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert!(matches!(
        graph.progress.nodes()["invoke"].state,
        WorkflowNodeState::Blocked
    ));
    assert!(matches!(
        graph.progress.nodes()["after"].state,
        WorkflowNodeState::Blocked
    ));
}

#[tokio::test]
async fn a_subflow_step_limit_stops_the_second_model_with_parent_capacity_remaining() {
    let f = fixture(false).await;
    let mut definition = subflow_definition();
    definition
        .subflows
        .get_mut("invoke")
        .unwrap()
        .budgets
        .max_steps = 1;
    let entry = f
        .runtime
        .store
        .create_workflow(&f.project.id, "limited-subflow", &definition)
        .await
        .unwrap();
    let mut input = f.input.clone();
    input.workflow_id = entry.id;
    input.version_id = entry.versions[0].id.clone();
    input.idempotency_key = "limited-subflow-run".into();
    let prepared = f
        .runtime
        .prepare_execution_plan(&RunRequest::Workflow(input.clone()))
        .await
        .unwrap();
    let run = f
        .runtime
        .commit_execution_plan(
            &input.order_id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap();
    let complete = f
        .runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(complete.state, RunState::Failed, "{complete:?}");
    assert_eq!(f.model.requests.lock().unwrap().len(), 1);
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert!(graph.progress.attempts().len() < complete.spec.budgets.max_steps as usize);
    assert!(matches!(
        graph.progress.nodes()["invoke"].state,
        WorkflowNodeState::Blocked
    ));
    assert!(matches!(
        graph.progress.nodes()["after"].state,
        WorkflowNodeState::Blocked
    ));
}

#[tokio::test]
async fn a_native_subflow_wait_survives_store_reopen_with_visible_scope_and_original_source() {
    let f = fixture(false).await;
    let mut definition: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-subflow.json"
    ))
    .unwrap();
    definition.rights = WorkflowRights::default();
    let call = definition.subflows.get_mut("invoke").unwrap();
    call.rights = WorkflowRights::default();
    call.definition.rights = WorkflowRights::default();
    call.definition.controls.get_mut("echo").unwrap().operation = WorkflowControlOperation::Wait {
        seconds_input: "amount".into(),
    };
    call.definition
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("echo")
        .unwrap()
        .output = WorkflowDataType::Object {
        fields: std::collections::BTreeMap::from([("wake_at".into(), WorkflowDataType::Integer)]),
    };
    call.result.pointer = "/wake_at".into();
    definition
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("invoke")
        .unwrap()
        .output = WorkflowDataType::Integer;
    definition
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("after")
        .unwrap()
        .inputs
        .get_mut("final")
        .unwrap()
        .value = WorkflowDataSource::Step {
        step_id: "invoke".into(),
        pointer: "".into(),
    };
    let runtime = Runtime::new(f.runtime.store.clone(), vec![], false)
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&f.temp.path().join("subflow-native-broker"), vec![])
                .unwrap(),
        );
    let entry = runtime
        .store
        .create_workflow(&f.project.id, "native-wait-subflow", &definition)
        .await
        .unwrap();
    let mut input = f.input.clone();
    input.workflow_id = entry.id;
    input.version_id = entry.versions[0].id.clone();
    input.idempotency_key = "native-subflow-run".into();
    input.inputs = Some(Box::new(
        json!({"amount":2,"private_note":"UNBOUND_NATIVE_SECRET"}),
    ));
    let prepared = runtime
        .prepare_execution_plan(&RunRequest::Workflow(input.clone()))
        .await
        .unwrap();
    assert!(
        matches!(&prepared.plan.preflight,cowork_workflow_contracts::RunPreflight::Workflow(preflight) if preflight.ready&&preflight.roles.is_empty())
    );
    let run = runtime
        .commit_execution_plan(
            &input.order_id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256.clone(),
            },
        )
        .await
        .unwrap();
    let waiting = runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(waiting.state, RunState::WaitingForTimer);
    let view = runtime
        .store
        .workflow_run_view(&f.project.id, &run.id)
        .await
        .unwrap();
    let scope = view.nodes.iter().find(|node| node.id == "invoke").unwrap();
    assert_eq!(
        scope.state,
        cowork_workflow_contracts::WorkflowNodeStatus::WaitingTimer
    );
    let wake = scope.wake_at.unwrap();
    assert_eq!(view.steps_used, 1);
    assert_eq!(view.charged_tokens, 0);
    let reopened = Store::sqlite(&f.temp.path().join("typed.sqlite3"))
        .await
        .unwrap();
    let next = Runtime::new(reopened, vec![], false)
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&f.temp.path().join("subflow-native-broker"), vec![])
                .unwrap(),
        );
    let stored = next
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(stored.plan.source_sha256, canonical_hash(&definition));
    assert_eq!(stored.progress.next_wake_at(), Some(wake));
    let before = next
        .store
        .workflow_run_view(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(before.run.started_at, waiting.started_at);
    assert_eq!(
        before
            .nodes
            .iter()
            .find(|node| node.id == "invoke")
            .unwrap()
            .wake_at,
        Some(wake)
    );
    let remaining = (wake * 1000).saturating_sub(cowork_storage::now());
    if remaining > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(remaining as u64 + 40)).await;
    }
    assert_eq!(next.store.wake_workflow_timers().await.unwrap(), 1);
    let complete = next
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(complete.state, RunState::Completed, "{complete:?}");
    let stored = next
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let WorkflowNodeState::Completed { output, .. } = &stored.progress.nodes()["after"].state
    else {
        panic!()
    };
    assert_eq!(
        serde_json::from_str::<Value>(output).unwrap(),
        json!({"final":wake})
    );
    assert_eq!(stored.progress.attempts().len(), 3);
    let replay = next
        .commit_execution_plan(
            &input.order_id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap();
    assert_eq!(replay.id, run.id);
    assert!(f.model.requests.lock().unwrap().is_empty());
    assert_eq!(f.model.probes.load(Ordering::SeqCst), 0);
    next.store.close().await;
}

#[tokio::test]
async fn a_model_after_a_durable_wait_receives_only_the_original_typed_wake_result() {
    let f = fixture(false).await;
    let old: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-dataflow.json"
    ))
    .unwrap();
    let mut d: WorkflowDefinition =
        serde_json::from_str(include_str!("../../../tests/fixtures/workflow-wait.json")).unwrap();
    d.roles = vec![old.roles[0].clone()];
    d.controls.remove("after");
    d.dataflow.as_mut().unwrap().steps.remove("after");
    d.steps.push(WorkflowStep {
        execution: None,
        zero_context: false,
        id: "report".into(),
        role_id: d.roles[0].id.clone(),
        goal: "Use the declared scheduled_at value.".into(),
        dependencies: vec!["wait".into()],
        rights: WorkflowRights::default(),
        budgets: d.budgets.clone(),
    });
    d.dataflow.as_mut().unwrap().steps.insert(
        "report".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: std::collections::BTreeMap::from([(
                "scheduled_at".into(),
                WorkflowValueBinding {
                    value_type: WorkflowDataType::Integer,
                    value: WorkflowDataSource::Step {
                        step_id: "wait".into(),
                        pointer: "/wake_at".into(),
                    },
                },
            )]),
            output: WorkflowDataType::Object {
                fields: std::collections::BTreeMap::from([(
                    "summary".into(),
                    WorkflowDataType::Text,
                )]),
            },
        },
    );
    let entry = f
        .runtime
        .store
        .create_workflow(&f.project.id, "wait-before-model", &d)
        .await
        .unwrap();
    let mut input = f.input.clone();
    input.workflow_id = entry.id;
    input.version_id = entry.versions[0].id.clone();
    input.idempotency_key = "model-after-wait".into();
    input.inputs = Some(Box::new(json!({"delay":2})));
    input.budgets = d.budgets;
    let plan = f
        .runtime
        .prepare_execution_plan(&RunRequest::Workflow(input.clone()))
        .await
        .unwrap();
    let run = f
        .runtime
        .commit_execution_plan(
            &input.order_id,
            &plan.plan.id,
            &CommitExecutionPlan {
                sha256: plan.sha256,
            },
        )
        .await
        .unwrap();
    let waiting = f
        .runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(waiting.state, RunState::WaitingForTimer);
    assert!(f.model.requests.lock().unwrap().is_empty());
    let before = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let wake = before.progress.next_wake_at().unwrap();
    let remaining = (wake * 1000).saturating_sub(cowork_storage::now());
    if remaining > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(remaining as u64 + 40)).await;
    }
    assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 1);
    let complete = f
        .runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(complete.state, RunState::Completed, "{complete:?}");
    let requests = f.model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let frame = requests[0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["content"].as_str())
        .filter_map(|s| serde_json::from_str::<Value>(s).ok())
        .find(|v| v["workflow_step"] == "report")
        .unwrap();
    assert_eq!(frame["inputs"], json!({"scheduled_at":wake}));
}

#[tokio::test]
async fn mapped_event_drives_native_branches_without_a_configured_provider_or_replay() {
    let f = fixture(false).await;
    let definition: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-control.json"
    ))
    .unwrap();
    let library = f
        .runtime
        .store
        .create_workflow(&f.project.id, "native-event-flow", &definition)
        .await
        .unwrap();
    let runtime = Runtime::new(f.runtime.store.clone(), vec![], false)
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&f.temp.path().join("native-event-broker"), vec![])
                .unwrap(),
        );
    let task=runtime.store.write_workspace_task(&f.project.id,None,&serde_json::from_value(json!({
        "expected_revision":0,"idempotency_key":"native-event-task","value":{"name":"Native event task","description":"","order_id":f.input.order_id,
        "prompt":"Route declared values.","expected_output":"Chosen typed route","execution":{"kind":"workflow","workflow_id":library.id,"version_id":library.versions[0].id,"inputs":{"amount":150}},
        "access_mode":"read_only","budgets":definition.budgets}})).unwrap()).await.unwrap();
    let trigger=runtime.write_automation_trigger(&f.project.id,&serde_json::from_value(json!({
        "expected_revision":0,"idempotency_key":"native-trigger","name":"Native event","task_id":task.id,"task_revision":task.revision,
        "source":"manual","predicates":[],"enabled":true,"workflow_inputs":{"format_version":1,"bindings":{"/amount":"/invoice/total"}}})).unwrap()).await.unwrap();
    let input = SubmitAutomationEvent {
        event_id: "native-source-event".into(),
        data: json!({"invoice":{"total":25},"extra":"UNBOUND"}),
    };
    let original = runtime
        .store
        .submit_automation_event(
            &f.project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &input,
        )
        .await
        .unwrap();
    assert_eq!(runtime.automation_tick().await.unwrap(), 1);
    let events = runtime
        .store
        .automation_events(&f.project.id, &trigger.id, None, 50)
        .await
        .unwrap();
    let event = &events.items[0];
    assert_eq!(
        event.workflow_input.as_ref().unwrap().values.as_ref(),
        &json!({"amount":25})
    );
    let run = runtime
        .run_native_workflow(
            &f.project.id,
            event.run_id.as_ref().unwrap(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let graph = runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert!(matches!(
        graph.progress.nodes()["review"].state,
        WorkflowNodeState::Skipped { .. }
    ));
    assert!(graph.agents.is_empty());
    assert_eq!(graph.progress.charged_tokens(), 0);
    graph.validate(&run).unwrap();
    let duplicate = runtime
        .store
        .submit_automation_event(
            &f.project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &input,
        )
        .await
        .unwrap();
    assert_eq!(canonical_hash(&duplicate), canonical_hash(&original));
    assert_eq!(runtime.automation_tick().await.unwrap(), 0);
    assert!(f.model.requests.lock().unwrap().is_empty());
    assert_eq!(f.model.probes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unselected_model_branch_never_builds_a_model_turn_or_sends_a_completion() {
    let fixture = fixture(false).await;
    let old: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-dataflow.json"
    ))
    .unwrap();
    let mut definition: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-control.json"
    ))
    .unwrap();
    definition.roles = vec![old.roles[0].clone()];
    definition.steps.push(WorkflowStep {
        execution: None,
        zero_context: false,
        id: "report".into(),
        role_id: definition.roles[0].id.clone(),
        goal: "Only run when selected".into(),
        dependencies: vec!["large".into()],
        rights: WorkflowRights::default(),
        budgets: definition.budgets.clone(),
    });
    definition.gates.insert(
        "report".into(),
        WorkflowGate {
            step_id: "large".into(),
            when: false,
        },
    );
    definition.dataflow.as_mut().unwrap().steps.insert(
        "report".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: Default::default(),
            output: WorkflowDataType::Object {
                fields: std::collections::BTreeMap::from([(
                    "summary".into(),
                    WorkflowDataType::Text,
                )]),
            },
        },
    );
    let library = fixture
        .runtime
        .store
        .create_workflow(&fixture.project.id, "skip-model-branch", &definition)
        .await
        .unwrap();
    let mut input = fixture.input.clone();
    input.workflow_id = library.id;
    input.version_id = library.versions[0].id.clone();
    input.inputs = Some(Box::new(json!({"amount":150})));
    input.idempotency_key = "skip-model".into();
    input.budgets = definition.budgets;
    let prepared = fixture
        .runtime
        .prepare_execution_plan(&RunRequest::Workflow(input.clone()))
        .await
        .unwrap();
    let run = fixture
        .runtime
        .commit_execution_plan(
            &input.order_id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap();
    let result = fixture
        .runtime
        .run_native_workflow(&fixture.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.state, RunState::Completed, "{result:?}");
    let saved = fixture
        .runtime
        .store
        .stored_workflow(&fixture.project.id, &run.id)
        .await
        .unwrap();
    assert!(matches!(
        saved.progress.nodes()["report"].state,
        WorkflowNodeState::Skipped { .. }
    ));
    assert!(!saved.agents.contains_key("report"));
    assert!(fixture.model.requests.lock().unwrap().is_empty());
    assert_eq!(saved.progress.charged_tokens(), 0);
    saved.validate(&result).unwrap();
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn fixture(malformed: bool) -> Fixture {
    let model = Model {
        malformed,
        ..Default::default()
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
    let store = Store::sqlite(&temp.path().join("typed.sqlite3"))
        .await
        .unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Typed workflow fixture".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Typed invoice".into(),
        })
        .await
        .unwrap();
    let profile:ProviderProfile=serde_json::from_value(json!({"id":"typed-fixture","name":"Loopback fixture","kind":"openai_compatible","endpoint":endpoint,"model":"typed-fixture-model","revision":1,"context_tokens":16384,"credential_ready":true,"credential_required":false})).unwrap();
    store.insert_provider(&profile).await.unwrap();
    let definition: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-dataflow.json"
    ))
    .unwrap();
    let library = store
        .create_workflow(&project.id, "typed-library", &definition)
        .await
        .unwrap();
    let input:WorkflowRunRequest=serde_json::from_value(json!({"kind":"workflow","order_id":order.id,"base_revision":order.revision,"idempotency_key":"typed-run","prompt":"Run the configured typed demonstration.","workflow_id":library.id,"version_id":library.versions[0].id,"access_mode":"read_only","budgets":definition.budgets,"inputs":{"path":"invoice.pdf","sha256":cowork_domain::hash(b"fixture"),"currency":"EUR","private_note":"UNBOUND_MUST_NOT_ENTER_MODEL_CONTEXT"}})).unwrap();
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
    Fixture {
        temp,
        runtime,
        project,
        input,
        model,
        server,
    }
}

#[tokio::test]
async fn reviewed_input_is_frozen_and_two_real_provider_requests_bind_typed_results() {
    let fixture = fixture(false).await;
    let request = RunRequest::Workflow(fixture.input.clone());
    let prepared = fixture
        .runtime
        .prepare_execution_plan(&request)
        .await
        .unwrap();
    assert!(fixture.model.requests.lock().unwrap().is_empty());
    let mut changed = fixture.input.clone();
    changed.inputs.as_mut().unwrap()["currency"] = json!("USD");
    assert!(
        fixture
            .runtime
            .prepare_execution_plan(&RunRequest::Workflow(changed))
            .await
            .is_err()
    );
    let run = fixture
        .runtime
        .commit_execution_plan(
            &fixture.input.order_id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        run.spec.workflow.as_ref().unwrap().inputs,
        fixture.input.inputs
    );
    let result = fixture
        .runtime
        .run_native_workflow(&fixture.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.state, RunState::Completed, "{result:?}");
    let requests = fixture.model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    let frames: Vec<Value> = requests
        .iter()
        .map(|body| {
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|m| m["content"].as_str())
                .filter_map(|text| serde_json::from_str::<Value>(text).ok())
                .find(|value| value["workflow_step"].is_string())
                .unwrap()
        })
        .collect();
    assert_eq!(
        frames[0]["inputs"],
        json!({"file":"invoice.pdf","expected_sha256":cowork_domain::hash(b"fixture")})
    );
    assert_eq!(
        frames[1]["inputs"],
        json!({"amount":42.5,"invoice":"INV-001","currency":"EUR"})
    );
    assert!(
        requests
            .iter()
            .all(|body| body["response_format"]["type"] == "json_schema")
    );
    assert!(
        !serde_json::to_string(&requests)
            .unwrap()
            .contains("UNBOUND_MUST_NOT_ENTER_MODEL_CONTEXT")
    );
    let graph = fixture
        .runtime
        .store
        .stored_workflow(&fixture.project.id, &run.id)
        .await
        .unwrap();
    graph.validate(&result).unwrap();
    let mut altered = graph.clone();
    let agent = altered.agents.get_mut("report").unwrap();
    let frame = agent.seed.last_mut().unwrap();
    let mut payload: Value = serde_json::from_str(frame["content"].as_str().unwrap()).unwrap();
    payload["inputs"]["amount"] = json!(0);
    frame["content"] = json!(payload.to_string());
    agent.seed_sha256 = canonical_hash(&agent.seed);
    assert!(
        altered.validate(&result).is_err(),
        "Rehashing a forged typed seed does not make it valid"
    );
    let reopened = Store::sqlite(&fixture.temp.path().join("typed.sqlite3"))
        .await
        .unwrap();
    let recovered = reopened
        .stored_workflow(&fixture.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(recovered.binding.inputs, fixture.input.inputs);
    recovered.validate(&result).unwrap();
    reopened.close().await;
}

#[tokio::test]
async fn malformed_input_is_rejected_before_a_probe_or_run_is_admitted() {
    let fixture = fixture(false).await;
    for input in [
        None,
        Some(json!({"path":42})),
        Some(
            json!({"path":"invoice.pdf","sha256":"hash","currency":"EUR","private_note":"x","extra":true}),
        ),
    ] {
        let mut request = fixture.input.clone();
        request.inputs = input.map(Box::new);
        assert!(
            fixture
                .runtime
                .prepare_execution_plan(&RunRequest::Workflow(request))
                .await
                .is_err()
        );
    }
    assert_eq!(fixture.model.probes.load(Ordering::SeqCst), 0);
    assert!(fixture.model.requests.lock().unwrap().is_empty());
    assert!(
        fixture
            .runtime
            .store
            .existing_product_workflow_request(&fixture.input)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn wrong_model_output_never_reaches_a_downstream_model() {
    let fixture = fixture(true).await;
    let run = fixture
        .runtime
        .create_run_request(&RunRequest::Workflow(fixture.input.clone()))
        .await
        .unwrap();
    let result = fixture
        .runtime
        .run_native_workflow(&fixture.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.state, RunState::Failed, "{result:?}");
    assert_eq!(fixture.model.requests.lock().unwrap().len(), 1);
    let graph = fixture
        .runtime
        .store
        .stored_workflow(&fixture.project.id, &run.id)
        .await
        .unwrap();
    assert!(!matches!(
        graph.progress.nodes()["report"].state,
        WorkflowNodeState::Completed { .. }
    ));
    assert!(
        graph.agents.get("extract").unwrap().turns[0]
            .response
            .as_ref()
            .unwrap()
            .rejection_code
            .is_some()
    );
}

#[tokio::test]
async fn draft_input_values_survive_chat_configuration_restart_without_coercion() {
    let fixture = fixture(false).await;
    let write:WriteChatConfiguration=serde_json::from_value(json!({"expected_revision":0,"idempotency_key":"typed-draft","value":{"agent_config":{"instructions":"","generation":{}},"provider_id":null,"provider_revision":null,"access_mode":"read_only","budgets":fixture.input.budgets,"environment":null,"model_binding":null,"output":null,"workflow":{"workflow_id":fixture.input.workflow_id,"version_id":fixture.input.version_id,"inputs":{"amount":-42.5,"nullable":null,"text":"Unsubmitted value"}}}})).unwrap();
    let saved = fixture
        .runtime
        .store
        .write_chat_configuration(&fixture.input.order_id, &write)
        .await
        .unwrap();
    let reopened = Store::sqlite(&fixture.temp.path().join("typed.sqlite3"))
        .await
        .unwrap();
    let current = reopened
        .chat_configuration(&fixture.input.order_id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&saved), canonical_hash(&current));
    assert_eq!(
        current.value.unwrap().workflow.unwrap().inputs,
        write.value.workflow.unwrap().inputs
    );
    reopened.close().await;
}

#[tokio::test]
async fn mapped_manual_and_webhook_events_bind_only_selected_data_before_real_model_calls() {
    for source in [
        AutomationEventSource::Manual,
        AutomationEventSource::Webhook,
    ] {
        let fixture = fixture(false).await;
        let task=fixture.runtime.store.write_workspace_task(&fixture.project.id,None,&serde_json::from_value(json!({
            "expected_revision":0,"idempotency_key":"mapped-task","value":{"name":"Mapped task","description":"","order_id":fixture.input.order_id,
            "prompt":"Use declared workflow inputs.","expected_output":"A checked result","execution":{"kind":"workflow","workflow_id":fixture.input.workflow_id,"version_id":fixture.input.version_id,"inputs":fixture.input.inputs},
            "access_mode":"read_only","budgets":fixture.input.budgets}})).unwrap()).await.unwrap();
        let trigger=fixture.runtime.write_automation_trigger(&fixture.project.id,&serde_json::from_value(json!({
            "id":null,"expected_revision":0,"idempotency_key":"mapped-trigger","name":"Mapped event","task_id":task.id,"task_revision":task.revision,
            "source":source,"predicates":[],"enabled":true,"workflow_inputs":{"format_version":1,"bindings":{"/path":"/filename","/sha256":"/hash"}}})).unwrap()).await.unwrap();
        let preview = fixture
            .runtime
            .store
            .preview_automation_event(
                &fixture.project.id,
                &trigger.id,
                &SubmitAutomationEvent {
                    event_id: "preview".into(),
                    data: json!({"filename":42}),
                },
            )
            .await
            .unwrap();
        assert!(preview.input_error.is_some());
        assert!(!preview.would_queue);
        let bad = SubmitAutomationEvent {
            event_id: "bad".into(),
            data: json!({"filename":42}),
        };
        let good = SubmitAutomationEvent {
            event_id: "good".into(),
            data: json!({"filename":"incoming.pdf","hash":cowork_domain::hash(b"synthetic file metadata"),"currency":"USD","unbound":"UNBOUND_EVENT_MARKER","allowed_tools":["execute_program"]}),
        };
        let secret = "ab".repeat(32);
        if source == AutomationEventSource::Webhook {
            let expires_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64
                + 60_000;
            fixture
                .runtime
                .store
                .configure_automation_webhook(
                    &fixture.project.id,
                    &trigger.id,
                    &ConfigureAutomationWebhook {
                        expected_revision: 0,
                        idempotency_key: "grant".into(),
                        token: Some(secret.clone()),
                        enabled: true,
                        expires_at,
                    },
                )
                .await
                .unwrap();
        }
        let invalid = if source == AutomationEventSource::Webhook {
            fixture
                .runtime
                .store
                .submit_automation_webhook(&trigger.id, &secret, &bad)
                .await
                .unwrap()
        } else {
            fixture
                .runtime
                .store
                .submit_automation_event(&fixture.project.id, &trigger.id, source.clone(), &bad)
                .await
                .unwrap()
        };
        assert_eq!(invalid.state, AutomationEventState::Blocked);
        assert_eq!(fixture.runtime.automation_tick().await.unwrap(), 0);
        assert!(fixture.model.requests.lock().unwrap().is_empty());
        assert_eq!(fixture.model.probes.load(Ordering::SeqCst), 0);
        let original = if source == AutomationEventSource::Webhook {
            fixture
                .runtime
                .store
                .submit_automation_webhook(&trigger.id, &secret, &good)
                .await
                .unwrap()
        } else {
            fixture
                .runtime
                .store
                .submit_automation_event(&fixture.project.id, &trigger.id, source.clone(), &good)
                .await
                .unwrap()
        };
        assert_eq!(
            original.workflow_input.as_ref().unwrap().values["currency"],
            "EUR"
        );
        assert_eq!(fixture.runtime.automation_tick().await.unwrap(), 1);
        let history = fixture
            .runtime
            .store
            .automation_events(&fixture.project.id, &trigger.id, None, 50)
            .await
            .unwrap();
        let event = history
            .items
            .iter()
            .find(|event| event.id == original.id)
            .unwrap();
        let run = fixture
            .runtime
            .store
            .stored_run(event.run_id.as_ref().unwrap())
            .await
            .unwrap();
        assert!(
            run.record
                .spec
                .allowed_tools
                .iter()
                .all(|tool| cowork_domain::read_only_tool(tool))
        );
        assert!(
            !run.record
                .spec
                .allowed_tools
                .iter()
                .any(|tool| tool == "execute_program")
        );
        assert_eq!(
            run.record.spec.workflow.as_ref().unwrap().inputs.as_deref(),
            Some(original.workflow_input.as_ref().unwrap().values.as_ref())
        );
        assert_eq!(
            fixture
                .runtime
                .run_native_workflow(
                    &fixture.project.id,
                    &run.record.id,
                    &CancellationToken::new()
                )
                .await
                .unwrap()
                .state,
            RunState::Completed
        );
        let requests = fixture.model.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|body| {
            body.get("tools")
                .is_none_or(|tools| tools.as_array().is_some_and(Vec::is_empty))
        }));
        let frame = requests[0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|message| message["content"].as_str())
            .filter_map(|text| serde_json::from_str::<Value>(text).ok())
            .find(|value| value["workflow_step"].is_string())
            .unwrap();
        assert_eq!(frame["inputs"]["file"], "incoming.pdf");
        assert_eq!(frame["inputs"]["expected_sha256"], good.data["hash"]);
        assert!(
            !serde_json::to_string(&requests)
                .unwrap()
                .contains("UNBOUND_EVENT_MARKER")
        );
        let replay = if source == AutomationEventSource::Webhook {
            fixture
                .runtime
                .store
                .submit_automation_webhook(&trigger.id, &secret, &good)
                .await
                .unwrap()
        } else {
            fixture
                .runtime
                .store
                .submit_automation_event(&fixture.project.id, &trigger.id, source.clone(), &good)
                .await
                .unwrap()
        };
        assert_eq!(canonical_hash(&replay), canonical_hash(&original));
        assert_eq!(fixture.runtime.automation_tick().await.unwrap(), 0);
        assert_eq!(fixture.model.requests.lock().unwrap().len(), 2);
        fixture.runtime.store.close().await;
    }
}

#[tokio::test]
async fn a_saved_task_and_automation_keep_their_explicit_typed_input_snapshot() {
    let fixture = fixture(false).await;
    let task=fixture.runtime.store.write_workspace_task(&fixture.project.id,None,&serde_json::from_value(json!({
        "expected_revision":0,"idempotency_key":"typed-task","value":{
            "name":"Typed task","description":"Fixed input snapshot","order_id":fixture.input.order_id,
            "prompt":"Use the configured typed inputs.","expected_output":"A typed result",
            "execution":{"kind":"workflow","workflow_id":fixture.input.workflow_id,"version_id":fixture.input.version_id,"inputs":fixture.input.inputs},
            "access_mode":"read_only","budgets":fixture.input.budgets
        }
    })).unwrap()).await.unwrap();
    let trigger=fixture.runtime.write_automation_trigger(&fixture.project.id,&serde_json::from_value(json!({
        "expected_revision":0,"idempotency_key":"typed-trigger","name":"Typed manual event","task_id":task.id,
        "task_revision":task.revision,"source":"manual","predicates":[],"enabled":true
    })).unwrap()).await.unwrap();
    fixture
        .runtime
        .store
        .submit_automation_event(
            &fixture.project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &SubmitAutomationEvent {
                event_id: "typed-event".into(),
                data: json!({"currency":"USD"}),
            },
        )
        .await
        .unwrap();
    assert_eq!(fixture.runtime.automation_tick().await.unwrap(), 1);
    let events = fixture
        .runtime
        .store
        .automation_events(&fixture.project.id, &trigger.id, None, 50)
        .await
        .unwrap();
    assert_eq!(events.items.len(), 1);
    let run = fixture
        .runtime
        .store
        .stored_run(events.items[0].run_id.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        run.record.spec.workflow.as_ref().unwrap().inputs,
        fixture.input.inputs
    );
    let result = fixture
        .runtime
        .run_native_workflow(
            &fixture.project.id,
            &run.record.id,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.state, RunState::Completed);
    assert_eq!(fixture.model.requests.lock().unwrap().len(), 2);
}
