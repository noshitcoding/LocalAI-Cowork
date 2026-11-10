//! Real document engine, PlanV2 working copies and the workflow effect journal.
//! Responses come from an explicitly synthetic local HTTP model fixture.
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use base64::Engine;
use cowork_contracts::*;
use cowork_domain::{Error, canonical_hash, hash, workflows::*};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::{Store, StoredWorkflow};
use cowork_workflow_contracts::{
    CommitExecutionPlan, ExecutionNetworkAccess, ExecutionProcessAccess, OwnerExecutionTargetV2,
    PrepareOwnerExecutionPlanV2, RunRequest, WorkflowRunRequest,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const PIXEL: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
#[derive(Clone, Copy)]
enum Mode {
    Preview,
    Write,
    DuplicateWrite,
    ForeignRead,
    ForbiddenWrite,
}
#[derive(Clone)]
struct Model {
    mode: Mode,
    requests: Arc<Mutex<Vec<Value>>>,
    probes: Arc<AtomicUsize>,
    foreign: Arc<Mutex<Option<String>>>,
}
async fn models(State(model): State<Model>) -> Json<Value> {
    model.probes.fetch_add(1, Ordering::SeqCst);
    Json(json!({"data":[{"id":"workflow-documents"}]}))
}
async fn chat(State(model): State<Model>, Json(body): Json<Value>) -> String {
    model.requests.lock().unwrap().push(body.clone());
    let messages = body["messages"].as_array().unwrap();
    let folders: Vec<FolderGrant> = messages
        .iter()
        .filter(|value| value["role"] == "system")
        .filter_map(|value| value["content"].as_str())
        .find_map(|text| {
            text.split_once(
                "Selected folder capabilities (use these IDs; no other folders are granted): ",
            )
            .and_then(|(_, text)| {
                serde_json::Deserializer::from_str(text)
                    .into_iter::<Vec<FolderGrant>>()
                    .next()
            })
            .map(Result::unwrap)
        })
        .unwrap();
    assert_eq!(folders.len(), 1);
    let folder = folders[0].id.clone();
    let tools = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect::<Vec<_>>();
    let (delta, finish) = if tools.is_empty() {
        let call = match model.mode {
            Mode::Preview => {
                json!({"id":"preview","type":"function","function":{"name":"document_read","arguments":json!({"operation":"preview","folder_id":folder,"path":"pixel.png","expected_sha256":hash(&base64::engine::general_purpose::STANDARD.decode(PIXEL).unwrap())}).to_string()}})
            }
            Mode::ForeignRead => {
                json!({"id":"foreign","type":"function","function":{"name":"document_read","arguments":json!({"operation":"text","folder_id":model.foreign.lock().unwrap().clone().unwrap(),"path":"private.docx"}).to_string()}})
            }
            Mode::Write | Mode::DuplicateWrite | Mode::ForbiddenWrite => {
                json!({"id":"create","type":"function","function":{"name":"document_write","arguments":json!({"operation":"create","output":{"path":"report.docx"},"spec":{"format":"docx","markdown":"# Workflow report\n\nOne confirmed document."}}).to_string()}})
            }
        };
        let mut calls = vec![call];
        if matches!(model.mode, Mode::DuplicateWrite) {
            let mut second = calls[0].clone();
            second["id"] = json!("existing-output");
            calls.push(second);
        }
        for (index, call) in calls.iter_mut().enumerate() {
            call["index"] = json!(index);
        }
        (
            json!({"content":"Use the declared document tool.","tool_calls":calls}),
            "tool_calls",
        )
    } else {
        let first: Value = serde_json::from_str(tools[0]["content"].as_str().unwrap()).unwrap();
        assert_eq!(first["_workflow_success"], true, "{first}");
        if matches!(model.mode, Mode::Preview) {
            assert_eq!(first["cowork_images"].as_array().unwrap().len(), 1);
            assert!(
                messages.iter().any(
                    |message| message["content"]
                        .as_array()
                        .is_some_and(|parts| parts.iter().any(|part| part["image_url"]["url"]
                            .as_str()
                            .is_some_and(|url| url.starts_with("data:image/png;base64,"))))
                )
            );
        } else {
            assert_eq!(first["operation_receipt"]["kind"], "generate");
        }
        if matches!(model.mode, Mode::DuplicateWrite) {
            assert_eq!(tools.len(), 2);
            let rejected: Value =
                serde_json::from_str(tools[1]["content"].as_str().unwrap()).unwrap();
            assert_eq!(rejected["precheck_rejected"], true, "{rejected}");
        }
        (json!({"content":"Document workflow complete."}), "stop")
    };
    let response = json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}],"usage":{"prompt_tokens":20,"completion_tokens":8}});
    format!("data: {response}\n\ndata: [DONE]\n\n")
}

fn python() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("COWORK_TEST_DOCUMENT_PYTHON")
            .expect("this explicitly selected suite needs COWORK_TEST_DOCUMENT_PYTHON"),
    );
    assert!(path.is_file());
    path
}
fn definition() -> WorkflowDefinition {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-documents.json"
    ))
    .unwrap()
}
struct Fixture {
    temp: tempfile::TempDir,
    runtime: Runtime,
    toolbox: Arc<cowork_toolbox::Toolbox>,
    project: Project,
    order: Order,
    profile: ProviderProfile,
    model: Model,
    workflow: String,
    version: String,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new(mode: Mode, with_engine: bool) -> Self {
        let model = Model {
            mode,
            requests: Default::default(),
            probes: Default::default(),
            foreign: Default::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/models", get(models))
            .route("/chat/completions", post(chat))
            .with_state(model.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let temp = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&temp.path().join("documents.sqlite3"))
            .await
            .unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Workflow documents".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Document working copy".into(),
            })
            .await
            .unwrap();
        let profile:ProviderProfile=serde_json::from_value(json!({"id":"model","name":"Explicit local document fixture","kind":"openai_compatible","endpoint":endpoint,"model":"workflow-documents","revision":1,"context_tokens":32768,"credential_ready":true,"credential_required":false,"capabilities":{"image_input":true}})).unwrap();
        store.insert_provider(&profile).await.unwrap();
        let entry = store
            .create_workflow(&project.id, "document-source", &definition())
            .await
            .unwrap();
        let mut programs = BTreeMap::new();
        if with_engine {
            programs.insert("python".into(), python());
        }
        let environment = ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok().map(|value| (name.into(), value)))
            .collect();
        let toolbox = cowork_toolbox::Toolbox::open(cowork_toolbox::Config {
            root: temp.path().join("toolbox"),
            programs,
            environment,
            github_token: None,
        })
        .unwrap();
        if with_engine {
            let until = tokio::time::Instant::now() + Duration::from_secs(30);
            while !toolbox.availability().documents_available {
                assert!(
                    tokio::time::Instant::now() < until,
                    "actual document engine must be ready"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        let runtime = Runtime::new(
            store,
            vec![ConfiguredProvider {
                profile: profile.clone(),
                credential_env: None,
            }],
            false,
        )
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&temp.path().join("broker"), vec![]).unwrap(),
        )
        .with_toolbox(toolbox.clone());
        runtime.workspace(&order.id).await.unwrap();
        Self {
            temp,
            runtime,
            toolbox,
            project,
            order,
            profile,
            model,
            workflow: entry.id,
            version: entry.versions[0].id.clone(),
            server,
        }
    }
    fn source(&self) -> PathBuf {
        self.temp
            .path()
            .join("broker/workspaces")
            .join(&self.order.id)
            .join("files")
    }
    fn request(&self, mode: AccessMode) -> WorkflowRunRequest {
        serde_json::from_value(json!({"kind":"workflow","order_id":self.order.id,"base_revision":self.order.revision,"idempotency_key":"document-plan","prompt":"Use the selected frozen document workspace.","workflow_id":self.workflow,"version_id":self.version,"access_mode":mode,"budgets":definition().budgets})).unwrap()
    }
    async fn prepare_request(
        &self,
        mode: AccessMode,
        target: OwnerExecutionTargetV2,
    ) -> PrepareOwnerExecutionPlanV2 {
        let workspace = self.runtime.workspace(&self.order.id).await.unwrap();
        let input = self
            .runtime
            .prepare_execution_input(
                &self.order.id,
                &PrepareExecutionInput {
                    idempotency_key: "document-snapshot".into(),
                    base_revision: self.order.revision,
                    folder_ids: vec![workspace.folder.id],
                },
            )
            .await
            .unwrap();
        PrepareOwnerExecutionPlanV2 {
            target,
            request: RunRequest::Workflow(self.request(mode)),
            input: ExecutionInputRef {
                snapshot_id: input.snapshot.id,
                sha256: input.sha256,
                manifest_sha256: input.snapshot.manifest.sha256,
            },
        }
    }
    async fn start(&self, mode: AccessMode) -> RunRecord {
        let request = self
            .prepare_request(mode, OwnerExecutionTargetV2::DocumentEngine {})
            .await;
        let plan = self
            .runtime
            .prepare_execution_plan_v2(&request)
            .await
            .unwrap();
        self.runtime
            .commit_execution_plan_v2(
                &self.order.id,
                &plan.plan.id,
                &CommitExecutionPlan {
                    sha256: plan.sha256,
                },
            )
            .await
            .unwrap()
    }
    async fn stored(&self, run: &str) -> StoredWorkflow {
        self.runtime
            .store
            .stored_workflow(&self.project.id, run)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn missing_document_runtime_blocks_before_any_model_probe_or_run() {
    let f = Fixture::new(Mode::Preview, false).await;
    let error = f
        .runtime
        .preflight_workflow(&f.request(AccessMode::ReadOnly))
        .await
        .unwrap_err();
    assert!(
        matches!(error,Error::Validation(ref code) if code=="workflow_document_runtime_unavailable"),
        "{error:?}"
    );
    assert_eq!(f.model.probes.load(Ordering::SeqCst), 0);
    assert!(f.model.requests.lock().unwrap().is_empty());
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_process_free_broker_target_cannot_silently_enable_document_programs() {
    let f = Fixture::new(Mode::Write, false).await;
    let request = f
        .prepare_request(
            AccessMode::Autonomous,
            OwnerExecutionTargetV2::BrokerController {},
        )
        .await;
    assert!(
        matches!(f.runtime.prepare_execution_plan_v2(&request).await,
        Err(Error::Validation(code)) if code=="workflow_document_target_required")
    );
    assert_eq!(f.model.probes.load(Ordering::SeqCst), 0);
    assert!(f.model.requests.lock().unwrap().is_empty());
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PYTHON with real document wheels"]
async fn a_saved_task_or_legacy_plan_cannot_infer_document_process_consent() {
    let f = Fixture::new(Mode::Write, true).await;
    let request = f.request(AccessMode::Autonomous);
    for error in [
        f.runtime.preflight_workflow(&request).await.unwrap_err(),
        f.runtime
            .create_run_request(&RunRequest::Workflow(request.clone()))
            .await
            .unwrap_err(),
        f.runtime
            .prepare_execution_plan(&RunRequest::Workflow(request))
            .await
            .unwrap_err(),
    ] {
        assert!(
            matches!(error,Error::Validation(ref code) if code=="workflow_document_target_required"),
            "{error:?}"
        );
    }
    let original = f.runtime.workspace(&f.order.id).await.unwrap();
    let mut scoped:RunSpec=serde_json::from_value(json!({"order_id":f.order.id,"project_id":f.project.id,"prompt":"No saved process target","policy_revision":1,"allowed_tools":["document_read"],"folders":[original.folder],"budgets":definition().budgets,"access_mode":"read_only","executor":"local-v2"})).unwrap();
    scoped.input_workspace = None;
    assert!(matches!(
        f.runtime
            .store
            .pinned_document_engine_revision(&scoped)
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(f.model.probes.load(Ordering::SeqCst), 0);
    assert!(f.model.requests.lock().unwrap().is_empty());
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PYTHON with real document wheels"]
async fn document_plan_binds_user_rights_engine_and_original_receipt() {
    let f = Fixture::new(Mode::Write, true).await;
    let request = f
        .prepare_request(
            AccessMode::ConfirmMutations,
            OwnerExecutionTargetV2::DocumentEngine {},
        )
        .await;
    let prepared = f.runtime.prepare_execution_plan_v2(&request).await.unwrap();
    assert!(
        matches!(&prepared.plan.target, cowork_workflow_contracts::ExecutionDestinationV2::DocumentEngine { engine_revision, .. }
        if engine_revision==&f.toolbox.document_engine_revision())
    );
    assert_eq!(
        prepared.plan.boundary.process_access,
        ExecutionProcessAccess::UserRights
    );
    assert_eq!(
        prepared.plan.boundary.process_network,
        ExecutionNetworkAccess::UserRights
    );
    assert!(prepared.plan.boundary.environment.is_none());
    assert!(prepared.plan.boundary.connector_aliases.is_empty());
    let commit = CommitExecutionPlan {
        sha256: prepared.sha256.clone(),
    };
    let run = f
        .runtime
        .commit_execution_plan_v2(&f.order.id, &prepared.plan.id, &commit)
        .await
        .unwrap();
    assert_eq!(
        f.runtime
            .commit_execution_plan_v2(&f.order.id, &prepared.plan.id, &commit)
            .await
            .unwrap()
            .id,
        run.id
    );
    assert!(f.model.requests.lock().unwrap().is_empty());
    if let Some(path) = std::env::var_os("COWORK_DOCUMENT_CONTRACT_FIXTURE") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({"prepare":request,"prepared":prepared,"run":run}))
                .unwrap(),
        )
        .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PYTHON with real document wheels"]
async fn an_engine_change_after_review_cannot_commit_the_old_plan() {
    let f = Fixture::new(Mode::Write, true).await;
    let request = f
        .prepare_request(
            AccessMode::Autonomous,
            OwnerExecutionTargetV2::DocumentEngine {},
        )
        .await;
    let prepared = f.runtime.prepare_execution_plan_v2(&request).await.unwrap();
    f.toolbox
        .configure_document_runtime(f.temp.path().join("changed-engine-cache"))
        .unwrap();
    let until = tokio::time::Instant::now() + Duration::from_secs(30);
    while !f.toolbox.availability().documents_available {
        assert!(tokio::time::Instant::now() < until);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let error = f
        .runtime
        .commit_execution_plan_v2(
            &f.order.id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::ExecutionPlanChanged), "{error:?}");
    assert!(f.model.requests.lock().unwrap().is_empty());
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PYTHON with real document wheels"]
async fn actual_preview_reads_the_frozen_copy_and_reaches_only_its_own_run_and_model() {
    let f = Fixture::new(Mode::Preview, true).await;
    std::fs::write(
        f.source().join("pixel.png"),
        base64::engine::general_purpose::STANDARD
            .decode(PIXEL)
            .unwrap(),
    )
    .unwrap();
    let run = f.start(AccessMode::ReadOnly).await;
    std::fs::write(
        f.source().join("pixel.png"),
        b"changed original after capture",
    )
    .unwrap();
    let done = f
        .runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(done.state, RunState::Completed, "{done:?}");
    assert_eq!(f.model.requests.lock().unwrap().len(), 2);
    let stored = f.stored(&run.id).await;
    let receipt = &stored.agents["document"].turns[0].tool_results[0];
    assert!(receipt.succeeded);
    let sha = receipt.result["cowork_images"][0]["sha256"]
        .as_str()
        .unwrap();
    let image = f.runtime.run_preview(&run.id, sha).await.unwrap();
    assert_eq!(hash(&image), sha);
    let other = f
        .runtime
        .store
        .create_order(CreateOrder {
            project_id: f.project.id.clone(),
            title: "Unrelated preview".into(),
        })
        .await
        .unwrap();
    let other_run = f
        .runtime
        .create_run(&CreateRun {
            order_id: other.id,
            base_revision: 1,
            idempotency_key: "foreign-preview".into(),
            prompt: "No image authority".into(),
            provider_id: "model".into(),
            provider_revision: 1,
            access_mode: AccessMode::ReadOnly,
            folders: vec![],
            budgets: Budgets::default(),
            agent_config: None,
            model_binding: None,
            output: None,
            native_tools: vec![],
            environment: None,
            mcp_tools: vec![],
        })
        .await
        .unwrap();
    assert!(matches!(
        f.runtime.run_preview(&other_run.id, sha).await,
        Err(Error::NotFound)
    ));
    assert_eq!(
        std::fs::read(f.source().join("pixel.png")).unwrap(),
        b"changed original after capture"
    );
    assert!(stored.plan.resource_definition.is_some());
}

#[tokio::test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PYTHON with real document wheels"]
async fn a_document_write_asks_once_and_an_existing_output_is_rejected_before_another_approval() {
    let f = Fixture::new(Mode::DuplicateWrite, true).await;
    let run = f.start(AccessMode::ConfirmMutations).await;
    let runtime = f.runtime.clone();
    let project = f.project.id.clone();
    let id = run.id.clone();
    let worker = tokio::spawn(async move {
        runtime
            .run_native_workflow(&project, &id, &CancellationToken::new())
            .await
    });
    let until = tokio::time::Instant::now() + Duration::from_secs(30);
    let approval = loop {
        let view = f
            .runtime
            .store
            .workflow_run_view(&f.project.id, &run.id)
            .await
            .unwrap();
        if let Some(item) = view.approvals.first() {
            break item.approval.clone();
        }
        assert!(tokio::time::Instant::now() < until);
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(
        f.runtime
            .store
            .detail(&run.id)
            .await
            .unwrap()
            .file_operations
            .is_empty()
    );
    assert!(
        f.stored(&run.id)
            .await
            .progress
            .attempts()
            .values()
            .all(|attempt| !matches!(attempt.operation, WorkflowOperation::Tool { .. }))
    );
    f.runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint,
                allow: true,
            },
        )
        .await
        .unwrap();
    let done = worker.await.unwrap().unwrap();
    assert_eq!(done.state, RunState::Completed, "{done:?}");
    let detail = f.runtime.store.detail(&run.id).await.unwrap();
    assert_eq!(detail.file_operations.len(), 1);
    assert_eq!(detail.file_operations[0].kind, FileOperationKind::Generate);
    assert_eq!(f.model.requests.lock().unwrap().len(), 2);
    let stored = f.stored(&run.id).await;
    let results = &stored.agents["document"].turns[0].tool_results;
    assert_eq!(results.len(), 2);
    assert!(results[0].succeeded);
    assert!(!results[1].succeeded);
    assert_eq!(results[1].result["precheck_rejected"], true);
    assert!(!f.source().join("report.docx").exists());
    let snapshot = f
        .runtime
        .store
        .workspace_snapshot(&run.id)
        .await
        .unwrap()
        .unwrap();
    let output = snapshot
        .entries
        .iter()
        .find(|entry| entry.path.ends_with("/report.docx"))
        .unwrap();
    let data = f
        .runtime
        .download_snapshot(
            &f.order.id,
            &run.id,
            DownloadFile {
                path: output.path.clone(),
                expected_sha256: output.sha256.clone(),
            },
        )
        .await
        .unwrap();
    assert!(data.bytes.starts_with(b"PK"));
    assert_eq!(
        results[0].result["document_engine_revision"],
        f.toolbox.document_engine_revision()
    );
}

#[tokio::test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PYTHON with real document wheels"]
async fn a_foreign_document_input_is_rejected_before_a_tool_attempt() {
    let f = Fixture::new(Mode::ForeignRead, true).await;
    let other = f
        .runtime
        .store
        .create_order(CreateOrder {
            project_id: f.project.id.clone(),
            title: "Foreign document source".into(),
        })
        .await
        .unwrap();
    let foreign = f.runtime.workspace(&other.id).await.unwrap();
    *f.model.foreign.lock().unwrap() = Some(foreign.folder.id);
    let run = f.start(AccessMode::ReadOnly).await;
    assert!(matches!(
        f.runtime
            .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
            .await,
        Err(Error::Forbidden)
    ));
    assert!(
        f.stored(&run.id)
            .await
            .progress
            .attempts()
            .values()
            .all(|attempt| !matches!(attempt.operation, WorkflowOperation::Tool { .. }))
    );
    assert_eq!(f.model.requests.lock().unwrap().len(), 1);
    assert!(
        f.runtime
            .store
            .detail(&run.id)
            .await
            .unwrap()
            .file_operations
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PYTHON with real document wheels"]
async fn read_only_workflows_never_advertise_or_execute_a_document_write() {
    let f = Fixture::new(Mode::ForbiddenWrite, true).await;
    let run = f.start(AccessMode::ReadOnly).await;
    let done = f
        .runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(done.state, RunState::Failed);
    {
        let requests = f.model.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .all(|tool| tool["function"]["name"] != "document_write"
                    && tool["function"]["name"] != "image_edit")
        );
    }
    assert!(!f.source().join("report.docx").exists());
    assert!(
        f.runtime
            .store
            .detail(&run.id)
            .await
            .unwrap()
            .file_operations
            .is_empty()
    );
}

async fn lost_publication_receipt(reject_source_outcome: bool) {
    let f = Fixture::new(Mode::Write, true).await;
    let run = f.start(AccessMode::Autonomous).await;
    let path = f.temp.path().join("documents.sqlite3");
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let trigger = if reject_source_outcome {
        "CREATE TRIGGER lose_document_ack BEFORE UPDATE ON workflow_external_evidence WHEN NEW.result_sha256 IS NOT NULL BEGIN SELECT RAISE(ABORT,'lost source result'); END"
    } else {
        "CREATE TRIGGER lose_document_ack BEFORE INSERT ON events WHEN json_extract(NEW.payload,'$.kind')='workflow_native_tool_completed' BEGIN SELECT RAISE(ABORT,'lost graph result'); END"
    };
    sqlx::query(trigger).execute(&pool).await.unwrap();
    assert!(
        f.runtime
            .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(
        f.runtime.store.detail(&run.id).await.unwrap().run.state,
        RunState::Interrupted
    );
    let before = f.stored(&run.id).await;
    let call = before
        .progress
        .attempts()
        .values()
        .find(|attempt| matches!(attempt.operation, WorkflowOperation::Tool { .. }))
        .unwrap()
        .id
        .clone();
    let evidence = f
        .runtime
        .store
        .workflow_external_evidence(&f.project.id, &run.id, &call)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evidence.kind, "file_operation");
    assert_eq!(evidence.result.is_none(), reject_source_outcome);
    let expected = evidence.preparation["operation_receipt"]["after"]["sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    sqlx::query("DROP TRIGGER lose_document_ack")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&path).await.unwrap();
    let runtime = Runtime::new(
        reopened,
        vec![ConfiguredProvider {
            profile: f.profile.clone(),
            credential_env: None,
        }],
        false,
    )
    .unwrap()
    .with_file_broker(
        cowork_broker::FileBroker::open(&f.temp.path().join("broker"), vec![]).unwrap(),
    )
    .with_toolbox(f.toolbox.clone());
    let result = runtime
        .reconcile_workflow_effects(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(result["settled_calls"].as_array().unwrap().len(), 1);
    assert!(
        result["unresolved_calls"].as_array().unwrap().is_empty(),
        "{result}"
    );
    assert_eq!(f.model.requests.lock().unwrap().len(), 1);
    let recovered = runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let receipt = &recovered.agents["document"].turns[0].tool_results[0];
    assert_eq!(
        receipt.result["operation_receipt"]["after"]["sha256"],
        expected
    );
    assert!(receipt.result.get("engine").is_some());
    assert_eq!(
        canonical_hash(&recovered.plan),
        canonical_hash(&before.plan)
    );
    let again = runtime
        .reconcile_workflow_effects(&f.project.id, &run.id)
        .await
        .unwrap();
    assert!(again["settled_calls"].as_array().unwrap().is_empty());
    runtime.store.resume(&run.id).await.unwrap();
    let done = runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(done.state, RunState::Completed, "{done:?}");
    assert_eq!(f.model.requests.lock().unwrap().len(), 2);
    assert_eq!(
        runtime
            .store
            .detail(&run.id)
            .await
            .unwrap()
            .file_operations
            .len(),
        1
    );
    let snapshot = runtime
        .store
        .workspace_snapshot(&run.id)
        .await
        .unwrap()
        .unwrap();
    let outputs = snapshot
        .entries
        .iter()
        .filter(|entry| entry.path.ends_with("/report.docx"))
        .collect::<Vec<_>>();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].sha256.as_deref(), Some(expected.as_str()));
}
#[tokio::test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PYTHON with real document wheels"]
async fn a_published_document_with_a_lost_graph_ack_is_reconciled_without_reexecution() {
    lost_publication_receipt(false).await;
}
#[tokio::test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PYTHON with real document wheels"]
async fn a_published_document_without_a_source_outcome_is_observed_without_reexecution() {
    lost_publication_receipt(true).await;
}

#[tokio::test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PYTHON with real document wheels"]
async fn native_document_nodes_prepare_without_any_provider_contact_or_early_run() {
    let mut f = Fixture::new(Mode::Write, true).await;
    let native: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-native-documents.json"
    ))
    .unwrap();
    assert!(native.roles.is_empty());
    let entry = f
        .runtime
        .store
        .create_workflow(&f.project.id, "native-document-source", &native)
        .await
        .unwrap();
    f.workflow = entry.id;
    f.version = entry.versions[0].id.clone();
    let mut request = f
        .prepare_request(
            AccessMode::Autonomous,
            OwnerExecutionTargetV2::DocumentEngine {},
        )
        .await;
    let RunRequest::Workflow(input) = &mut request.request else {
        unreachable!()
    };
    input.inputs = Some(Box::new(
        json!({"source_path":"source.docx","source_sha256":"a".repeat(64)}),
    ));
    let prepared = f.runtime.prepare_execution_plan_v2(&request).await.unwrap();
    let cowork_workflow_contracts::ExecutionPreflightV2::Owner {
        review: cowork_workflow_contracts::RunPreflight::Workflow(preflight),
    } = &prepared.plan.preflight
    else {
        panic!("expected native workflow preflight");
    };
    assert!(preflight.ready && preflight.roles.is_empty());
    assert_eq!(f.model.probes.load(Ordering::SeqCst), 0);
    assert!(f.model.requests.lock().unwrap().is_empty());
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
}
