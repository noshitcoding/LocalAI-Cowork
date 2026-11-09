//! Actual Single-chat PlanV2 document effects; synthetic local HTTP completions.
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use base64::Engine;
use cowork_contracts::*;
use cowork_domain::{Error, hash};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use cowork_workflow_contracts::{
    CommitExecutionPlan, OwnerExecutionTargetV2, PrepareOwnerExecutionPlanV2, RunRequest,
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
    Write,
    Preview,
    Rejected,
    Foreign,
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
        .filter(|message| message["role"] == "system")
        .filter_map(|message| message["content"].as_str())
        .find_map(|text| {
            text.split_once(
                "Selected folder capabilities (use these IDs; no other folders are granted): ",
            )
            .and_then(|(_, raw)| {
                serde_json::Deserializer::from_str(raw)
                    .into_iter::<Vec<FolderGrant>>()
                    .next()
            })
            .map(Result::unwrap)
        })
        .unwrap();
    assert_eq!(folders.len(), 1);
    let tool = messages.iter().find(|message| message["role"] == "tool");
    let (delta, finish) = if let Some(tool) = tool {
        let value: Value = serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();
        match model.mode {
            Mode::Write => assert_eq!(value["kind"], "generate", "{value}"),
            Mode::Preview => assert_eq!(value["previews"]["count"], 1, "{value}"),
            Mode::Rejected => assert!(value["error"].is_string(), "{value}"),
            Mode::Foreign => assert_eq!(value["error"], "forbidden", "{value}"),
        }
        (json!({"content":"Single document task complete."}), "stop")
    } else {
        let (name, args) = match model.mode {
            Mode::Preview => (
                "document_read",
                json!({"operation":"preview","folder_id":folders[0].id,"path":"pixel.png","expected_sha256":hash(&base64::engine::general_purpose::STANDARD.decode(PIXEL).unwrap())}),
            ),
            Mode::Foreign => (
                "document_read",
                json!({"operation":"text","folder_id":model.foreign.lock().unwrap().clone().unwrap(),"path":"private.txt"}),
            ),
            _ => (
                "document_write",
                json!({"operation":"create","output":{"path":"report.docx"},"spec":{"format":"docx","markdown":"# Single report\n\nCreated from the confirmed copy."}}),
            ),
        };
        (
            json!({"tool_calls":[{"index":0,"id":"document-call","type":"function","function":{"name":name,"arguments":args.to_string()}}]}),
            "tool_calls",
        )
    };
    let response = json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}],"usage":{"prompt_tokens":20,"completion_tokens":8}});
    format!("data: {response}\n\ndata: [DONE]\n\n")
}
fn python() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("COWORK_TEST_DOCUMENT_PYTHON")
            .expect("Explicit real document engine required"),
    );
    assert!(path.is_file());
    path
}
fn budgets() -> Budgets {
    Budgets {
        max_steps: 32,
        max_seconds: 180,
        max_output_tokens: 1024,
        max_total_tokens: 131072,
    }
}
struct Fixture {
    temp: tempfile::TempDir,
    runtime: Runtime,
    toolbox: Arc<cowork_toolbox::Toolbox>,
    project: Project,
    order: Order,
    profile: ProviderProfile,
    model: Model,
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
    fn request(&self, mode: AccessMode) -> CreateRun {
        serde_json::from_value(json!({"order_id":self.order.id,"base_revision":self.order.revision,"idempotency_key":"single-document-plan","prompt":"Use the selected frozen document workspace.","provider_id":"model","provider_revision":1,"access_mode":mode,"budgets":budgets()})).unwrap()
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
            request: RunRequest::Single(self.request(mode)),
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
}

struct Worker {
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Worker {
    fn start(runtime: Runtime) -> Self {
        let stop = CancellationToken::new();
        let task = tokio::spawn(runtime.serve(1, stop.clone()));
        Self {
            stop,
            task: Some(task),
        }
    }
    async fn close(mut self) {
        self.stop.cancel();
        self.task.take().unwrap().await.unwrap();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
async fn until(f: &Fixture, run: &str, predicate: impl Fn(&RunDetail) -> bool) -> RunDetail {
    let end = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let detail = f.runtime.store.detail(run).await.unwrap();
        if predicate(&detail) {
            return detail;
        }
        assert!(
            tokio::time::Instant::now() < end,
            "last Single state: {:?}",
            detail.run
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
async fn terminal(f: &Fixture, run: &str) -> RunDetail {
    until(f, run, |d| {
        matches!(
            d.run.state,
            RunState::Completed | RunState::Failed | RunState::Expired | RunState::Interrupted
        )
    })
    .await
}

#[tokio::test]
async fn a_missing_single_document_engine_blocks_before_provider_probe_or_run() {
    let f = Fixture::new(Mode::Write, false).await;
    let request = f
        .prepare_request(
            AccessMode::Autonomous,
            OwnerExecutionTargetV2::DocumentEngine {},
        )
        .await;
    assert!(
        matches!(f.runtime.prepare_execution_plan_v2(&request).await,Err(Error::Validation(code)) if code=="workflow_document_runtime_unavailable")
    );
    assert_eq!(f.model.probes.load(Ordering::SeqCst), 0);
    assert!(f.model.requests.lock().unwrap().is_empty());
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
}
#[tokio::test]
#[ignore = "requires explicitly configured real document engine"]
async fn a_confirmed_single_document_copy_asks_once_and_retains_its_original_plan() {
    let f = Fixture::new(Mode::Write, true).await;
    let prepare = f
        .prepare_request(
            AccessMode::ConfirmMutations,
            OwnerExecutionTargetV2::DocumentEngine {},
        )
        .await;
    let prepared = f.runtime.prepare_execution_plan_v2(&prepare).await.unwrap();
    assert!(f.model.requests.lock().unwrap().is_empty());
    let commit = CommitExecutionPlan {
        sha256: prepared.sha256.clone(),
    };
    let run = f
        .runtime
        .commit_execution_plan_v2(&f.order.id, &prepared.plan.id, &commit)
        .await
        .unwrap();
    let worker = Worker::start(f.runtime.clone());
    let pending = until(&f, &run.id, |d| d.run.state == RunState::WaitingApproval).await;
    assert_eq!(pending.approvals.len(), 1);
    assert!(pending.file_operations.is_empty());
    let approval = &pending.approvals[0];
    f.runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint.clone(),
                allow: true,
            },
        )
        .await
        .unwrap();
    let done = terminal(&f, &run.id).await;
    assert_eq!(done.run.state, RunState::Completed, "{:?}", done.run);
    assert_eq!(done.file_operations.len(), 1);
    assert_eq!(done.approvals.len(), 1);
    assert_eq!(f.model.requests.lock().unwrap().len(), 2);
    assert!(!f.source().join("report.docx").exists());
    let snapshot = done.workspace_snapshot.as_ref().unwrap();
    let file = snapshot
        .entries
        .iter()
        .find(|entry| entry.path.ends_with("/report.docx"))
        .unwrap();
    assert!(file.sha256.is_some());
    assert_eq!(
        f.runtime
            .commit_execution_plan_v2(&f.order.id, &prepared.plan.id, &commit)
            .await
            .unwrap()
            .id,
        run.id
    );
    if let Some(path) = std::env::var_os("COWORK_SINGLE_DOCUMENT_CONTRACT_FIXTURE") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({"prepare":prepare,"prepared":prepared,"run":run}))
                .unwrap(),
        )
        .unwrap();
    }
    worker.close().await;
}
#[tokio::test]
#[ignore = "requires explicitly configured real document engine"]
async fn a_single_engine_change_after_approval_is_rejected_before_publication() {
    let f = Fixture::new(Mode::Rejected, true).await;
    let run = f.start(AccessMode::ConfirmMutations).await;
    let worker = Worker::start(f.runtime.clone());
    let pending = until(&f, &run.id, |d| d.run.state == RunState::WaitingApproval).await;
    f.toolbox
        .configure_document_runtime(f.temp.path().join("changed-runtime"))
        .unwrap();
    let approval = &pending.approvals[0];
    f.runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint.clone(),
                allow: true,
            },
        )
        .await
        .unwrap();
    let done = terminal(&f, &run.id).await;
    assert_eq!(done.run.state, RunState::Completed, "{:?}", done.run);
    assert!(done.file_operations.is_empty());
    let stored = f.runtime.store.stored_run(&run.id).await.unwrap();
    let message = stored
        .checkpoint
        .messages
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap();
    let value: Value = serde_json::from_str(message["content"].as_str().unwrap()).unwrap();
    assert_eq!(value["error"], "execution_plan_changed");
    worker.close().await;
}
#[tokio::test]
#[ignore = "requires explicitly configured real document engine"]
async fn a_single_preview_reads_the_confirmed_copy_and_reaches_its_model() {
    let f = Fixture::new(Mode::Preview, true).await;
    let pixels = base64::engine::general_purpose::STANDARD
        .decode(PIXEL)
        .unwrap();
    std::fs::write(f.source().join("pixel.png"), &pixels).unwrap();
    let run = f.start(AccessMode::ReadOnly).await;
    std::fs::write(
        f.source().join("pixel.png"),
        b"Changed original after snapshot",
    )
    .unwrap();
    let worker = Worker::start(f.runtime.clone());
    let done = terminal(&f, &run.id).await;
    assert_eq!(done.run.state, RunState::Completed, "{:?}", done.run);
    assert!(done.file_operations.is_empty());
    {
        let requests = f.model.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1]["messages"].as_array().unwrap().iter().any(|m| {
            m["content"].as_array().is_some_and(|parts| {
                parts.iter().any(|part| {
                    part["image_url"]["url"]
                        .as_str()
                        .is_some_and(|url| url.starts_with("data:image/png;base64,"))
                })
            })
        }));
    }
    let stored = f.runtime.store.stored_run(&run.id).await.unwrap();
    let image = stored
        .checkpoint
        .messages
        .iter()
        .find_map(|m| m["cowork_images"][0]["sha256"].as_str())
        .unwrap();
    assert_eq!(
        hash(&f.runtime.run_preview(&run.id, image).await.unwrap()),
        image
    );
    worker.close().await;
}
#[tokio::test]
#[ignore = "requires explicitly configured real document engine"]
async fn single_document_reads_cannot_access_another_orders_folder() {
    let f = Fixture::new(Mode::Foreign, true).await;
    let other = f
        .runtime
        .store
        .create_order(CreateOrder {
            project_id: f.project.id.clone(),
            title: "Foreign source".into(),
        })
        .await
        .unwrap();
    let folder = f.runtime.workspace(&other.id).await.unwrap().folder;
    std::fs::write(
        f.temp
            .path()
            .join("broker/workspaces")
            .join(&other.id)
            .join("files/private.txt"),
        b"FOREIGN_DOC_SENTINEL",
    )
    .unwrap();
    *f.model.foreign.lock().unwrap() = Some(folder.id);
    let run = f.start(AccessMode::ReadOnly).await;
    let worker = Worker::start(f.runtime.clone());
    let done = terminal(&f, &run.id).await;
    assert_eq!(done.run.state, RunState::Completed, "{:?}", done.run);
    assert!(done.file_operations.is_empty());
    assert!(done.approvals.is_empty());
    worker.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly configured real document engine"]
async fn the_single_broker_destination_never_advertises_or_launches_document_tools() {
    let f = Fixture::new(Mode::Rejected, true).await;
    let request = f
        .prepare_request(
            AccessMode::Autonomous,
            OwnerExecutionTargetV2::BrokerController {},
        )
        .await;
    let prepared = f.runtime.prepare_execution_plan_v2(&request).await.unwrap();
    let run = f
        .runtime
        .commit_execution_plan_v2(
            &f.order.id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap();
    let worker = Worker::start(f.runtime.clone());
    let done = terminal(&f, &run.id).await;
    assert!(done.file_operations.is_empty());
    assert!(done.approvals.is_empty());
    {
        let requests = f.model.requests.lock().unwrap();
        assert!(!requests.is_empty());
        assert!(
            requests[0]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .all(|tool| !matches!(
                    tool["function"]["name"].as_str(),
                    Some("document_read" | "document_write" | "image_edit")
                ))
        );
    }
    worker.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly configured real document engine"]
async fn a_single_read_only_document_plan_never_grants_image_or_document_writes() {
    let f = Fixture::new(Mode::Rejected, true).await;
    let run = f.start(AccessMode::ReadOnly).await;
    assert!(
        !run.spec
            .allowed_tools
            .iter()
            .any(|name| matches!(name.as_str(), "document_write" | "image_edit"))
    );
    let worker = Worker::start(f.runtime.clone());
    let done = terminal(&f, &run.id).await;
    assert!(done.file_operations.is_empty());
    assert!(done.approvals.is_empty());
    worker.close().await;
}
#[tokio::test]
#[ignore = "requires explicitly configured real document engine"]
async fn a_single_published_copy_with_lost_ack_is_reopened_and_settled_without_a_second_document() {
    let mut f = Fixture::new(Mode::Write, true).await;
    let run = f.start(AccessMode::Autonomous).await;
    let path = f.temp.path().join("documents.sqlite3");
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    sqlx::query("CREATE TRIGGER lose_single_document_ack BEFORE INSERT ON events WHEN json_extract(NEW.payload,'$.kind')='tool_completed' BEGIN SELECT RAISE(ABORT,'lost_single_document_ack'); END").execute(&pool).await.unwrap();
    let worker = Worker::start(f.runtime.clone());
    let interrupted = terminal(&f, &run.id).await;
    assert_eq!(
        interrupted.run.state,
        RunState::Interrupted,
        "{:?}",
        interrupted.run
    );
    assert!(interrupted.file_operations.is_empty());
    assert!(interrupted.uncertain_file_operation.is_some());
    assert_eq!(f.model.requests.lock().unwrap().len(), 1);
    worker.close().await;
    let before = f.runtime.store.stored_run(&run.id).await.unwrap();
    let tool = before.checkpoint.in_flight.as_ref().unwrap();
    let fingerprint = cowork_domain::approval_fingerprint(&before.record, tool);
    sqlx::query("DROP TRIGGER lose_single_document_ack")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    f.runtime.store.close().await;
    let runtime = Runtime::new(
        Store::sqlite(&path).await.unwrap(),
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
    f.runtime = runtime;
    f.runtime
        .reconcile_file(
            &run.id,
            ReconcileFile {
                fingerprint,
                decision: ReconcileFileDecision::AcceptCompleted,
            },
        )
        .await
        .unwrap();
    assert_eq!(f.model.requests.lock().unwrap().len(), 1);
    assert_eq!(
        f.runtime
            .store
            .detail(&run.id)
            .await
            .unwrap()
            .file_operations
            .len(),
        1
    );
    assert_eq!(
        cowork_domain::canonical_hash(&f.runtime.store.detail(&run.id).await.unwrap().run.spec),
        cowork_domain::canonical_hash(&run.spec)
    );
    let worker = Worker::start(f.runtime.clone());
    f.runtime.store.resume(&run.id).await.unwrap();
    let done = terminal(&f, &run.id).await;
    assert_eq!(done.run.state, RunState::Completed, "{:?}", done.run);
    assert_eq!(done.file_operations.len(), 1);
    assert_eq!(f.model.requests.lock().unwrap().len(), 2);
    assert_eq!(
        done.workspace_snapshot
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .filter(|item| item.path.ends_with("/report.docx"))
            .count(),
        1
    );
    assert!(!f.source().join("report.docx").exists());
    worker.close().await;
}
