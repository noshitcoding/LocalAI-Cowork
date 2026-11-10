//! Functional actual OCR -> real loopback provider protocol -> native human review.
//! Scripted model responses qualify dataflow, not extraction accuracy or parser isolation.
use super::*;
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use cowork_runtime::ConfiguredProvider;
use std::sync::Mutex;

#[derive(Clone, Default)]
struct Model {
    malformed: bool,
    requests: Arc<Mutex<Vec<Value>>>,
}
struct Server {
    endpoint: String,
    model: Model,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn models() -> Json<Value> {
    Json(json!({"data":[{"id":"extraction-fixture-model"}]}))
}
async fn chat(State(model): State<Model>, Json(body): Json<Value>) -> String {
    model.requests.lock().unwrap().push(body);
    let proposed = if model.malformed {
        json!({"invoice":"R-2026-1042","issued":null,"net_minor":10000,"tax_minor":1900,"gross_minor":"119.00"})
    } else {
        json!({"invoice":"R-2026-1042","issued":null,"net_minor":10000,"tax_minor":1900,"gross_minor":11800})
    };
    let frame = json!({"choices":[{"index":0,"delta":{"content":proposed.to_string()},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":32}});
    format!("data: {frame}\n\ndata: [DONE]\n\n")
}
async fn server(malformed: bool) -> Server {
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
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        endpoint,
        model,
        task,
    }
}
fn profile(server: &Server) -> ConfiguredProvider {
    ConfiguredProvider{profile:serde_json::from_value(json!({"id":"extraction-fixture","name":"Scripted loopback functional fixture","kind":"openai_compatible","endpoint":server.endpoint,"model":"extraction-fixture-model","revision":1,"context_tokens":16384,"credential_ready":true,"credential_required":false})).unwrap(),credential_env:None}
}
fn definition() -> WorkflowDefinition {
    serde_json::from_str(include_str!(
        "../../../../tests/fixtures/workflow-document-extraction.json"
    ))
    .unwrap()
}
async fn fixture(server: &Server, private_field: bool) -> (Fixture, WorkflowDefinition) {
    let mut f = Box::pin(Fixture::new(false)).await;
    f.ocr_source(true, 0, 100).await;
    let configured = profile(server);
    f.runtime
        .store
        .insert_provider(&configured.profile)
        .await
        .unwrap();
    f.runtime = Runtime::new(f.runtime.store.clone(), vec![configured], false)
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&f.temp.path().join("broker"), vec![]).unwrap(),
        )
        .with_toolbox(f.toolbox.clone());
    let mut definition = definition();
    if private_field {
        let WorkflowDataType::Object { fields } = &mut definition.dataflow.as_mut().unwrap().input
        else {
            panic!()
        };
        fields.insert("private_note".into(), WorkflowDataType::Text);
    }
    definition.validate_definition().unwrap();
    let entry = f
        .runtime
        .store
        .create_workflow(&f.project.id, "generated-document-extraction", &definition)
        .await
        .unwrap();
    f.workflow = entry.id;
    f.version = entry.versions[0].id.clone();
    (f, definition)
}
async fn start(f: &Fixture, definition: &WorkflowDefinition, private_field: bool) -> RunRecord {
    let mut request = f.request(AccessMode::ReadOnly).await;
    let RunRequest::Workflow(workflow) = &mut request.request else {
        panic!()
    };
    workflow.budgets = definition.budgets.clone();
    if private_field {
        workflow.inputs.as_mut().unwrap()["private_note"] =
            json!("UNBOUND_PRIVATE_NOTE_MUST_NOT_REACH_PROVIDER");
    }
    let prepared = f.runtime.prepare_execution_plan_v2(&request).await.unwrap();
    f.runtime
        .commit_execution_plan_v2(
            &f.order.id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap()
}
fn confirmed() -> Value {
    json!({"invoice":"R-2026-1042","issued":"2026-10-31","net_minor":10000,"tax_minor":1900,"gross_minor":11900})
}
fn model_frame(server: &Server) -> Value {
    let requests = server.model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    requests[0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .find(|value| value["workflow_step"] == "extract-values")
        .unwrap()
}

const CONTEXT_HISTORY: &str = "EXTRACTION_PARENT_HISTORY_NOT_BOUND_OLD_TOTAL_99999";
const CONTEXT_PROJECT: &str = "EXTRACTION_PROJECT_SKILL_NOT_BOUND_OTHER_CUSTOMER";
const CONTEXT_BASE: &str = "EXTRACTION_BASE_INSTRUCTION_NOT_BOUND_OLD_PERSONA";
const CONTEXT_ROLE: &str = "EXTRACTION_ROLE_TEMPLATE_NOT_BOUND_OLD_PERSONA";

async fn context_fixture(
    server: &Server,
    additional_instructions: bool,
) -> (Fixture, WorkflowDefinition) {
    let (mut f, mut definition) = Box::pin(fixture(server, false)).await;
    if additional_instructions {
        definition.roles[0].agent_config = Some(EffectiveAgentConfig {
            instructions: CONTEXT_ROLE.into(),
            ..Default::default()
        });
        let entry = f
            .runtime
            .store
            .create_workflow(
                &f.project.id,
                "extraction-context-instructions",
                &definition,
            )
            .await
            .unwrap();
        f.workflow = entry.id;
        f.version = entry.versions[0].id.clone();
    }
    let provider = profile(server).profile;
    let previous: CreateRun = serde_json::from_value(json!({"order_id":f.order.id,"base_revision":f.order.revision,"idempotency_key":"previous-unrelated-chat","prompt":CONTEXT_HISTORY,"provider_id":provider.id,"provider_revision":provider.revision,"access_mode":"read_only","budgets":Budgets::default(),"native_tools":[],"folders":[],"mcp_tools":[]})).unwrap();
    let spec: RunSpec = serde_json::from_value(json!({"project_id":f.project.id,"order_id":f.order.id,"prompt":CONTEXT_HISTORY,"provider":provider,"access_mode":"read_only","executor":"local-v2","policy_revision":1,"allowed_tools":[],"budgets":Budgets::default(),"folders":[],"mcp_tools":[],"workflow":null,"environment":null,"file_action":null,"output":null,"model_binding":null,"agent_config":null})).unwrap();
    let previous = f.runtime.store.create_run(&previous, spec).await.unwrap();
    assert_eq!(
        f.runtime.store.cancel(&previous.id).await.unwrap().state,
        RunState::Canceled
    );
    let skill: WriteSkill = serde_json::from_value(json!({"id":null,"expected_revision":0,"name":"Unrelated project instructions","description":"","instructions":CONTEXT_PROJECT,"enabled":true})).unwrap();
    f.runtime
        .store
        .productivity_write_skill(&f.project.id, skill)
        .await
        .unwrap();
    f.order = f.runtime.store.order(&f.order.id).await.unwrap();
    (f, definition)
}

async fn assert_context_provider_boundary(additional_instructions: bool) {
    let server = server(false).await;
    let (f, definition) = Box::pin(context_fixture(&server, additional_instructions)).await;
    let mut request = f.request(AccessMode::ReadOnly).await;
    let RunRequest::Workflow(workflow) = &mut request.request else {
        panic!()
    };
    workflow.budgets = definition.budgets.clone();
    if additional_instructions {
        workflow.agent_config = Some(EffectiveAgentConfig {
            instructions: CONTEXT_BASE.into(),
            ..Default::default()
        });
    }
    let plan = f.runtime.prepare_execution_plan_v2(&request).await.unwrap();
    let run = f
        .runtime
        .commit_execution_plan_v2(
            &f.order.id,
            &plan.plan.id,
            &CommitExecutionPlan {
                sha256: plan.sha256,
            },
        )
        .await
        .unwrap();
    let parent = f.runtime.store.stored_run(&run.id).await.unwrap();
    let parent_seed = serde_json::to_string(&parent.checkpoint.messages).unwrap();
    assert!(
        parent_seed.contains(CONTEXT_HISTORY) && parent_seed.contains(CONTEXT_PROJECT),
        "fixture did not freeze the competing context"
    );
    let (stop, task) = worker(&f);
    let question = question(&f, &run).await;
    assert_eq!(
        question.review.as_ref().unwrap().evidence[0].output["text"],
        model_frame(&server)["inputs"]["document_text"]
    );
    let requests = server.model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let full_request = serde_json::to_string(&requests[0]).unwrap();
    let leaked: Vec<_> = [CONTEXT_HISTORY, CONTEXT_PROJECT, CONTEXT_BASE, CONTEXT_ROLE]
        .into_iter()
        .filter(|value| full_request.contains(value))
        .collect();
    if let Some(directory) = std::env::var_os("COWORK_CONTEXT_CAPTURE_DIR") {
        let name = if additional_instructions {
            "base-role-boundary"
        } else {
            "parent-project-boundary"
        };
        std::fs::write(Path::new(&directory).join(format!("{name}-provider-request.json")), serde_json::to_vec_pretty(&json!({"actual_loopback_request":requests[0],"accepted_parent_messages":parent.checkpoint.messages,"additional_instructions":additional_instructions,"leaked_sentinels":leaked})).unwrap()).unwrap();
    }
    stop.cancel();
    task.await.unwrap();
    f.runtime.store.close().await;
    assert!(
        leaked.is_empty(),
        "unbound context reached actual loopback provider: {leaked:?}"
    );
}

#[tokio::test]
#[ignore = "requires explicit real host OCR; captures the entire scripted loopback provider request"]
async fn extraction_provider_request_excludes_unbound_parent_and_project_context() {
    Box::pin(assert_context_provider_boundary(false)).await;
}

#[tokio::test]
#[ignore = "requires explicit real host OCR; captures the entire scripted loopback provider request"]
async fn extraction_provider_request_excludes_base_and_role_template_instructions() {
    Box::pin(assert_context_provider_boundary(true)).await;
}
async fn intent(
    f: &Fixture,
    run: &RunRecord,
    input: &WorkflowInputView,
) -> WorkflowInputAnswerIntent {
    let head = f
        .runtime
        .store
        .workflow_input_draft(&run.id, &input.request_id)
        .await
        .unwrap();
    let WorkflowInputDraftSave::Saved { draft, .. } = f
        .runtime
        .store
        .save_workflow_input_draft(
            &run.id,
            &input.request_id,
            &SaveWorkflowInputDraft {
                fingerprint: input.fingerprint.clone(),
                base_revision: head.draft.revision,
                idempotency_key: "correct-extracted-values".into(),
                answer: confirmed().to_string(),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    f.runtime
        .store
        .prepare_workflow_input_answer(
            &run.id,
            &input.request_id,
            &PrepareWorkflowInputAnswer {
                fingerprint: input.fingerprint.clone(),
                base_draft_revision: draft.revision,
                idempotency_key: "confirm-extraction-original-intent".into(),
            },
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires explicit actual OCR Python and language pack; model protocol is scripted loopback"]
async fn actual_ocr_typed_extraction_review_correction_and_waiting_restart_never_repeat_model_or_parser()
 {
    let server = server(false).await;
    let (mut f, definition) = Box::pin(fixture(&server, false)).await;
    let run = start(&f, &definition, false).await;
    let (stop, task) = worker(&f);
    let input = question(&f, &run).await;
    let review = input.review.as_ref().unwrap();
    assert!(input.role_id.is_none());
    assert_eq!(review.form.source_steps, ["document-source"]);
    assert_eq!(review.evidence.len(), 1);
    let source = &review.evidence[0].output;
    assert_eq!(source["source"]["sha256"], f.source_sha);
    assert_eq!(source["words"].as_array().unwrap().len(), 19);
    assert!(review.values["issued"].is_null());
    assert_eq!(review.values["gross_minor"], 11800);
    assert!(review.form.validate_answer(&review.values).is_err());
    let frame = model_frame(&server);
    assert_eq!(frame["inputs"]["document_text"], source["text"]);
    assert_eq!(frame["inputs"]["source_sha256"], f.source_sha);
    assert_eq!(frame["inputs"].as_object().unwrap().len(), 6);
    assert!(frame["inputs"].get("words").is_none());
    let before = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let mut operations = before
        .progress
        .attempts()
        .values()
        .map(|attempt| {
            (
                attempt.claim.node_id.clone(),
                serde_json::to_value(&attempt.operation).unwrap()["operation"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        })
        .collect::<Vec<_>>();
    operations.sort();
    assert_eq!(
        operations,
        [
            ("document-source", "preparation"),
            ("document-source", "tool"),
            ("extract-values", "model"),
            ("review-values", "preparation"),
            ("review-values", "tool")
        ]
        .map(|(node, operation)| (node.to_owned(), operation.to_owned()))
    );
    assert!(before.plan.roles["Extract"].rights.tools.is_empty());
    assert!(before.plan.roles["Extract"].rights.folders.is_empty());
    let original = intent(&f, &run, &input).await;
    stop.cancel();
    task.await.unwrap();
    f.runtime.store.close().await;
    f.runtime = Runtime::new(
        Store::sqlite(&f.temp.path().join("native-documents.sqlite3"))
            .await
            .unwrap(),
        vec![profile(&server)],
        false,
    )
    .unwrap()
    .with_file_broker(
        cowork_broker::FileBroker::open(&f.temp.path().join("broker"), vec![]).unwrap(),
    )
    .with_toolbox(f.toolbox.clone());
    let preview = f
        .runtime
        .store
        .workflow_run_view(&f.project.id, &run.id)
        .await
        .unwrap()
        .input_resume_preview;
    assert!(preview.available, "{:?}", preview.blocked_reason);
    let request = ResumeWorkflowInput {
        run_id: run.id.clone(),
        base_run_revision: preview.base_run_revision,
        base_workflow_revision: preview.base_workflow_revision,
        expected_checkpoint_sha256: preview.expected_checkpoint_sha256,
        expected_cursor_sha256: preview.expected_cursor_sha256.unwrap(),
        idempotency_key: "resume-extracted-review".into(),
    };
    let operation = f
        .runtime
        .store
        .prepare_workflow_operation(
            &f.project.id,
            &PrepareWorkflowOperation {
                intent_id: "resume-extracted-review".into(),
                request: WorkflowOperationRequest::ResumeInput { request },
                draft_snapshot: None,
            },
        )
        .await
        .unwrap();
    f.runtime
        .commit_workflow_operation(&f.project.id, &operation.id)
        .await
        .unwrap();
    let (stop, task) = worker(&f);
    let restored = question(&f, &run).await;
    assert_eq!(restored.review, input.review);
    assert_eq!(restored.fingerprint, input.fingerprint);
    assert_eq!(restored.expires_at, input.expires_at);
    assert_eq!(server.model.requests.lock().unwrap().len(), 1);
    let next = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(next.progress.attempts()),
        canonical_hash(before.progress.attempts())
    );
    let once = f
        .runtime
        .store
        .commit_workflow_input_answer_intent(&run.id, &original.id)
        .await
        .unwrap();
    let detail = f.completed(&run.id).await;
    assert_eq!(detail.run.state, RunState::Completed);
    assert!(detail.approvals.is_empty() && detail.file_operations.is_empty());
    let twice = f
        .runtime
        .store
        .commit_workflow_input_answer_intent(&run.id, &original.id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&once), canonical_hash(&twice));
    assert_eq!(server.model.requests.lock().unwrap().len(), 1);
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let WorkflowNodeState::Completed { output, .. } =
        &stored.progress.nodes()["review-values"].state
    else {
        panic!()
    };
    assert_eq!(serde_json::from_str::<Value>(output).unwrap(), confirmed());
    assert_eq!(
        stored.inputs[&input.request_id]
            .review
            .as_ref()
            .unwrap()
            .evidence[0]
            .output,
        *source
    );
    assert_eq!(
        f.runtime
            .store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .model_attempts
            .len(),
        1
    );
    stop.cancel();
    task.await.unwrap();
    f.runtime.store.close().await;
}

#[tokio::test]
#[ignore = "requires explicit actual OCR Python and language pack; model protocol is scripted loopback"]
async fn wrong_model_field_type_stops_before_human_review_and_unbound_inputs_never_enter_model_context()
 {
    let server = server(true).await;
    let (f, definition) = Box::pin(fixture(&server, true)).await;
    let run = start(&f, &definition, true).await;
    let (stop, task) = worker(&f);
    let until = tokio::time::Instant::now() + Duration::from_secs(45);
    let detail = loop {
        let value = f.runtime.store.detail(&run.id).await.unwrap();
        if value.run.state == RunState::Failed {
            break value;
        }
        assert!(
            matches!(
                value.run.state,
                RunState::Queued | RunState::Running | RunState::WaitingForSnapshot
            ),
            "{:?}",
            value.run
        );
        assert!(
            tokio::time::Instant::now() < until,
            "malformed model result did not stop"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(detail.approvals.is_empty() && detail.file_operations.is_empty());
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert!(stored.inputs.is_empty());
    assert!(matches!(
        stored.progress.nodes()["document-source"].state,
        WorkflowNodeState::Completed { .. }
    ));
    assert!(matches!(
        stored.progress.nodes()["extract-values"].state,
        WorkflowNodeState::Failed { .. }
    ));
    let frame = model_frame(&server);
    assert_eq!(frame["inputs"].as_object().unwrap().len(), 6);
    assert!(
        !serde_json::to_string(&server.model.requests.lock().unwrap().clone())
            .unwrap()
            .contains("UNBOUND_PRIVATE_NOTE_MUST_NOT_REACH_PROVIDER")
    );
    stop.cancel();
    task.await.unwrap();
    f.runtime.store.close().await;
}
