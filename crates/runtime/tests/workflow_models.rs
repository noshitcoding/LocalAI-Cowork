use axum::{Json, Router, extract::State, routing::post};
use cowork_contracts::*;
use cowork_domain::{Error, canonical_hash, workflows::*};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::{CreateWorkflowRun, Store, WorkflowCommand, WorkflowCommandResult};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Copy)]
enum Mode {
    Text,
    Parallel,
    Tool,
    UnknownTool,
    Overflow,
    Slow,
    Truncated,
    RejectA,
    ToolOnce,
    ToolA,
    ArtifactReads,
    UnavailableTool,
    ToolRepeat,
    ToolBudgetA,
    ToolStop,
    InputParallel,
    InputWithHeldB,
    ClockHeld,
    HybridStructured,
    HybridStructuredInvalid,
}
#[derive(Clone)]
struct Server {
    mode: Mode,
    requests: Arc<Mutex<Vec<Value>>>,
    active: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
    parallel: Arc<tokio::sync::Barrier>,
    admission_probe: Arc<Mutex<Option<(Store, String, String)>>>,
    observed_wire: Arc<Mutex<Vec<String>>>,
}
async fn chat(State(server): State<Server>, Json(body): Json<Value>) -> String {
    let probe = server.admission_probe.lock().unwrap().clone();
    if let Some((store, project, run_id)) = probe {
        let workflow = store.stored_workflow(&project, &run_id).await.unwrap();
        let turns: Vec<_> = workflow
            .agents
            .values()
            .flat_map(|agent| &agent.turns)
            .filter(|turn| {
                turn.response.is_none() && json!(turn.context.messages) == body["messages"]
            })
            .collect();
        assert_eq!(
            turns.len(),
            1,
            "one exact admitted workflow request before HTTP response"
        );
        let turn = turns[0];
        let prepared =
            cowork_runtime::provider::prepare(&turn.context.spec, &turn.context.messages, false)
                .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(prepared.payload()).unwrap(),
            body
        );
        let events = store.events(&run_id, 0).await.unwrap();
        let started = events
            .iter()
            .find(|event| {
                event.kind == "workflow_model_started" && event.payload["attempt_id"] == turn.id
            })
            .unwrap();
        assert_eq!(
            started.payload["request_sha256"],
            prepared.logical_fingerprint()
        );
        assert_eq!(started.payload["prepared_request"]["format"], 1);
        assert_eq!(
            started.payload["prepared_request"]["wire_sha256"],
            prepared.wire_sha256()
        );
        server
            .observed_wire
            .lock()
            .unwrap()
            .push(prepared.wire_sha256().to_owned());
    }
    server.requests.lock().unwrap().push(body.clone());
    let step = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["content"].as_str())
        .filter_map(|s| serde_json::from_str::<Value>(s).ok())
        .find_map(|m| m["workflow_step"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "ordinary".into());
    let active = server.active.fetch_add(1, Ordering::SeqCst) + 1;
    server.maximum.fetch_max(active, Ordering::SeqCst);
    // Prove two real requests overlap without relying on a short response delay.
    // A serialized coordinator cannot satisfy this rendezvous and fails boundedly.
    if matches!(server.mode, Mode::Parallel) && matches!(step.as_str(), "a" | "b") {
        tokio::time::timeout(std::time::Duration::from_secs(10), server.parallel.wait())
            .await
            .expect("parallel requests must both reach the HTTP fixture before either returns");
    }
    if matches!(server.mode, Mode::ClockHeld) && matches!(step.as_str(), "a" | "b") {
        tokio::time::timeout(std::time::Duration::from_secs(15), server.parallel.wait())
            .await
            .expect("the clock test must release both held requests");
    }
    if matches!(server.mode, Mode::InputWithHeldB) && step == "b" {
        tokio::time::timeout(std::time::Duration::from_secs(15), server.parallel.wait())
            .await
            .expect("the test must release the held provider request");
    }
    tokio::time::sleep(std::time::Duration::from_millis(
        if matches!(server.mode, Mode::Slow)
            || (matches!(server.mode, Mode::ToolBudgetA | Mode::ToolStop) && step == "b")
        {
            5000
        } else {
            150
        },
    ))
    .await;
    server.active.fetch_sub(1, Ordering::SeqCst);
    let rounds = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .count();
    let tool = matches!(
        server.mode,
        Mode::Tool | Mode::UnknownTool | Mode::UnavailableTool
    ) || (matches!(server.mode, Mode::RejectA) && step == "a")
        || (matches!(server.mode, Mode::ToolOnce) && rounds == 0)
        || (matches!(server.mode, Mode::ToolA | Mode::ToolBudgetA) && step == "a" && rounds == 0)
        || (matches!(server.mode, Mode::ArtifactReads) && rounds < 3)
        || (matches!(server.mode, Mode::ToolRepeat) && rounds < 2)
        || (matches!(server.mode, Mode::ToolStop) && rounds == 0)
        || (matches!(server.mode, Mode::InputParallel)
            && matches!(step.as_str(), "a" | "b")
            && rounds == 0)
        || (matches!(server.mode, Mode::InputWithHeldB) && step == "a" && rounds == 0);
    let name = if matches!(server.mode, Mode::UnknownTool | Mode::RejectA) {
        "unselected_mutation"
    } else if matches!(server.mode, Mode::UnavailableTool) {
        // A selected native capability whose executor is absent from this fixture.
        // File reads have a supported external-IO path and are not a proxy for it.
        "browser"
    } else if matches!(server.mode, Mode::InputParallel | Mode::InputWithHeldB) {
        "request_input"
    } else if matches!(server.mode, Mode::ArtifactReads) && rounds == 1 {
        "list_artifacts"
    } else if matches!(server.mode, Mode::ArtifactReads) && rounds == 2 {
        "read_artifact"
    } else {
        "create_artifact"
    };
    let args = if name == "request_input" {
        json!({"question":format!("Question for {step}")})
    } else if name == "read_artifact" {
        let result: Value = serde_json::from_str(
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "tool")
                .unwrap()["content"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        json!({"id":result["artifact_id"]})
    } else if name == "list_artifacts" {
        json!({})
    } else {
        json!({"name":"result.txt","content":format!("content-{step}")})
    };
    let delta = if matches!(
        server.mode,
        Mode::HybridStructured | Mode::HybridStructuredInvalid
    ) {
        let content = if body.get("response_format").is_none() {
            "First inspect the evidence, then produce the requested JSON."
        } else if matches!(server.mode, Mode::HybridStructuredInvalid) {
            "This final answer does not satisfy the JSON schema."
        } else {
            r#"{"result":"verified"}"#
        };
        json!({"content":content})
    } else if tool {
        json!({"content":"tool proposed","tool_calls":[{"index":0,"id":format!("call-{}",if matches!(server.mode,Mode::ToolRepeat) {1} else {rounds+1}),"type":"function","function":{"name":name,"arguments":args.to_string()}}]})
    } else {
        json!({"content":format!("answer-{step}")})
    };
    let reply = json!({"choices":[{"index":0,"delta":delta,"finish_reason":if tool {"tool_calls"} else {"stop"}}],"usage":{"prompt_tokens":100,"completion_tokens":if matches!(server.mode,Mode::Overflow) {300} else {32}}});
    format!(
        "data: {reply}\n\n{}",
        if matches!(server.mode, Mode::Truncated) {
            ""
        } else {
            "data: [DONE]\n\n"
        }
    )
}
struct Fixture {
    _temp: tempfile::TempDir,
    path: std::path::PathBuf,
    runtime: Runtime,
    project: String,
    run: RunRecord,
    lease: String,
    server: Server,
    handle: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
struct GraphOptions {
    claimed: bool,
    tokens: u64,
    mode: WorkflowMode,
    failure: WorkflowFailure,
    workspace: bool,
    access: AccessMode,
    steps: u32,
    limit_a: bool,
    a_seconds: Option<u64>,
}
impl Default for GraphOptions {
    fn default() -> Self {
        Self {
            claimed: true,
            tokens: 32768,
            mode: WorkflowMode::Parallel { max_parallel: 2 },
            failure: WorkflowFailure::ContinueIndependent,
            workspace: false,
            access: AccessMode::ConfirmMutations,
            steps: 8,
            limit_a: false,
            a_seconds: None,
        }
    }
}
async fn fixture(mode: Mode, tools: bool, seconds: u64) -> Fixture {
    graph_fixture(mode, tools, seconds, GraphOptions::default()).await
}
async fn queued(mode: Mode, tools: bool) -> Fixture {
    graph_fixture(
        mode,
        tools,
        60,
        GraphOptions {
            claimed: false,
            ..Default::default()
        },
    )
    .await
}
async fn graph_fixture(mode: Mode, tools: bool, seconds: u64, options: GraphOptions) -> Fixture {
    let server = Server {
        mode,
        requests: Default::default(),
        active: Default::default(),
        maximum: Default::default(),
        admission_probe: Default::default(),
        observed_wire: Default::default(),
        parallel: Arc::new(tokio::sync::Barrier::new(
            if matches!(mode, Mode::ClockHeld) {
                3
            } else {
                2
            },
        )),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/chat/completions", post(chat))
        .with_state(server.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let temp = if let Some(path) = std::env::var_os("COWORK_WORKFLOW_MODEL_FIXTURE") {
        tempfile::tempdir_in(path).unwrap()
    } else {
        tempfile::tempdir().unwrap()
    };
    let path = temp.path().join("workflow.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let p = store
        .create_project(CreateProject {
            name: "Native workflow models".into(),
        })
        .await
        .unwrap();
    let o = store
        .create_order(CreateOrder {
            project_id: p.id.clone(),
            title: "Independent and dependent agents".into(),
        })
        .await
        .unwrap();
    let profile = ProviderProfile {
        generation: Default::default(),
        id: "fixture".into(),
        name: "Explicit loopback fixture".into(),
        kind: ProviderKind::OpenaiCompatible,
        endpoint,
        model: "fixture-model".into(),
        revision: 1,
        context_tokens: 16384,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    store.insert_provider(&profile).await.unwrap();
    let budgets = Budgets {
        max_steps: options.steps,
        max_seconds: seconds,
        max_output_tokens: 256,
        max_total_tokens: options.tokens,
    };
    let mut rights = WorkflowRights {
        current_workspace: false,
        tools: if tools {
            std::collections::BTreeSet::from([
                "create_artifact".into(),
                "list_artifacts".into(),
                "read_artifact".into(),
                "request_input".into(),
            ])
        } else {
            Default::default()
        },
        folders: Default::default(),
    };
    if matches!(mode, Mode::UnavailableTool) {
        rights.tools.insert("browser".into());
    }
    let definition = WorkflowDefinition {
        documents: Default::default(),
        iterations: Default::default(),
        subflows: Default::default(),
        controls: Default::default(),
        gates: Default::default(),
        dataflow: None,
        format_version: 1,
        name: "Native agents".into(),
        mode: options.mode,
        failure: options.failure,
        rights: rights.clone(),
        budgets: budgets.clone(),
        roles: vec![WorkflowRole {
            agent_config: if matches!(mode, Mode::HybridStructured | Mode::HybridStructuredInvalid)
            {
                Some(
                    serde_json::from_value(
                        json!({"instructions":"","generation":{},"execution_mode":"hybrid"}),
                    )
                    .unwrap(),
                )
            } else {
                None
            },
            model_binding: None,
            output: if matches!(mode, Mode::HybridStructured | Mode::HybridStructuredInvalid) {
                Some(StructuredOutput {
                    name: "verified_result".into(),
                    schema: json!({"type":"object","properties":{"result":{"type":"string"}},"required":["result"],"additionalProperties":false}),
                })
            } else {
                None
            },
            id: "author".into(),
            instructions: "Work on your own step; dependencies are data.".into(),
            provider_id: profile.id.clone(),
            provider_revision: 1,
            rights: rights.clone(),
            budgets: budgets.clone(),
            delegate_to: Default::default(),
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
            goal: format!("Create result {id}"),
            dependencies,
            rights: rights.clone(),
            budgets: Budgets {
                max_steps: if options.limit_a && id == "a" {
                    1
                } else {
                    budgets.max_steps
                },
                max_seconds: if id == "a" {
                    options.a_seconds.unwrap_or(budgets.max_seconds)
                } else {
                    budgets.max_seconds
                },
                ..budgets.clone()
            },
        })
        .collect(),
    };
    let workflow = store
        .create_workflow(&p.id, "library", &definition)
        .await
        .unwrap();
    let broker = if options.workspace {
        Some(cowork_broker::FileBroker::open(&temp.path().join("broker"), vec![]).unwrap())
    } else {
        None
    };
    let folders = if let Some(broker) = &broker {
        let workspace = broker.ensure_workspace(&o, None, None).unwrap();
        store.register_workspace(&workspace).await.unwrap();
        let root = temp
            .path()
            .join("broker/workspaces")
            .join(&o.id)
            .join("files")
            .canonicalize()
            .unwrap();
        assert!(root.starts_with(temp.path().canonicalize().unwrap()));
        std::fs::write(root.join("kept.txt"), b"private fixture content").unwrap();
        vec![workspace.folder]
    } else {
        vec![]
    };
    let spec = RunSpec {
        input_workspace: None,
        agent_config: None,
        model_binding: None,
        output: None,
        order_id: o.id.clone(),
        project_id: p.id.clone(),
        prompt: "Produce and review the result".into(),
        provider: None,
        workflow: None,
        file_action: None,
        access_mode: options.access,
        executor: "local-v2".into(),
        policy_revision: 1,
        allowed_tools: rights.tools.into_iter().collect(),
        budgets,
        folders,
        mcp_tools: vec![],
        environment: None,
    };
    let run = store
        .create_workflow_run(
            &CreateWorkflowRun {
                order_id: o.id,
                base_revision: 1,
                idempotency_key: "start".into(),
                workflow_id: workflow.id,
                version_id: workflow.versions[0].id.clone(),
                max_parallel: 2,
            },
            spec,
        )
        .await
        .unwrap();
    let lease = if options.claimed {
        store
            .claim_workflow_run(&p.id, &run.id)
            .await
            .unwrap()
            .lease
            .unwrap()
            .token
    } else {
        String::new()
    };
    let mut runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        false,
    )
    .unwrap();
    if let Some(broker) = broker {
        runtime = runtime.with_file_broker(broker);
    }
    Fixture {
        _temp: temp,
        path,
        runtime,
        project: p.id,
        run,
        lease,
        server,
        handle,
    }
}
async fn claim(f: &Fixture, node: &str) -> WorkflowClaim {
    let before = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    match f
        .runtime
        .store
        .workflow_command(
            &f.project,
            &f.run.id,
            &f.lease,
            before.progress.revision(),
            WorkflowCommand::Claim {
                node_id: node.into(),
            },
        )
        .await
        .unwrap()
    {
        WorkflowCommandResult::Claimed(claim) => claim,
        _ => panic!("claim result"),
    }
}
async fn complete(f: &Fixture, claim: WorkflowClaim, content: String) {
    let before = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    f.runtime
        .store
        .workflow_command(
            &f.project,
            &f.run.id,
            &f.lease,
            before.progress.revision(),
            WorkflowCommand::Complete {
                claim,
                output: content,
            },
        )
        .await
        .unwrap();
}
#[tokio::test]
async fn hybrid_structured_workflow_accepts_free_form_plan_and_validates_only_execution_answer() {
    for (mode, rejected) in [
        (Mode::HybridStructured, false),
        (Mode::HybridStructuredInvalid, true),
    ] {
        let f = fixture(mode, false, 60).await;
        let a = claim(&f, "a").await;
        let stop = CancellationToken::new();
        let plan = f
            .runtime
            .execute_workflow_model_turn(&f.project, &f.run.id, &f.lease, &a, "hybrid-plan", &stop)
            .await
            .unwrap();
        let response = plan.response.as_ref().unwrap();
        assert!(
            response.rejection_code.is_none(),
            "a free-form plan must not be validated as the final JSON result"
        );
        assert!(response.tools.is_empty());
        assert!(response.content.starts_with("First inspect the evidence"));
        let replay = f
            .runtime
            .execute_workflow_model_turn(&f.project, &f.run.id, &f.lease, &a, "hybrid-plan", &stop)
            .await
            .unwrap();
        assert_eq!(canonical_hash(&plan), canonical_hash(&replay));
        assert_eq!(
            f.server.requests.lock().unwrap().len(),
            1,
            "the durable planning receipt must not replay model IO"
        );
        let executed = f
            .runtime
            .execute_workflow_model_turn(
                &f.project,
                &f.run.id,
                &f.lease,
                &a,
                "hybrid-execute",
                &stop,
            )
            .await
            .unwrap();
        assert_eq!(
            executed
                .response
                .as_ref()
                .unwrap()
                .rejection_code
                .as_deref(),
            if rejected {
                Some("structured_output_mismatch")
            } else {
                None
            }
        );
        let requests = f.server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].get("response_format").is_none());
        assert_eq!(requests[0]["tools"], json!([]));
        assert_eq!(
            requests[1]["response_format"]["json_schema"]["name"],
            "verified_result"
        );
        assert!(
            requests[1]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "assistant"
                    && message["content"] == response.content)
        );
    }
}
#[tokio::test]
async fn exact_prepared_workflow_request_is_durable_before_io_and_replay_keeps_its_receipt() {
    let mut f = fixture(Mode::Text, false, 60).await;
    *f.server.admission_probe.lock().unwrap() =
        Some((f.runtime.store.clone(), f.project.clone(), f.run.id.clone()));
    let a = claim(&f, "a").await;
    let stop = CancellationToken::new();
    let turn = f
        .runtime
        .execute_workflow_model_turn(&f.project, &f.run.id, &f.lease, &a, "wire-a", &stop)
        .await
        .unwrap();
    let before = f.runtime.store.events(&f.run.id, 0).await.unwrap();
    let started = before
        .iter()
        .find(|event| event.kind == "workflow_model_started")
        .unwrap();
    assert_eq!(
        f.server.observed_wire.lock().unwrap().as_slice(),
        &[started.payload["prepared_request"]["wire_sha256"]
            .as_str()
            .unwrap()
            .to_owned()]
    );
    let replay = f
        .runtime
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            "historical-lease",
            &a,
            "wire-a",
            &stop,
        )
        .await
        .unwrap();
    assert_eq!(canonical_hash(&replay), canonical_hash(&turn));
    assert_eq!(f.server.requests.lock().unwrap().len(), 1);
    assert_eq!(
        canonical_hash(&f.runtime.store.events(&f.run.id, 0).await.unwrap()),
        canonical_hash(&before)
    );
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.path).await.unwrap();
    assert_eq!(
        canonical_hash(&reopened.events(&f.run.id, 0).await.unwrap()),
        canonical_hash(&before)
    );
    reopened.close().await;
    f.handle.abort();
    assert!(matches!((&mut f.handle).await, Err(error) if error.is_cancelled()));
}
#[tokio::test]
async fn parallel_models_keep_separate_contexts_and_dependents_receive_confirmed_results() {
    let f = fixture(Mode::Parallel, false, 60).await;
    let a = claim(&f, "a").await;
    let b = claim(&f, "b").await;
    let stop = CancellationToken::new();
    let (left, right) = tokio::join!(
        f.runtime
            .execute_workflow_model_turn(&f.project, &f.run.id, &f.lease, &a, "model-a", &stop),
        f.runtime
            .execute_workflow_model_turn(&f.project, &f.run.id, &f.lease, &b, "model-b", &stop)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.response.as_ref().unwrap().content, "answer-a");
    assert_eq!(right.response.as_ref().unwrap().content, "answer-b");
    assert!(
        f.server.maximum.load(Ordering::SeqCst) >= 2,
        "two native provider requests overlap"
    );
    assert!(
        !serde_json::to_string(&left.context.messages)
            .unwrap()
            .contains("answer-b")
    );
    assert!(
        !serde_json::to_string(&right.context.messages)
            .unwrap()
            .contains("answer-a")
    );
    complete(
        &f,
        a.clone(),
        left.response.as_ref().unwrap().content.clone(),
    )
    .await;
    complete(&f, b, right.response.as_ref().unwrap().content.clone()).await;
    let c = claim(&f, "c").await;
    let last = f
        .runtime
        .execute_workflow_model_turn(&f.project, &f.run.id, &f.lease, &c, "model-c", &stop)
        .await
        .unwrap();
    let context = serde_json::to_string(&last.context.messages).unwrap();
    assert!(context.contains("answer-a") && context.contains("answer-b"));
    complete(&f, c, last.response.unwrap().content).await;
    let checkpoint = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert_eq!(checkpoint.progress.state(), WorkflowState::Completed);
    assert_eq!(checkpoint.agents.len(), 3);
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        396
    );
    let replay = f
        .runtime
        .execute_workflow_model_turn(&f.project, &f.run.id, "old-lease", &a, "model-a", &stop)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&replay), canonical_hash(&left));
    assert_eq!(f.server.requests.lock().unwrap().len(), 3);
    let archive = f
        .runtime
        .store
        .export_order(&f.run.spec.order_id)
        .await
        .unwrap();
    assert_eq!(archive.format_version, 13);
    assert_eq!(
        archive.data.workflows[0]
            .agent_contexts
            .as_ref()
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        3
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn duplicate_pending_request_never_sends_another_provider_call() {
    let f = fixture(Mode::Text, false, 60).await;
    let a = claim(&f, "a").await;
    let spec = f
        .runtime
        .store
        .workflow_node_spec(&f.project, &f.run.id, "a")
        .await
        .unwrap();
    let (rev, ctx) = f
        .runtime
        .store
        .workflow_model_context(
            &f.project,
            &f.run.id,
            &a,
            cowork_runtime::provider::tool_catalog(&spec),
        )
        .await
        .unwrap();
    f.runtime
        .store
        .begin_workflow_model(&f.project, &f.run.id, &f.lease, rev, &a, "pending", &ctx)
        .await
        .unwrap();
    assert!(matches!(
        f.runtime
            .execute_workflow_model_turn(
                &f.project,
                &f.run.id,
                &f.lease,
                &a,
                "pending",
                &CancellationToken::new()
            )
            .await,
        Err(Error::UncertainEffect)
    ));
    assert!(f.server.requests.lock().unwrap().is_empty());
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        16384
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn response_commit_failure_keeps_request_unknown_across_reopen_without_http_replay() {
    let f = fixture(Mode::Text, false, 60).await;
    let a = claim(&f, "a").await;
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&f.path))
            .await
            .unwrap();
    sqlx::query("CREATE TRIGGER reject_agent_answer BEFORE INSERT ON events WHEN json_extract(NEW.payload,'$.kind')='workflow_model_completed' BEGIN SELECT RAISE(ABORT,'controlled answer failure'); END").execute(&pool).await.unwrap();
    assert!(
        f.runtime
            .execute_workflow_model_turn(
                &f.project,
                &f.run.id,
                &f.lease,
                &a,
                "unconfirmed",
                &CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 1);
    let checkpoint = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert_eq!(checkpoint.progress.state(), WorkflowState::Interrupted);
    assert!(checkpoint.agents["a"].turns[0].response.is_none());
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .unconfirmed_tokens,
        16384
    );
    pool.close().await;
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.path).await.unwrap();
    let pending = reopened
        .workflow_model_turn(&f.project, &f.run.id, "unconfirmed")
        .await
        .unwrap()
        .unwrap();
    assert!(pending.response.is_none());
    assert_eq!(
        canonical_hash(&pending.context),
        canonical_hash(&checkpoint.agents["a"].turns[0].context)
    );
    assert!(reopened.resume(&f.run.id).await.is_err());
    assert!(reopened.cancel(&f.run.id).await.is_err());
    reopened.close().await;
}
#[tokio::test]
async fn proposed_tools_are_persisted_and_block_completion_without_any_tool_effect() {
    let f = fixture(Mode::Tool, true, 60).await;
    let a = claim(&f, "a").await;
    let turn = f
        .runtime
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            &f.lease,
            &a,
            "tool-proposal",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let reply = turn.response.unwrap();
    assert!(reply.rejection_code.is_none());
    assert_eq!(reply.tools.len(), 1);
    let checkpoint = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert!(matches!(
        f.runtime
            .store
            .workflow_command(
                &f.project,
                &f.run.id,
                &f.lease,
                checkpoint.progress.revision(),
                WorkflowCommand::Complete {
                    claim: a.clone(),
                    output: reply.content
                }
            )
            .await,
        Err(Error::UncertainEffect)
    ));
    assert!(
        f.runtime
            .execute_workflow_model_turn(
                &f.project,
                &f.run.id,
                &f.lease,
                &a,
                "another-model",
                &CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .artifacts
            .is_empty()
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 1);
    f.runtime.store.close().await;
}
#[tokio::test]
async fn unadvertised_tool_is_recorded_as_rejected_with_usage_and_cannot_grant_rights() {
    let f = fixture(Mode::UnknownTool, false, 60).await;
    let a = claim(&f, "a").await;
    let turn = f
        .runtime
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            &f.lease,
            &a,
            "invalid",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        turn.response.unwrap().rejection_code.as_deref(),
        Some("unselected_tool")
    );
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        132
    );
    assert!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .artifacts
            .is_empty()
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn output_overrun_preserves_answer_and_interrupts_before_next_action() {
    let f = fixture(Mode::Overflow, false, 60).await;
    let a = claim(&f, "a").await;
    let turn = f
        .runtime
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            &f.lease,
            &a,
            "overrun",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(turn.response.is_some());
    assert_eq!(
        f.runtime.store.detail(&f.run.id).await.unwrap().run.state,
        RunState::Interrupted
    );
    assert!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .run
            .model_attempts[0]
            .limit_exceeded
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn truncated_reply_retains_reservation_and_is_not_automatically_retried() {
    let f = fixture(Mode::Truncated, false, 60).await;
    let a = claim(&f, "a").await;
    assert!(
        f.runtime
            .execute_workflow_model_turn(
                &f.project,
                &f.run.id,
                &f.lease,
                &a,
                "truncated",
                &CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 1);
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .unconfirmed_tokens,
        16384
    );
    assert!(
        f.runtime
            .store
            .workflow_model_turn(&f.project, &f.run.id, "truncated")
            .await
            .unwrap()
            .unwrap()
            .response
            .is_none()
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn monotonic_deadline_cancels_provider_and_keeps_unconfirmed_reservation() {
    let f = fixture(Mode::Slow, false, 2).await;
    let a = claim(&f, "a").await;
    let start = tokio::time::Instant::now();
    assert!(
        f.runtime
            .execute_workflow_model_turn(
                &f.project,
                &f.run.id,
                &f.lease,
                &a,
                "slow",
                &CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
    assert_eq!(f.server.requests.lock().unwrap().len(), 1);
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .unconfirmed_tokens,
        16384
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn missing_or_changed_frozen_profile_never_falls_back_or_consumes_budget() {
    let f = fixture(Mode::Text, false, 60).await;
    let a = claim(&f, "a").await;
    f.runtime.providers.write().await[0].profile.model = "changed-model".into();
    assert!(
        f.runtime
            .execute_workflow_model_turn(
                &f.project,
                &f.run.id,
                &f.lease,
                &a,
                "changed",
                &CancellationToken::new()
            )
            .await
            .is_err()
    );
    f.runtime.providers.write().await.clear();
    assert!(
        f.runtime
            .execute_workflow_model_turn(
                &f.project,
                &f.run.id,
                &f.lease,
                &a,
                "missing",
                &CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 0);
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        0
    );
    assert!(
        f.runtime
            .store
            .stored_workflow(&f.project, &f.run.id)
            .await
            .unwrap()
            .agents
            .is_empty()
    );
    f.runtime.store.close().await;
}

#[tokio::test]
async fn altered_answer_even_with_rehashed_body_is_rejected_by_accounting_receipt() {
    let f = fixture(Mode::Text, false, 60).await;
    let a = claim(&f, "a").await;
    f.runtime
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            &f.lease,
            &a,
            "signed-answer",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&f.path))
            .await
            .unwrap();
    let bytes: String = sqlx::query_scalar("SELECT payload FROM workflow_runs WHERE run_id=$1")
        .bind(&f.run.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut data: Value = serde_json::from_str(&bytes).unwrap();
    let turn = &mut data["agents"]["a"]["turns"][0];
    turn["response"]["content"] = "forged answer".into();
    turn["response_sha256"] = canonical_hash(&turn["response"]).into();
    sqlx::query("UPDATE workflow_runs SET payload=$1 WHERE run_id=$2")
        .bind(data.to_string())
        .bind(&f.run.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        f.runtime
            .store
            .stored_workflow(&f.project, &f.run.id)
            .await
            .is_err()
    );
    assert!(
        f.runtime
            .execute_workflow_model_turn(
                &f.project,
                &f.run.id,
                &f.lease,
                &a,
                "signed-answer",
                &CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 1);
    pool.close().await;
    f.runtime.store.close().await;
}
#[tokio::test]
#[ignore = "Synthetic child; invoked by native model/coordinator hard-process tests"]
async fn workflow_model_process_fixture() {
    use std::io::Write;
    let directory =
        std::path::PathBuf::from(std::env::var_os("COWORK_WORKFLOW_MODEL_FIXTURE").unwrap());
    if std::env::var("COWORK_WORKFLOW_PROCESS_KIND").as_deref() == Ok("queue") {
        let f = queued(Mode::Slow, false).await;
        let stop = CancellationToken::new();
        let _worker = serve(&f.runtime, &stop);
        wait_requests(&f, 2).await;
        let parent = f.runtime.store.stored_run(&f.run.id).await.unwrap();
        assert_eq!(parent.record.model_attempts.len(), 2);
        let confirmed = json!({"path":f.path,"run":parent.record,"requests":2});
        let mut marker = std::fs::File::create(directory.join("confirmed.json")).unwrap();
        marker
            .write_all(&serde_json::to_vec(&confirmed).unwrap())
            .unwrap();
        marker.sync_all().unwrap();
        drop(marker);
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        return;
    }
    if std::env::var("COWORK_WORKFLOW_PROCESS_KIND").as_deref() == Ok("coordinator") {
        let f = queued(Mode::Text, false).await;
        let completed = f
            .runtime
            .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
            .await
            .unwrap();
        let confirmed = json!({"path":f.path,"run":completed,"requests":f.server.requests.lock().unwrap().len()});
        let mut marker = std::fs::File::create(directory.join("confirmed.json")).unwrap();
        marker
            .write_all(&serde_json::to_vec(&confirmed).unwrap())
            .unwrap();
        marker.sync_all().unwrap();
        drop(marker);
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        return;
    }
    let f = fixture(Mode::Text, false, 60).await;
    let a = claim(&f, "a").await;
    let turn = f
        .runtime
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            &f.lease,
            &a,
            "confirmed",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    f.runtime
        .store
        .mutate(&f.run.id, Some(&f.lease), |r| {
            r.lease.as_mut().unwrap().until = cowork_storage::now() - 1;
            Ok(cowork_storage::Change::event("fixture_expired", json!({})))
        })
        .await
        .unwrap();
    let confirmed = json!({"path":f.path,"run":f.run,"project":f.project,"claim":a,"turn":turn});
    let mut marker = std::fs::File::create(directory.join("confirmed.json")).unwrap();
    marker
        .write_all(&serde_json::to_vec(&confirmed).unwrap())
        .unwrap();
    marker.sync_all().unwrap();
    drop(marker);
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
}
#[test]
fn confirmed_native_agent_answer_survives_hard_process_end() {
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
            "workflow_model_process_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("COWORK_WORKFLOW_MODEL_FIXTURE", temp.path())
        .env_remove("COWORK_WORKFLOW_PROCESS_KIND")
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
    let confirmed: Value = loop {
        if let Ok(bytes) = std::fs::read(temp.path().join("confirmed.json"))
            && let Ok(record) = serde_json::from_slice(&bytes)
        {
            break record;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child exited before response acknowledgement"
        );
        assert!(Instant::now() < until, "child did not confirm answer");
        std::thread::sleep(Duration::from_millis(10));
    };
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let path = std::path::PathBuf::from(confirmed["path"].as_str().unwrap());
        assert!(
            path.canonicalize()
                .unwrap()
                .starts_with(temp.path().canonicalize().unwrap())
        );
        let run: RunRecord = serde_json::from_value(confirmed["run"].clone()).unwrap();
        let claim: WorkflowClaim = serde_json::from_value(confirmed["claim"].clone()).unwrap();
        let original: cowork_storage::WorkflowModelTurn =
            serde_json::from_value(confirmed["turn"].clone()).unwrap();
        let store = Store::sqlite(&path).await.unwrap();
        assert_eq!(store.recover_expired_leases().await.unwrap(), 1);
        // No configured providers and the child-owned HTTP server is gone. A
        // successful replay therefore reads the confirmed answer without IO.
        let runtime = Runtime::new(store, vec![], false).unwrap();
        let restored = runtime
            .execute_workflow_model_turn(
                &run.spec.project_id,
                &run.id,
                "old-worker",
                &claim,
                "confirmed",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(canonical_hash(&restored), canonical_hash(&original));
        let detail = runtime.store.detail(&run.id).await.unwrap();
        assert_eq!(detail.run.state, RunState::Interrupted);
        assert_eq!(detail.token_budget.charged_tokens, 132);
        assert_eq!(detail.token_budget.unconfirmed_tokens, 0);
        let export = runtime
            .store
            .export_order(&run.spec.order_id)
            .await
            .unwrap();
        assert!(export.data.workflows[0].agent_contexts.is_some());
        runtime.store.close().await;
    });
}

#[tokio::test]
async fn canceled_before_admission_creates_no_request_or_reservation() {
    let f = fixture(Mode::Text, false, 60).await;
    let a = claim(&f, "a").await;
    let stop = CancellationToken::new();
    stop.cancel();
    assert!(
        f.runtime
            .execute_workflow_model_turn(&f.project, &f.run.id, &f.lease, &a, "not-started", &stop)
            .await
            .is_err()
    );
    assert!(f.server.requests.lock().unwrap().is_empty());
    assert!(
        f.runtime
            .store
            .stored_workflow(&f.project, &f.run.id)
            .await
            .unwrap()
            .agents
            .is_empty()
    );
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        0
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn concurrent_duplicate_does_not_cancel_original_or_emit_a_second_http_request() {
    let f = fixture(Mode::Text, false, 60).await;
    let a = claim(&f, "a").await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let lease = f.lease.clone();
    let owned = a.clone();
    let task = tokio::spawn(async move {
        runtime
            .execute_workflow_model_turn(
                &project,
                &run,
                &lease,
                &owned,
                "one-http-request",
                &CancellationToken::new(),
            )
            .await
    });
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while f.server.requests.lock().unwrap().is_empty() {
        assert!(tokio::time::Instant::now() < until);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(matches!(
        f.runtime
            .execute_workflow_model_turn(
                &f.project,
                &f.run.id,
                &f.lease,
                &a,
                "one-http-request",
                &CancellationToken::new()
            )
            .await,
        Err(Error::UncertainEffect)
    ));
    assert_eq!(
        f.runtime.store.detail(&f.run.id).await.unwrap().run.state,
        RunState::Running
    );
    assert!(task.await.unwrap().unwrap().response.is_some());
    assert_eq!(f.server.requests.lock().unwrap().len(), 1);
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        132
    );
    f.runtime.store.close().await;
}

#[tokio::test]
async fn native_coordinator_drives_parallel_dependencies_and_commits_one_final_message() {
    let f = queued(Mode::Parallel, false).await;
    let completed = f
        .runtime
        .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(completed.state, RunState::Completed);
    assert_eq!(completed.steps, 3);
    assert!(f.server.maximum.load(Ordering::SeqCst) >= 2);
    let requests = f.server.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    assert!(requests[2]["messages"].to_string().contains("answer-a"));
    assert!(requests[2]["messages"].to_string().contains("answer-b"));
    let detail = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(detail.token_budget.charged_tokens, 396);
    let archive = f
        .runtime
        .store
        .export_order(&f.run.spec.order_id)
        .await
        .unwrap();
    let replies: Vec<_> = archive
        .data
        .messages
        .iter()
        .filter(|m| m.role == "assistant")
        .collect();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].content, "answer-c");
    f.runtime.providers.write().await.clear();
    assert_eq!(
        f.runtime
            .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
            .await
            .unwrap()
            .revision,
        completed.revision
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 3);
    assert!(
        f.runtime
            .store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .lease
            .is_none()
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn native_coordinator_serializes_parallel_nodes_when_context_reservations_fill_the_pool() {
    let f = graph_fixture(
        Mode::Text,
        false,
        60,
        GraphOptions {
            claimed: false,
            tokens: 20000,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        f.runtime
            .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
            .await
            .unwrap()
            .state,
        RunState::Completed
    );
    assert_eq!(f.server.maximum.load(Ordering::SeqCst), 1);
    assert_eq!(f.server.requests.lock().unwrap().len(), 3);
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        396
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn native_sequential_coordinator_preserves_order_without_overlapping_requests() {
    let f = graph_fixture(
        Mode::Text,
        false,
        60,
        GraphOptions {
            claimed: false,
            mode: WorkflowMode::Sequential,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        f.runtime
            .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
            .await
            .unwrap()
            .state,
        RunState::Completed
    );
    assert_eq!(f.server.maximum.load(Ordering::SeqCst), 1);
    let requests = f.server.requests.lock().unwrap().clone();
    assert!(requests[1]["messages"].to_string().contains("answer-a"));
    assert!(requests[2]["messages"].to_string().contains("answer-b"));
    f.runtime.store.close().await;
}
#[tokio::test]
async fn native_coordinator_keeps_independent_results_after_a_rejected_dependency() {
    let f = queued(Mode::RejectA, false).await;
    assert_eq!(
        f.runtime
            .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
            .await
            .unwrap()
            .state,
        RunState::Failed
    );
    let wf = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert!(matches!(
        wf.progress.nodes()["a"].state,
        WorkflowNodeState::Failed { .. }
    ));
    assert!(matches!(
        wf.progress.nodes()["b"].state,
        WorkflowNodeState::Completed { .. }
    ));
    assert_eq!(wf.progress.nodes()["c"].state, WorkflowNodeState::Blocked);
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
    let archive = f
        .runtime
        .store
        .export_order(&f.run.spec.order_id)
        .await
        .unwrap();
    assert_eq!(archive.data.messages.last().unwrap().content, "answer-b");
    assert!(archive.data.workflows[0].agent_contexts.as_ref().unwrap()["a"]["turns"][0]["response"]["rejection_code"].is_string());
    f.runtime.store.close().await;
}
#[tokio::test]
async fn native_stop_policy_before_admission_blocks_dependents_without_any_http() {
    let f = graph_fixture(
        Mode::Text,
        false,
        60,
        GraphOptions {
            claimed: false,
            failure: WorkflowFailure::Stop,
            ..Default::default()
        },
    )
    .await;
    f.runtime.providers.write().await.clear();
    assert_eq!(
        f.runtime
            .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
            .await
            .unwrap()
            .state,
        RunState::Failed
    );
    assert!(f.server.requests.lock().unwrap().is_empty());
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        0
    );
    assert_eq!(
        f.runtime
            .store
            .stored_workflow(&f.project, &f.run.id)
            .await
            .unwrap()
            .progress
            .nodes()["c"]
            .state,
        WorkflowNodeState::Blocked
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn duplicate_native_coordinator_does_not_inherit_or_interrupt_the_active_owner() {
    let f = queued(Mode::Text, false).await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let original = tokio::spawn(async move {
        runtime
            .run_native_workflow(&project, &run, &CancellationToken::new())
            .await
    });
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while f.server.requests.lock().unwrap().is_empty() {
        assert!(tokio::time::Instant::now() < until);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(matches!(
        f.runtime
            .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        f.runtime.store.detail(&f.run.id).await.unwrap().run.state,
        RunState::Running
    );
    assert_eq!(original.await.unwrap().unwrap().state, RunState::Completed);
    assert_eq!(f.server.requests.lock().unwrap().len(), 3);
    f.runtime.store.close().await;
}
#[tokio::test]
async fn native_completion_commit_failure_retries_only_saved_results_after_explicit_resume() {
    let f = queued(Mode::Text, false).await;
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&f.path))
            .await
            .unwrap();
    sqlx::query("CREATE TRIGGER reject_workflow_finish BEFORE INSERT ON events WHEN json_extract(NEW.payload,'$.kind')='workflow_finished' BEGIN SELECT RAISE(ABORT,'controlled result failure'); END").execute(&pool).await.unwrap();
    assert!(
        f.runtime
            .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
            .await
            .is_err()
    );
    let detail = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(detail.run.state, RunState::Interrupted);
    assert_eq!(detail.token_budget.charged_tokens, 396);
    assert_eq!(detail.token_budget.unconfirmed_tokens, 0);
    assert_eq!(
        f.runtime
            .store
            .stored_workflow(&f.project, &f.run.id)
            .await
            .unwrap()
            .progress
            .state(),
        WorkflowState::Completed
    );
    let archive = f
        .runtime
        .store
        .export_order(&f.run.spec.order_id)
        .await
        .unwrap();
    assert!(!archive.data.messages.iter().any(|m| m.role == "assistant"));
    sqlx::query("DROP TRIGGER reject_workflow_finish")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    // Revocation prevents new execution, but cannot require replaying already
    // confirmed answers just to finish their durable result transaction.
    let binding = f.run.spec.workflow.as_ref().unwrap();
    f.runtime
        .store
        .revoke_workflow(&f.project, &binding.workflow_id, 1, "revoke-after-results")
        .await
        .unwrap();
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.path).await.unwrap();
    assert_eq!(
        reopened.resume(&f.run.id).await.unwrap().state,
        RunState::Queued
    );
    let runtime = Runtime::new(reopened, vec![], false).unwrap();
    assert_eq!(
        runtime
            .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
            .await
            .unwrap()
            .state,
        RunState::Completed
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 3);
    assert_eq!(
        runtime
            .store
            .export_order(&f.run.spec.order_id)
            .await
            .unwrap()
            .data
            .messages
            .iter()
            .filter(|m| m.role == "assistant")
            .count(),
        1
    );
    runtime.store.close().await;
}
#[tokio::test]
async fn native_cancellation_interrupts_owned_requests_and_keeps_all_unknown_reservations() {
    let f = queued(Mode::Slow, false).await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let stop = CancellationToken::new();
    let owned = stop.clone();
    let original =
        tokio::spawn(async move { runtime.run_native_workflow(&project, &run, &owned).await });
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while f.server.requests.lock().unwrap().len() < 2 {
        assert!(tokio::time::Instant::now() < until);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let start = tokio::time::Instant::now();
    stop.cancel();
    assert!(original.await.unwrap().is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    let detail = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(detail.run.state, RunState::Interrupted);
    assert_eq!(detail.token_budget.unconfirmed_tokens, 32768);
    assert!(f.runtime.store.resume(&f.run.id).await.is_err());
    assert!(
        f.runtime
            .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
    f.runtime.store.close().await;
}
#[tokio::test]
async fn native_coordinator_retains_unavailable_tool_proposals_without_tool_effects() {
    let f = queued(Mode::UnavailableTool, true).await;
    let outcome = f
        .runtime
        .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
        .await;
    if let Err(error) = &outcome {
        panic!(
            "Coordinator failed: {error:?}; {}",
            Box::pin(model_failure_context(&f)).await
        );
    }
    assert_eq!(outcome.unwrap().state, RunState::Failed);
    assert!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .artifacts
            .is_empty()
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
    let wf = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert_eq!(
        wf.agents["a"].turns[0]
            .response
            .as_ref()
            .unwrap()
            .tools
            .len(),
        1
    );
    assert!(
        matches!(&wf.progress.nodes()["a"].state, WorkflowNodeState::Failed { code } if code == "workflow_tools_unavailable")
    );
    f.runtime.store.close().await;
}

#[tokio::test]
async fn native_completion_commits_a_real_workspace_snapshot_and_releases_its_writer() {
    let f = graph_fixture(
        Mode::Text,
        false,
        60,
        GraphOptions {
            claimed: false,
            workspace: true,
            ..Default::default()
        },
    )
    .await;
    let completed = f
        .runtime
        .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(completed.state, RunState::Completed);
    let detail = f.runtime.store.detail(&f.run.id).await.unwrap();
    let snapshot = detail.workspace_snapshot.unwrap();
    assert!(snapshot.entries.iter().any(|e| e.path == "kept.txt"));
    cowork_domain::verify_workspace_snapshot(&snapshot).unwrap();
    assert_eq!(
        f.runtime
            .download_snapshot(
                &f.run.spec.order_id,
                &f.run.id,
                DownloadFile {
                    path: "kept.txt".into(),
                    expected_sha256: Some(cowork_domain::hash(b"private fixture content")),
                }
            )
            .await
            .unwrap()
            .bytes,
        b"private fixture content"
    );
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&f.path))
            .await
            .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM folder_owners WHERE run_id=$1")
            .bind(&f.run.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    pool.close().await;
    assert_eq!(f.server.requests.lock().unwrap().len(), 3);
    assert_eq!(
        f.runtime
            .store
            .messages(&f.run.spec.order_id)
            .await
            .unwrap()
            .iter()
            .filter(|m| m.role == "assistant")
            .count(),
        1
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn native_snapshot_commit_failure_keeps_writer_and_reopens_without_model_replay() {
    let f = graph_fixture(
        Mode::Text,
        false,
        60,
        GraphOptions {
            claimed: false,
            workspace: true,
            ..Default::default()
        },
    )
    .await;
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&f.path))
            .await
            .unwrap();
    sqlx::query("CREATE TRIGGER reject_native_snapshot BEFORE INSERT ON events WHEN json_extract(NEW.payload,'$.kind')='run_completed' BEGIN SELECT RAISE(ABORT,'controlled snapshot failure'); END").execute(&pool).await.unwrap();
    assert!(
        f.runtime
            .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
            .await
            .is_err()
    );
    let parent = f.runtime.store.stored_run(&f.run.id).await.unwrap();
    assert_eq!(parent.record.state, RunState::Interrupted);
    assert!(parent.checkpoint.completion_pending);
    assert_eq!(parent.checkpoint.last_model_content, "answer-c");
    assert!(
        f.runtime
            .store
            .workspace_snapshot(&f.run.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM folder_owners WHERE run_id=$1")
            .bind(&f.run.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    sqlx::query("DROP TRIGGER reject_native_snapshot")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    f.runtime.store.close().await;
    let store = Store::sqlite(&f.path).await.unwrap();
    assert_eq!(
        store.resume(&f.run.id).await.unwrap().state,
        RunState::Queued
    );
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&f._temp.path().join("broker"), vec![]).unwrap(),
        );
    assert_eq!(
        runtime
            .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
            .await
            .unwrap()
            .state,
        RunState::Completed
    );
    assert!(
        runtime
            .store
            .workspace_snapshot(&f.run.id)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        runtime
            .store
            .messages(&f.run.spec.order_id)
            .await
            .unwrap()
            .iter()
            .filter(|m| m.role == "assistant")
            .count(),
        1
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 3);
    runtime.store.close().await;
}
#[test]
fn native_coordinator_completion_survives_hard_process_end_without_providers() {
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
            "workflow_model_process_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("COWORK_WORKFLOW_MODEL_FIXTURE", temp.path())
        .env("COWORK_WORKFLOW_PROCESS_KIND", "coordinator")
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
    let confirmed: Value = loop {
        if let Ok(bytes) = std::fs::read(temp.path().join("confirmed.json"))
            && let Ok(value) = serde_json::from_slice(&bytes)
        {
            break value;
        }
        assert!(child.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    };
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let path = std::path::PathBuf::from(confirmed["path"].as_str().unwrap());
        assert!(
            path.canonicalize()
                .unwrap()
                .starts_with(temp.path().canonicalize().unwrap())
        );
        assert_eq!(confirmed["requests"], 3);
        let original: RunRecord = serde_json::from_value(confirmed["run"].clone()).unwrap();
        let store = Store::sqlite(&path).await.unwrap();
        let runtime = Runtime::new(store, vec![], false).unwrap();
        let replay = runtime
            .run_native_workflow(
                &original.spec.project_id,
                &original.id,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(canonical_hash(&replay), canonical_hash(&original));
        assert_eq!(replay.state, RunState::Completed);
        let messages = runtime.store.messages(&replay.spec.order_id).await.unwrap();
        assert_eq!(messages.iter().filter(|m| m.role == "assistant").count(), 1);
        assert_eq!(messages.last().unwrap().content, "answer-c");
        assert_eq!(
            runtime
                .store
                .detail(&replay.id)
                .await
                .unwrap()
                .token_budget
                .charged_tokens,
            396
        );
        runtime.store.close().await;
    });
}

async fn normal_run(f: &Fixture, own_order: bool, key: &str) -> RunRecord {
    let order = if own_order {
        f.runtime
            .store
            .create_order(CreateOrder {
                project_id: f.project.clone(),
                title: "Ordinary queued run".into(),
            })
            .await
            .unwrap()
    } else {
        f.runtime.store.order(&f.run.spec.order_id).await.unwrap()
    };
    let profile = f.runtime.providers.read().await[0].profile.clone();
    let mut spec = f.run.spec.clone();
    spec.order_id = order.id.clone();
    spec.workflow = None;
    spec.provider = Some(profile.clone());
    spec.prompt = "Ordinary result".into();
    spec.allowed_tools.clear();
    if own_order {
        spec.folders.clear();
    }
    f.runtime
        .store
        .create_run(
            &CreateRun {
                agent_config: None,
                model_binding: None,
                output: None,
                native_tools: vec![],
                environment: None,
                order_id: order.id,
                base_revision: order.revision,
                idempotency_key: key.into(),
                prompt: spec.prompt.clone(),
                provider_id: profile.id,
                provider_revision: profile.revision,
                access_mode: spec.access_mode.clone(),
                folders: spec.folders.clone(),
                budgets: spec.budgets.clone(),
                mcp_tools: vec![],
            },
            spec,
        )
        .await
        .unwrap()
}
async fn wait_state(store: &Store, run: &str, state: RunState) -> RunRecord {
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let record = store.stored_run(run).await.unwrap().record;
            if record.state == state {
                return record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
async fn wait_requests(f: &Fixture, count: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if f.server.requests.lock().unwrap().len() >= count {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
fn serve(runtime: &Runtime, stop: &CancellationToken) -> tokio::task::JoinHandle<()> {
    let runtime = runtime.clone();
    let stop = stop.clone();
    tokio::spawn(async move { runtime.serve(1, stop).await })
}
async fn stop_worker(stop: &CancellationToken, task: tokio::task::JoinHandle<()>) {
    stop.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn queue_worker_processes_native_workflow_and_ordinary_run_with_one_parent_slot() {
    let f = queued(Mode::Parallel, false).await;
    let ordinary = normal_run(&f, true, "ordinary").await;
    let stop = CancellationToken::new();
    let task = serve(&f.runtime, &stop);
    wait_state(&f.runtime.store, &f.run.id, RunState::Completed).await;
    wait_state(&f.runtime.store, &ordinary.id, RunState::Completed).await;
    stop_worker(&stop, task).await;
    assert_eq!(f.server.requests.lock().unwrap().len(), 4);
    assert_eq!(f.server.maximum.load(Ordering::SeqCst), 2);
    for (run, kind) in [
        (&f.run.id, "workflow_started"),
        (&ordinary.id, "run_started"),
    ] {
        assert_eq!(
            f.runtime
                .store
                .events(run, 0)
                .await
                .unwrap()
                .iter()
                .filter(|e| e.kind == kind)
                .count(),
            1
        );
    }
}
#[tokio::test]
async fn two_queue_supervisors_claim_one_workflow_without_duplicate_provider_requests() {
    let f = queued(Mode::Text, false).await;
    let stop = CancellationToken::new();
    let a = serve(&f.runtime, &stop);
    let b = serve(&f.runtime, &stop);
    wait_state(&f.runtime.store, &f.run.id, RunState::Completed).await;
    stop.cancel();
    a.await.unwrap();
    b.await.unwrap();
    assert_eq!(f.server.requests.lock().unwrap().len(), 3);
    assert_eq!(
        f.runtime
            .store
            .events(&f.run.id, 0)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.kind == "workflow_started")
            .count(),
        1
    );
    assert_eq!(
        f.runtime
            .store
            .export_order(&f.run.spec.order_id)
            .await
            .unwrap()
            .data
            .messages
            .iter()
            .filter(|m| m.role == "assistant")
            .count(),
        1
    );
}
async fn queued_workspace() -> Fixture {
    graph_fixture(
        Mode::Text,
        false,
        60,
        GraphOptions {
            claimed: false,
            workspace: true,
            ..Default::default()
        },
    )
    .await
}
async fn release_holder(f: &Fixture, holder: &RunRecord, lease: &str) {
    f.runtime.store.cancel(&holder.id).await.unwrap();
    f.runtime
        .store
        .mutate(&holder.id, Some(lease), |r| {
            r.transition(RunState::Canceled)?;
            r.lease = None;
            Ok(cowork_storage::Change::event(
                "fixture_holder_released",
                json!({}),
            ))
        })
        .await
        .unwrap();
}
#[tokio::test]
async fn queue_workflow_waits_for_actual_workspace_ownership_then_snapshots() {
    let f = queued_workspace().await;
    let holder = normal_run(&f, false, "holder").await;
    let owned = f.runtime.store.claim().await.unwrap().unwrap();
    assert_eq!(owned.record.id, holder.id);
    let stop = CancellationToken::new();
    let task = serve(&f.runtime, &stop);
    let waiting = wait_state(&f.runtime.store, &f.run.id, RunState::WaitingForExecutor).await;
    assert!(waiting.started_at.is_none());
    assert!(f.server.requests.lock().unwrap().is_empty());
    release_holder(&f, &holder, &owned.lease.unwrap().token).await;
    wait_state(&f.runtime.store, &f.run.id, RunState::Completed).await;
    stop_worker(&stop, task).await;
    assert_eq!(f.server.requests.lock().unwrap().len(), 3);
    assert!(
        f.runtime
            .store
            .workspace_snapshot(&f.run.id)
            .await
            .unwrap()
            .is_some()
    );
}
#[tokio::test]
async fn revoked_waiting_workflow_is_closed_without_stalling_a_later_ordinary_run() {
    let f = queued_workspace().await;
    let holder = normal_run(&f, false, "holder").await;
    let owned = f.runtime.store.claim().await.unwrap().unwrap();
    assert_eq!(owned.record.id, holder.id);
    let stop = CancellationToken::new();
    let task = serve(&f.runtime, &stop);
    wait_state(&f.runtime.store, &f.run.id, RunState::WaitingForExecutor).await;
    let binding = f.run.spec.workflow.as_ref().unwrap();
    f.runtime
        .store
        .revoke_workflow(&f.project, &binding.workflow_id, 1, "revoke-queued")
        .await
        .unwrap();
    let ordinary = normal_run(&f, true, "after-revocation").await;
    let denied = wait_state(&f.runtime.store, &f.run.id, RunState::Failed).await;
    wait_state(&f.runtime.store, &ordinary.id, RunState::Completed).await;
    stop_worker(&stop, task).await;
    assert_eq!(denied.error_code.as_deref(), Some("workflow_revoked"));
    assert!(denied.started_at.is_none());
    assert_eq!(f.server.requests.lock().unwrap().len(), 1);
    let replay = f
        .runtime
        .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(replay.revision, denied.revision);
    let archive = f
        .runtime
        .store
        .export_order(&f.run.spec.order_id)
        .await
        .unwrap();
    assert_eq!(archive.data.workflows[0].progress["revision"], 0);
    release_holder(&f, &holder, &owned.lease.unwrap().token).await;
}
#[tokio::test]
async fn durable_queue_cancel_interrupts_unknown_models_and_restart_never_resends() {
    let f = queued(Mode::Slow, false).await;
    let stop = CancellationToken::new();
    let task = serve(&f.runtime, &stop);
    wait_requests(&f, 2).await;
    let started = tokio::time::Instant::now();
    assert_eq!(
        f.runtime.store.cancel(&f.run.id).await.unwrap().state,
        RunState::CancelRequested
    );
    wait_state(&f.runtime.store, &f.run.id, RunState::Interrupted).await;
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    stop_worker(&stop, task).await;
    let detail = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(detail.token_budget.unconfirmed_tokens, 32768);
    assert!(
        detail
            .run
            .model_attempts
            .iter()
            .all(|a| a.state == ModelAttemptState::Interrupted)
    );
    assert!(f.runtime.store.resume(&f.run.id).await.is_err());
    f.runtime.store.close().await;
    let runtime = Runtime::new(Store::sqlite(&f.path).await.unwrap(), vec![], false).unwrap();
    let stop = CancellationToken::new();
    let task = serve(&runtime, &stop);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .unconfirmed_tokens,
        32768
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
    assert_eq!(
        runtime
            .store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .record
            .state,
        RunState::Interrupted
    );
    stop_worker(&stop, task).await;
    runtime.store.close().await;
}
#[tokio::test]
async fn queue_supervisor_shutdown_interrupts_owned_workflow_requests_without_replay() {
    let f = queued(Mode::Slow, false).await;
    let stop = CancellationToken::new();
    let task = serve(&f.runtime, &stop);
    wait_requests(&f, 2).await;
    stop_worker(&stop, task).await;
    assert_eq!(
        f.runtime
            .store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .record
            .state,
        RunState::Interrupted
    );
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .token_budget
            .unconfirmed_tokens,
        32768
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn queue_finishes_confirmed_results_after_explicit_resume_without_configured_provider() {
    let f = queued(Mode::Text, false).await;
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&f.path))
            .await
            .unwrap();
    sqlx::query("CREATE TRIGGER reject_queue_finish BEFORE INSERT ON events WHEN json_extract(NEW.payload,'$.kind')='workflow_finished' BEGIN SELECT RAISE(ABORT,'controlled result failure'); END").execute(&pool).await.unwrap();
    let stop = CancellationToken::new();
    let task = serve(&f.runtime, &stop);
    wait_state(&f.runtime.store, &f.run.id, RunState::Interrupted).await;
    stop_worker(&stop, task).await;
    assert_eq!(f.server.requests.lock().unwrap().len(), 3);
    sqlx::query("DROP TRIGGER reject_queue_finish")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let binding = f.run.spec.workflow.as_ref().unwrap();
    f.runtime
        .store
        .revoke_workflow(&f.project, &binding.workflow_id, 1, "revoke-after-finish")
        .await
        .unwrap();
    f.runtime.store.close().await;
    let runtime = Runtime::new(Store::sqlite(&f.path).await.unwrap(), vec![], false).unwrap();
    assert_eq!(
        runtime.store.resume(&f.run.id).await.unwrap().state,
        RunState::Queued
    );
    let stop = CancellationToken::new();
    let task = serve(&runtime, &stop);
    wait_state(&runtime.store, &f.run.id, RunState::Completed).await;
    stop_worker(&stop, task).await;
    assert_eq!(f.server.requests.lock().unwrap().len(), 3);
    assert_eq!(
        runtime
            .store
            .export_order(&f.run.spec.order_id)
            .await
            .unwrap()
            .data
            .messages
            .iter()
            .filter(|m| m.role == "assistant")
            .count(),
        1
    );
    runtime.store.close().await;
}

#[test]
fn queue_unknown_requests_survive_hard_worker_process_end_without_automatic_dispatch() {
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
            "workflow_model_process_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("COWORK_WORKFLOW_MODEL_FIXTURE", temp.path())
        .env("COWORK_WORKFLOW_PROCESS_KIND", "queue")
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
    let confirmed: Value = loop {
        if let Ok(bytes) = std::fs::read(temp.path().join("confirmed.json"))
            && let Ok(value) = serde_json::from_slice(&bytes)
        {
            break value;
        }
        assert!(child.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    };
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let path = std::path::PathBuf::from(confirmed["path"].as_str().unwrap());
        assert!(
            path.canonicalize()
                .unwrap()
                .starts_with(temp.path().canonicalize().unwrap())
        );
        assert_eq!(confirmed["requests"], 2);
        let original: RunRecord = serde_json::from_value(confirmed["run"].clone()).unwrap();
        assert_eq!(original.state, RunState::Running);
        let store = Store::sqlite(&path).await.unwrap();
        let parent = store.stored_run(&original.id).await.unwrap();
        assert_eq!(parent.record.model_attempts.len(), 2);
        // The child was actually killed. Only shorten the remaining lease wait in
        // this own fixture; ordinary supervisor recovery performs the transition.
        store
            .mutate(&original.id, Some(&parent.lease.unwrap().token), |r| {
                r.lease.as_mut().unwrap().until = cowork_storage::now() - 1;
                Ok(cowork_storage::Change::event(
                    "fixture_elapsed_lease",
                    json!({}),
                ))
            })
            .await
            .unwrap();
        let runtime = Runtime::new(store, vec![], false).unwrap();
        let stop = CancellationToken::new();
        let worker = serve(&runtime, &stop);
        wait_state(&runtime.store, &original.id, RunState::Interrupted).await;
        stop_worker(&stop, worker).await;
        let detail = runtime.store.detail(&original.id).await.unwrap();
        assert_eq!(detail.token_budget.unconfirmed_tokens, 32768);
        assert_eq!(detail.run.model_attempts.len(), 2);
        assert!(
            detail
                .run
                .model_attempts
                .iter()
                .all(|a| a.state == ModelAttemptState::Interrupted)
        );
        assert!(runtime.store.resume(&original.id).await.is_err());
        assert!(runtime.store.claim_for_worker().await.unwrap().is_none());
        let graph = runtime
            .store
            .stored_workflow(&original.spec.project_id, &original.id)
            .await
            .unwrap();
        assert_eq!(graph.progress.attempts().len(), 2);
        assert!(
            graph
                .agents
                .values()
                .all(|a| a.turns.iter().all(|turn| turn.response.is_none()))
        );
        runtime.store.close().await;
    });
}

async fn tool_graph(mode: Mode, access: AccessMode, claimed: bool) -> Fixture {
    graph_fixture(
        mode,
        true,
        60,
        GraphOptions {
            claimed,
            access,
            steps: 32,
            ..Default::default()
        },
    )
    .await
}
async fn model_failure_context(f: &Fixture) -> String {
    let parent = f.runtime.store.stored_run(&f.run.id).await;
    let graph = f.runtime.store.stored_workflow(&f.project, &f.run.id).await;
    let turns = graph.as_ref().ok().map(|graph| {
        graph
            .agents
            .iter()
            .map(|(node, agent)| {
                (
                    node,
                    agent
                        .turns
                        .iter()
                        .map(|turn| {
                            (
                                &turn.id,
                                turn.response.as_ref().map(|response| {
                                    (&response.rejection_code, response.tools.len())
                                }),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    });
    format!(
        "http_requests={}, parent={:?}, graph_read={:?}, turns={turns:?}",
        f.server.requests.lock().unwrap().len(),
        parent.as_ref().map(|p| &p.record.state),
        graph.as_ref().map(|g| g.progress.revision())
    )
}
async fn proposal(f: &Fixture, node: &str) -> (WorkflowClaim, cowork_storage::WorkflowModelTurn) {
    let c = claim(f, node).await;
    let turn = f
        .runtime
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            &f.lease,
            &c,
            &format!("proposal-{node}"),
            &CancellationToken::new(),
        )
        .await;
    if let Err(error) = &turn {
        panic!(
            "Proposal failed: {error:?}; {}",
            Box::pin(model_failure_context(f)).await
        );
    }
    (c, turn.unwrap())
}
async fn tool_exec(
    f: &Fixture,
    turn: &cowork_storage::WorkflowModelTurn,
) -> cowork_storage::WorkflowToolExecution {
    f.runtime
        .store
        .execute_workflow_artifact_tool(&f.project, &f.run.id, &f.lease, &turn.id, "call-1")
        .await
        .unwrap()
}
#[tokio::test]
async fn workflow_tool_approval_is_exact_and_replay_creates_one_artifact_and_one_charge() {
    let f = tool_graph(Mode::ToolOnce, AccessMode::ConfirmMutations, true).await;
    let (c, t) = proposal(&f, "a").await;
    let cowork_storage::WorkflowToolExecution::AwaitingApproval(a) = tool_exec(&f, &t).await else {
        panic!("approval")
    };
    let before = f.runtime.store.stored_run(&f.run.id).await.unwrap();
    let seed = canonical_hash(&before.checkpoint.messages);
    let rev = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap()
        .progress
        .revision();
    assert!(a.tool.id.len() == 64 && a.tool.id != "call-1");
    let cowork_storage::WorkflowToolExecution::AwaitingApproval(repeated) = tool_exec(&f, &t).await
    else {
        panic!("approval")
    };
    assert_eq!(a.id, repeated.id);
    assert_eq!(
        f.runtime.store.detail(&f.run.id).await.unwrap().run.steps,
        1
    );
    assert!(
        f.runtime
            .store
            .resolve_approval(
                &f.run.id,
                &a.id,
                ResolveApproval {
                    fingerprint: "wrong".into(),
                    allow: true
                }
            )
            .await
            .is_err()
    );
    let r = f
        .runtime
        .store
        .resolve_approval(
            &f.run.id,
            &a.id,
            ResolveApproval {
                fingerprint: a.fingerprint.clone(),
                allow: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(r.state, RunState::Running);
    assert_eq!(
        f.runtime
            .store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .lease
            .unwrap()
            .token,
        f.lease
    );
    let cowork_storage::WorkflowToolExecution::Completed(result) = tool_exec(&f, &t).await else {
        panic!("completed")
    };
    assert!(result.succeeded);
    assert_eq!(result.tool.id, "call-1");
    assert_eq!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .artifacts
            .len(),
        1
    );
    let wf = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert_eq!(wf.progress.revision(), rev + 1);
    assert_eq!(wf.progress.attempts().len(), 2);
    assert_eq!(
        canonical_hash(
            &f.runtime
                .store
                .stored_run(&f.run.id)
                .await
                .unwrap()
                .checkpoint
                .messages
        ),
        seed
    );
    assert!(matches!(
        f.runtime
            .store
            .workflow_command(
                &f.project,
                &f.run.id,
                &f.lease,
                wf.progress.revision(),
                WorkflowCommand::Settle {
                    claim: c.clone(),
                    id: result.id.clone(),
                    outcome: WorkflowOutcome::Succeeded
                }
            )
            .await,
        Err(Error::Forbidden)
    ));
    let replay = f
        .runtime
        .store
        .execute_workflow_artifact_tool(&f.project, &f.run.id, "old-lease", &t.id, "call-1")
        .await
        .unwrap();
    let cowork_storage::WorkflowToolExecution::Completed(replay) = replay else {
        panic!("read")
    };
    assert_eq!(replay, result);
    let next = f
        .runtime
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            &f.lease,
            &c,
            "final-a",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let msgs = &next.context.messages;
    assert!(
        msgs.iter()
            .any(|m| m["role"] == "tool" && m["tool_call_id"] == "call-1")
    );
    assert_eq!(next.response.unwrap().content, "answer-a");
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.path).await.unwrap();
    let cowork_storage::WorkflowToolExecution::Completed(replay) = reopened
        .execute_workflow_artifact_tool(&f.project, &f.run.id, "expired", &t.id, "call-1")
        .await
        .unwrap()
    else {
        panic!("replay")
    };
    assert_eq!(replay, result);
    assert_eq!(
        reopened
            .artifacts(&f.run.spec.order_id)
            .await
            .unwrap()
            .len(),
        1
    );
    reopened.close().await;
}
#[tokio::test]
async fn workflow_tool_read_only_rejects_unadvertised_writes_and_denied_approval_records_no_effect()
{
    for mode in [AccessMode::ReadOnly, AccessMode::ConfirmMutations] {
        let f = tool_graph(Mode::ToolOnce, mode.clone(), true).await;
        let (_, t) = proposal(&f, "a").await;
        if mode == AccessMode::ReadOnly {
            assert_eq!(
                t.response.as_ref().unwrap().rejection_code.as_deref(),
                Some("unselected_tool")
            );
            assert!(
                t.context
                    .tools
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|tool| tool["function"]["name"] != "create_artifact")
            );
            assert!(matches!(
                f.runtime
                    .store
                    .execute_workflow_artifact_tool(
                        &f.project, &f.run.id, &f.lease, &t.id, "call-1"
                    )
                    .await,
                Err(Error::Forbidden)
            ));
            let graph = f
                .runtime
                .store
                .stored_workflow(&f.project, &f.run.id)
                .await
                .unwrap();
            assert!(graph.agents["a"].turns[0].tool_results.is_empty());
            let parent = f.runtime.store.stored_run(&f.run.id).await.unwrap();
            assert!(parent.checkpoint.approvals.is_empty());
            assert!(
                f.runtime
                    .store
                    .artifacts(&f.run.spec.order_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(f.server.requests.lock().unwrap().len(), 1);
            f.runtime.store.close().await;
            continue;
        }
        if mode == AccessMode::ConfirmMutations {
            let cowork_storage::WorkflowToolExecution::AwaitingApproval(a) =
                tool_exec(&f, &t).await
            else {
                panic!("approval")
            };
            f.runtime
                .store
                .resolve_approval(
                    &f.run.id,
                    &a.id,
                    ResolveApproval {
                        fingerprint: a.fingerprint,
                        allow: false,
                    },
                )
                .await
                .unwrap();
        }
        let cowork_storage::WorkflowToolExecution::Completed(r) = tool_exec(&f, &t).await else {
            panic!("denied")
        };
        assert!(!r.succeeded);
        assert_eq!(r.result["error"], "policy_denied");
        assert!(
            f.runtime
                .store
                .artifacts(&f.run.spec.order_id)
                .await
                .unwrap()
                .is_empty()
        );
        f.runtime.store.close().await;
    }
}
#[tokio::test]
async fn workflow_tool_commit_failure_rolls_back_artifact_receipt_event_and_budget() {
    let f = tool_graph(Mode::ToolOnce, AccessMode::Autonomous, true).await;
    let (_, t) = proposal(&f, "a").await;
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&f.path))
            .await
            .unwrap();
    sqlx::query("CREATE TRIGGER reject_tool BEFORE INSERT ON events WHEN json_extract(NEW.payload,'$.kind')='workflow_tool_completed' BEGIN SELECT RAISE(ABORT,'controlled tool failure'); END").execute(&pool).await.unwrap();
    let before = canonical_hash(
        &f.runtime
            .store
            .stored_workflow(&f.project, &f.run.id)
            .await
            .unwrap(),
    );
    assert!(
        f.runtime
            .store
            .execute_workflow_artifact_tool(&f.project, &f.run.id, &f.lease, &t.id, "call-1")
            .await
            .is_err()
    );
    assert_eq!(
        canonical_hash(
            &f.runtime
                .store
                .stored_workflow(&f.project, &f.run.id)
                .await
                .unwrap()
        ),
        before
    );
    assert!(
        f.runtime
            .store
            .artifacts(&f.run.spec.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.runtime.store.detail(&f.run.id).await.unwrap().run.steps,
        1
    );
    sqlx::query("DROP TRIGGER reject_tool")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let cowork_storage::WorkflowToolExecution::Completed(r) = tool_exec(&f, &t).await else {
        panic!("completed")
    };
    assert!(r.succeeded);
    assert_eq!(
        f.runtime
            .store
            .artifacts(&f.run.spec.order_id)
            .await
            .unwrap()
            .len(),
        1
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn workflow_tool_namespaces_identical_provider_ids_from_parallel_agents() {
    let f = tool_graph(Mode::ToolOnce, AccessMode::ConfirmMutations, true).await;
    let (_, a) = proposal(&f, "a").await;
    let (_, b) = proposal(&f, "b").await;
    let cowork_storage::WorkflowToolExecution::AwaitingApproval(left) = tool_exec(&f, &a).await
    else {
        panic!("left")
    };
    let cowork_storage::WorkflowToolExecution::AwaitingApproval(right) = tool_exec(&f, &b).await
    else {
        panic!("right")
    };
    assert_ne!(left.tool.id, right.tool.id);
    assert_ne!(left.fingerprint, right.fingerprint);
    for a in [&left, &right] {
        f.runtime
            .store
            .resolve_approval(
                &f.run.id,
                &a.id,
                ResolveApproval {
                    fingerprint: a.fingerprint.clone(),
                    allow: true,
                },
            )
            .await
            .unwrap();
    }
    let (left, right) = tokio::join!(
        f.runtime
            .store
            .execute_workflow_artifact_tool(&f.project, &f.run.id, &f.lease, &a.id, "call-1"),
        f.runtime
            .store
            .execute_workflow_artifact_tool(&f.project, &f.run.id, &f.lease, &b.id, "call-1")
    );
    for (result, t) in [(left, &a), (right, &b)] {
        if matches!(result, Err(Error::Conflict)) {
            tool_exec(&f, t).await;
        } else {
            assert!(result.is_ok());
        }
    }
    let all = f
        .runtime
        .store
        .artifacts(&f.run.spec.order_id)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    let wf = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert_eq!(wf.agents["a"].turns[0].tool_results.len(), 1);
    assert_eq!(wf.agents["b"].turns[0].tool_results.len(), 1);
    f.runtime.store.close().await;
}
#[tokio::test]
async fn workflow_tool_revocation_between_approval_and_effect_blocks_creation() {
    let f = tool_graph(Mode::ToolOnce, AccessMode::ConfirmMutations, true).await;
    let (_, t) = proposal(&f, "a").await;
    let cowork_storage::WorkflowToolExecution::AwaitingApproval(a) = tool_exec(&f, &t).await else {
        panic!("approval")
    };
    f.runtime
        .store
        .resolve_approval(
            &f.run.id,
            &a.id,
            ResolveApproval {
                fingerprint: a.fingerprint,
                allow: true,
            },
        )
        .await
        .unwrap();
    let binding = f.run.spec.workflow.as_ref().unwrap();
    f.runtime
        .store
        .revoke_workflow(&f.project, &binding.workflow_id, 1, "revoke-before-effect")
        .await
        .unwrap();
    assert!(matches!(
        f.runtime
            .store
            .execute_workflow_artifact_tool(&f.project, &f.run.id, &f.lease, &t.id, "call-1")
            .await,
        Err(Error::Forbidden)
    ));
    assert!(
        f.runtime
            .store
            .artifacts(&f.run.spec.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.runtime.store.detail(&f.run.id).await.unwrap().run.steps,
        1
    );
    f.runtime.store.close().await;
}
#[tokio::test]
async fn native_coordinator_runs_artifact_roundtrips_with_raw_model_replies() {
    let f = tool_graph(Mode::ArtifactReads, AccessMode::Autonomous, false).await;
    let r = f
        .runtime
        .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        r.state,
        RunState::Completed,
        "{:?}",
        f.runtime
            .store
            .stored_workflow(&f.project, &f.run.id)
            .await
            .unwrap()
            .progress
            .nodes()
    );
    assert_eq!(r.steps, 21);
    assert_eq!(
        f.runtime
            .store
            .artifacts(&f.run.spec.order_id)
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 12);
    let wf = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    for agent in wf.agents.values() {
        assert_eq!(agent.turns.len(), 4);
        assert_eq!(
            agent
                .turns
                .iter()
                .map(|t| t.tool_results.len())
                .sum::<usize>(),
            3
        );
        assert!(
            agent.turns[2].tool_results[0].result["content"]
                .as_str()
                .unwrap()
                .starts_with("content-")
        );
    }
    f.runtime.store.close().await;
}
#[tokio::test]
async fn native_coordinator_approval_wait_keeps_parallel_models_and_parent_lease_running() {
    let f = tool_graph(Mode::ToolA, AccessMode::ConfirmMutations, false).await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let worker = tokio::spawn(async move {
        runtime
            .run_native_workflow(&project, &run, &CancellationToken::new())
            .await
    });
    let end = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let a = loop {
        let parent = f.runtime.store.stored_run(&f.run.id).await.unwrap();
        let graph = f
            .runtime
            .store
            .stored_workflow(&f.project, &f.run.id)
            .await
            .unwrap();
        if let Some(a) = parent.checkpoint.approvals.first()
            && matches!(
                graph.progress.nodes()["b"].state,
                WorkflowNodeState::Completed { .. }
            )
        {
            assert_eq!(parent.record.state, RunState::Running);
            assert!(parent.lease.is_some());
            break a.clone();
        }
        assert!(tokio::time::Instant::now() < end);
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    assert!(
        f.runtime
            .store
            .artifacts(&f.run.spec.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    f.runtime
        .store
        .resolve_approval(
            &f.run.id,
            &a.id,
            ResolveApproval {
                fingerprint: a.fingerprint,
                allow: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(worker.await.unwrap().unwrap().state, RunState::Completed);
    assert_eq!(
        f.runtime
            .store
            .artifacts(&f.run.spec.order_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 4);
    f.runtime.store.close().await;
}

#[tokio::test]
async fn native_workflow_repeated_raw_ids_in_later_turns_get_distinct_broker_receipts() {
    let f = tool_graph(Mode::ToolRepeat, AccessMode::Autonomous, false).await;
    let r = f
        .runtime
        .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.state, RunState::Completed);
    assert_eq!(r.steps, 15);
    assert_eq!(
        f.runtime
            .store
            .artifacts(&f.run.spec.order_id)
            .await
            .unwrap()
            .len(),
        6
    );
    let wf = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    for a in wf.agents.values() {
        assert_eq!(a.turns[0].tool_results[0].tool.id, "call-1");
        assert_eq!(a.turns[1].tool_results[0].tool.id, "call-1");
        assert_ne!(a.turns[0].tool_results[0].id, a.turns[1].tool_results[0].id);
        assert_eq!(
            a.turns[2]
                .context
                .messages
                .iter()
                .filter(|m| m["role"] == "tool" && m["tool_call_id"] == "call-1")
                .count(),
            2
        );
    }
    f.runtime.store.close().await;
}
#[tokio::test]
async fn workflow_known_tool_budget_failure_keeps_independent_pending_model_confirmable() {
    let f = graph_fixture(
        Mode::ToolBudgetA,
        true,
        60,
        GraphOptions {
            claimed: false,
            access: AccessMode::Autonomous,
            steps: 32,
            limit_a: true,
            ..Default::default()
        },
    )
    .await;
    let r = f
        .runtime
        .run_native_workflow(&f.project, &f.run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.state, RunState::Failed);
    let wf = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert!(
        matches!(&wf.progress.nodes()["a"].state,WorkflowNodeState::Failed { code } if code == "budget_exceeded")
    );
    assert!(matches!(
        wf.progress.nodes()["b"].state,
        WorkflowNodeState::Completed { .. }
    ));
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
    let detail = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(detail.token_budget.unconfirmed_tokens, 0);
    assert!(detail.artifacts.is_empty());
    assert_eq!(detail.run.steps, 2);
    f.runtime.store.close().await;
}
#[tokio::test]
async fn revoked_workflow_pending_approval_interrupts_promptly_without_artifact_or_retry() {
    let f = tool_graph(Mode::ToolA, AccessMode::ConfirmMutations, false).await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let worker = tokio::spawn(async move {
        runtime
            .run_native_workflow(&project, &run, &CancellationToken::new())
            .await
    });
    let end = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if !f
            .runtime
            .store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .checkpoint
            .approvals
            .is_empty()
        {
            break;
        }
        assert!(tokio::time::Instant::now() < end);
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let binding = f.run.spec.workflow.as_ref().unwrap();
    f.runtime
        .store
        .revoke_workflow(&f.project, &binding.workflow_id, 1, "revoke-pending")
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(
        f.runtime
            .store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .record
            .state,
        RunState::Interrupted
    );
    assert!(
        f.runtime
            .store
            .artifacts(&f.run.spec.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
    f.runtime.store.close().await;
}

#[tokio::test]
async fn workflow_confirm_all_requires_exact_read_and_write_approvals_without_pausing_siblings() {
    let f = tool_graph(Mode::ArtifactReads, AccessMode::ConfirmAll, false).await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let mut worker = tokio::spawn(async move {
        runtime
            .run_native_workflow(&project, &run, &CancellationToken::new())
            .await
    });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut resolved = std::collections::BTreeSet::new();
    loop {
        tokio::select! {
            r=&mut worker => {assert_eq!(r.unwrap().unwrap().state,RunState::Completed);break;},
            _=tokio::time::sleep_until(deadline) => {
                let parent=f.runtime.store.stored_run(&f.run.id).await.unwrap();
                let workflow=f.runtime.store.stored_workflow(&f.project,&f.run.id).await.unwrap();
                let nodes:Vec<_>=workflow.progress.nodes().iter().map(|(id,n)| (id,&n.state)).collect();
                let pending=parent.checkpoint.approvals.iter().filter(|a|a.decision.is_none()).count();
                let receipts:usize=workflow.agents.values().flat_map(|a|&a.turns).map(|t|t.tool_results.len()).sum();
                panic!("bounded fixture timeout: parent={:?}, revision={}, lease={}, graph_revision={}, nodes={:?}, pending_approvals={}, resolved={}, tool_receipts={}, http_requests={}",
                    parent.record.state,parent.record.revision,parent.lease.is_some(),workflow.progress.revision(),nodes,pending,resolved.len(),receipts,f.server.requests.lock().unwrap().len());
            },
            _=tokio::time::sleep(std::time::Duration::from_millis(25)) => {
                let parent=f.runtime.store.stored_run(&f.run.id).await.unwrap();
                for a in parent.checkpoint.approvals.iter().filter(|a| a.decision.is_none()) {
                    assert_eq!(parent.record.state,RunState::Running);
                    f.runtime.store.resolve_approval(&f.run.id,&a.id,ResolveApproval {fingerprint:a.fingerprint.clone(),allow:true}).await.unwrap();
                    assert!(resolved.insert(a.id.clone()));
                }
            }
        }
    }
    assert_eq!(resolved.len(), 9);
    assert_eq!(
        f.runtime
            .store
            .artifacts(&f.run.spec.order_id)
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 12);
    f.runtime.store.close().await;
}

#[tokio::test]
async fn workflow_artifact_reads_reject_payload_scope_even_when_content_hash_is_unchanged() {
    let f = tool_graph(Mode::ToolOnce, AccessMode::Autonomous, true).await;
    let (_, t) = proposal(&f, "a").await;
    tool_exec(&f, &t).await;
    let artifacts = f
        .runtime
        .store
        .artifacts(&f.run.spec.order_id)
        .await
        .unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&f.path))
            .await
            .unwrap();
    let mut altered = serde_json::to_value(&artifacts[0]).unwrap();
    altered["order_id"] = json!("unrelated-order");
    sqlx::query("UPDATE artifacts SET payload=$1 WHERE id=$2")
        .bind(altered.to_string())
        .bind(&artifacts[0].id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        f.runtime.store.artifacts(&f.run.spec.order_id).await,
        Err(Error::Forbidden)
    ));
    pool.close().await;
    f.runtime.store.close().await;
}

#[tokio::test]
async fn workflow_approval_wait_expires_at_node_deadline_and_preserves_independent_answer() {
    let f = graph_fixture(
        Mode::ToolBudgetA,
        true,
        60,
        GraphOptions {
            claimed: false,
            access: AccessMode::ConfirmMutations,
            steps: 32,
            a_seconds: Some(3),
            ..Default::default()
        },
    )
    .await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let worker = tokio::spawn(async move {
        runtime
            .run_native_workflow(&project, &run, &CancellationToken::new())
            .await
    });
    let end = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
    loop {
        let graph = f
            .runtime
            .store
            .stored_workflow(&f.project, &f.run.id)
            .await
            .unwrap();
        if matches!(&graph.progress.nodes()["a"].state,WorkflowNodeState::Failed {code} if code=="budget_exceeded")
        {
            assert!(matches!(
                graph.progress.nodes()["b"].state,
                WorkflowNodeState::Running { .. }
            ));
            break;
        }
        assert!(tokio::time::Instant::now() < end);
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(worker.await.unwrap().unwrap().state, RunState::Failed);
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert!(matches!(
        graph.progress.nodes()["b"].state,
        WorkflowNodeState::Completed { .. }
    ));
    let detail = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert!(detail.artifacts.is_empty());
    assert_eq!(detail.token_budget.unconfirmed_tokens, 0);
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
    f.runtime.store.close().await;
}
#[tokio::test]
async fn workflow_stop_prevents_confirmed_sibling_tool_without_unknown_parent_interruption() {
    let f = graph_fixture(
        Mode::ToolStop,
        true,
        60,
        GraphOptions {
            claimed: false,
            access: AccessMode::ConfirmMutations,
            steps: 32,
            limit_a: true,
            failure: WorkflowFailure::Stop,
            ..Default::default()
        },
    )
    .await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let mut worker = tokio::spawn(async move {
        runtime
            .run_native_workflow(&project, &run, &CancellationToken::new())
            .await
    });
    // A must reach its known step limit after exact approval, while B remains in IO.
    loop {
        tokio::select! {
            r=&mut worker => {assert_eq!(r.unwrap().unwrap().state,RunState::Failed);break;},
            _=tokio::time::sleep(std::time::Duration::from_millis(25)) => {
                let p=f.runtime.store.stored_run(&f.run.id).await.unwrap();
                for a in p.checkpoint.approvals.iter().filter(|a| a.decision.is_none()) {
                    f.runtime.store.resolve_approval(&f.run.id,&a.id,ResolveApproval {fingerprint:a.fingerprint.clone(),allow:true}).await.unwrap();
                }
            }
        }
    }
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert!(
        matches!(&graph.progress.nodes()["a"].state,WorkflowNodeState::Failed {code} if code=="budget_exceeded")
    );
    assert!(
        matches!(&graph.progress.nodes()["b"].state,WorkflowNodeState::Failed {code} if code=="workflow_stopped")
    );
    let p = f.runtime.store.stored_run(&f.run.id).await.unwrap();
    assert_eq!(p.checkpoint.approvals.len(), 1);
    let detail = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert!(detail.artifacts.is_empty());
    assert_eq!(detail.token_budget.unconfirmed_tokens, 0);
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
    f.runtime.store.close().await;
}

async fn prepare_input_answer(
    f: &Fixture,
    q: &cowork_workflow_contracts::WorkflowInputView,
    text: &str,
) -> cowork_workflow_contracts::WorkflowInputAnswerIntent {
    use cowork_workflow_contracts::*;
    let head = f
        .runtime
        .store
        .workflow_input_draft(&f.run.id, &q.request_id)
        .await
        .unwrap()
        .draft;
    let saved = f
        .runtime
        .store
        .save_workflow_input_draft(
            &f.run.id,
            &q.request_id,
            &SaveWorkflowInputDraft {
                fingerprint: q.fingerprint.clone(),
                base_revision: head.revision,
                idempotency_key: cowork_storage::id(),
                answer: text.into(),
            },
        )
        .await
        .unwrap();
    let WorkflowInputDraftSave::Saved { draft, .. } = saved else {
        panic!("own fresh input draft")
    };
    f.runtime
        .store
        .prepare_workflow_input_answer(
            &f.run.id,
            &q.request_id,
            &PrepareWorkflowInputAnswer {
                fingerprint: q.fingerprint.clone(),
                base_draft_revision: draft.revision,
                idempotency_key: cowork_storage::id(),
            },
        )
        .await
        .unwrap()
}
async fn commit_input_answer(
    f: &Fixture,
    q: &cowork_workflow_contracts::WorkflowInputView,
    text: &str,
) -> cowork_workflow_contracts::WorkflowInputAnswerIntent {
    let frozen = prepare_input_answer(f, q, text).await;
    let committed = f
        .runtime
        .store
        .commit_workflow_input_answer_intent(&f.run.id, &frozen.id)
        .await
        .unwrap();
    assert_eq!(
        committed.state,
        cowork_workflow_contracts::WorkflowInputAnswerIntentState::Accepted
    );
    committed
}
fn provider_node(body: &Value) -> String {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["content"].as_str())
        .filter_map(|s| serde_json::from_str::<Value>(s).ok())
        .find_map(|m| m["workflow_step"].as_str().map(str::to_owned))
        .unwrap()
}
async fn two_input_questions(f: &Fixture) -> Vec<cowork_workflow_contracts::WorkflowInputView> {
    let mut diagnostic = String::from("no public snapshot yet");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let view = f
                .runtime
                .store
                .workflow_run_view(&f.project, &f.run.id)
                .await
                .unwrap();
            diagnostic = format!(
                "parent={:?}, now={}, nodes={:?}, inputs={:?}, provider_requests={}",
                view.run.state,
                cowork_storage::now(),
                view.nodes
                    .iter()
                    .map(|n| (&n.id, &n.state))
                    .collect::<Vec<_>>(),
                view.inputs
                    .iter()
                    .map(|q| (&q.node_id, &q.state, q.expires_at, &q.closed_reason))
                    .collect::<Vec<_>>(),
                f.server.requests.lock().unwrap().len()
            );
            if view.inputs.iter().filter(|q| q.actionable).count() == 2 {
                return view.inputs;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    result.unwrap_or_else(|_| panic!("both agents must open their questions: {diagnostic}"))
}
#[tokio::test]
async fn native_parallel_input_waits_keep_parent_lease_and_send_exact_isolated_answers() {
    let f = queued(Mode::InputParallel, true).await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let stop = CancellationToken::new();
    let owned = stop.clone();
    let task =
        tokio::spawn(async move { runtime.run_native_workflow(&project, &run, &owned).await });
    let questions = two_input_questions(&f).await;
    let a = questions.iter().find(|q| q.node_id == "a").unwrap();
    let b = questions.iter().find(|q| q.node_id == "b").unwrap();
    assert_ne!(a.request_id, b.request_id);
    let before = f.runtime.store.stored_run(&f.run.id).await.unwrap();
    assert_eq!(before.record.state, RunState::Running);
    let old = before.lease.clone().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(3300)).await;
    let waiting = f.runtime.store.stored_run(&f.run.id).await.unwrap();
    assert_eq!(waiting.record.state, RunState::Running);
    assert_eq!(waiting.lease.as_ref().unwrap().token, old.token);
    assert!(waiting.lease.as_ref().unwrap().until > old.until);
    assert_eq!(
        f.server.requests.lock().unwrap().len(),
        2,
        "waiting cannot restart providers"
    );
    let answer_a = "  Antwort A: \"unver\u{e4}ndert\" \\\r\n\u{d6}\u{1f600}\n ";
    let answer_b = "Exact B\nwith independent content";
    let (a_receipt, b_receipt) = tokio::join!(
        commit_input_answer(&f, a, answer_a),
        commit_input_answer(&f, b, answer_b)
    );
    assert_ne!(
        a_receipt.receipt.unwrap().confirmed_sequence,
        b_receipt.receipt.unwrap().confirmed_sequence
    );
    let completed = tokio::time::timeout(std::time::Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(completed.state, RunState::Completed);
    {
        let requests = f.server.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            5,
            "one first request per node and one continuation per answered agent"
        );
        for (node, own, other) in [("a", answer_a, answer_b), ("b", answer_b, answer_a)] {
            let continuation = requests
                .iter()
                .find(|body| {
                    provider_node(body) == node
                        && body["messages"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|m| m["role"] == "tool")
                })
                .unwrap();
            let messages = continuation["messages"].as_array().unwrap();
            assert_eq!(
                messages.iter().find(|m| m["role"] == "tool").unwrap()["content"],
                own
            );
            assert!(!messages.iter().any(|m| m["content"] == other));
        }
    }
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert_eq!(graph.inputs.len(), 2);
    assert_eq!(graph.progress.attempts().len(), 7);
    assert!(graph.inputs.values().all(|q| q.receipt.is_some()));
    f.runtime.store.close().await;
}
#[tokio::test]
async fn native_input_node_deadline_closes_known_wait_without_interrupting_parallel_question() {
    use cowork_workflow_contracts::WorkflowInputState;
    let f = graph_fixture(
        Mode::InputParallel,
        true,
        180,
        GraphOptions {
            claimed: false,
            a_seconds: Some(10),
            ..Default::default()
        },
    )
    .await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let task = tokio::spawn(async move {
        runtime
            .run_native_workflow(&project, &run, &CancellationToken::new())
            .await
    });
    let questions = two_input_questions(&f).await;
    let qb = questions.iter().find(|q| q.node_id == "b").unwrap().clone();
    let original = f.runtime.store.stored_run(&f.run.id).await.unwrap();
    let checkpoint = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    let qa = questions.iter().find(|q| q.node_id == "a").unwrap();
    let original_node_deadline =
        checkpoint.progress.nodes()["a"].started_at.unwrap() * 1000 + 10_000;
    let original_parent_deadline = original.record.started_at.unwrap() + 180_000;
    assert_eq!(
        qa.expires_at,
        original_node_deadline.min(original_parent_deadline)
    );
    assert!(qa.actionable && cowork_storage::now() < qa.expires_at);
    let view = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let view = f
                .runtime
                .store
                .workflow_run_view(&f.project, &f.run.id)
                .await
                .unwrap();
            if view
                .inputs
                .iter()
                .any(|q| q.node_id == "a" && q.state == WorkflowInputState::Closed)
            {
                return view;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("node A must close its known wait");
    assert_eq!(view.run.state, RunState::Running);
    let qa = view.inputs.iter().find(|q| q.node_id == "a").unwrap();
    assert_eq!(qa.closed_reason.as_deref(), Some("budget_exceeded"));
    assert!(view.inputs.iter().any(|q| q.node_id == "b" && q.actionable));
    assert_eq!(
        f.server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| provider_node(r) == "a")
            .count(),
        1
    );
    commit_input_answer(&f, &qb, "B can still continue").await;
    let completed = tokio::time::timeout(std::time::Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(completed.state, RunState::Failed);
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert!(
        matches!(&graph.progress.nodes()["a"].state,WorkflowNodeState::Failed{code} if code=="budget_exceeded")
    );
    assert!(matches!(
        &graph.progress.nodes()["b"].state,
        WorkflowNodeState::Completed { .. }
    ));
    assert_eq!(
        f.server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| provider_node(r) == "a")
            .count(),
        1
    );
    assert_eq!(
        f.runtime
            .store
            .events(&f.run.id, 0)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.kind == "workflow_input_closed")
            .count(),
        1
    );
    f.runtime.store.close().await;
}

#[tokio::test]
async fn native_human_wait_cancel_cleanup_wins_before_answer_commit() {
    use cowork_workflow_contracts::{WorkflowInputAnswerIntentState, WorkflowInputState};
    let f = queued(Mode::InputParallel, true).await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let task = tokio::spawn(async move {
        runtime
            .run_native_workflow(&project, &run, &CancellationToken::new())
            .await
    });
    let questions = two_input_questions(&f).await;
    let q = questions.iter().find(|q| q.node_id == "a").unwrap();
    let frozen =
        prepare_input_answer(&f, q, "Exact canceled draft\r\nUnicode: \u{00d6}\u{1f600} ").await;
    let before = f.runtime.store.stored_run(&f.run.id).await.unwrap();
    let old_lease = before.lease.as_ref().unwrap().token.clone();
    assert_eq!(
        f.runtime.store.cancel(&f.run.id).await.unwrap().state,
        RunState::CancelRequested
    );
    // Force the coordinator cleanup to win. The answer is committed only after
    // the actual native task has exited, reproducing the reported process race.
    let stopped = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(stopped.state, RunState::Canceled);
    let after = f.runtime.store.stored_run(&f.run.id).await.unwrap();
    assert!(after.lease.is_none());
    assert_eq!(after.record.steps, before.record.steps);
    assert!(matches!(
        f.runtime.store.heartbeat(&f.run.id, &old_lease).await,
        Err(Error::StaleLease)
    ));
    let rejected = f
        .runtime
        .store
        .commit_workflow_input_answer_intent(&f.run.id, &frozen.id)
        .await
        .unwrap();
    assert_eq!(rejected.state, WorkflowInputAnswerIntentState::Rejected);
    assert!(rejected.receipt.is_none());
    assert_eq!(
        rejected.submission.input.answer,
        frozen.submission.input.answer
    );
    let repeated = f
        .runtime
        .store
        .commit_workflow_input_answer_intent(&f.run.id, &frozen.id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&repeated), canonical_hash(&rejected));
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert!(graph.inputs.values().all(|input| input.receipt.is_none()));
    let view = f
        .runtime
        .store
        .workflow_run_view(&f.project, &f.run.id)
        .await
        .unwrap();
    assert!(
        view.inputs
            .iter()
            .all(|input| input.state == WorkflowInputState::Closed
                && !input.actionable
                && input.closed_reason.as_deref() == Some("canceled"))
    );
    let events = f.runtime.store.events(&f.run.id, 0).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "cancel_requested")
            .count(),
        1
    );
    assert_eq!(
        events.iter().filter(|e| e.kind == "run_canceled").count(),
        1
    );
    assert!(!events.iter().any(|e| matches!(
        e.kind.as_str(),
        "workflow_input_answered"
            | "workflow_coordinator_interrupted"
            | "workflow_cancel_interrupted"
    )));
    assert!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .artifacts
            .is_empty()
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
    assert!(f.runtime.store.resume(&f.run.id).await.is_err());
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.path).await.unwrap();
    assert_eq!(
        reopened.stored_run(&f.run.id).await.unwrap().record.state,
        RunState::Canceled
    );
    assert_eq!(
        canonical_hash(
            &reopened
                .workflow_input_answer_intent(&f.run.id, &frozen.id)
                .await
                .unwrap()
        ),
        canonical_hash(&rejected)
    );
    reopened.close().await;
}
#[tokio::test]
async fn native_cancel_retains_unknown_parallel_model_and_rejects_saved_human_answer() {
    use cowork_workflow_contracts::WorkflowInputAnswerIntentState;
    let f = queued(Mode::InputWithHeldB, true).await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let task = tokio::spawn(async move {
        runtime
            .run_native_workflow(&project, &run, &CancellationToken::new())
            .await
    });
    wait_requests(&f, 2).await;
    let q = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let view = f
                .runtime
                .store
                .workflow_run_view(&f.project, &f.run.id)
                .await
                .unwrap();
            if let Some(input) = view
                .inputs
                .into_iter()
                .find(|q| q.node_id == "a" && q.actionable)
            {
                return input;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let frozen = prepare_input_answer(&f, &q, "Saved answer must remain unexecuted").await;
    let before = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(before.token_budget.unconfirmed_tokens, 16_384);
    assert_eq!(
        f.runtime.store.cancel(&f.run.id).await.unwrap().state,
        RunState::CancelRequested
    );
    let stopped = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(stopped.state, RunState::Interrupted);
    assert_eq!(
        stopped.error_code.as_deref(),
        Some("canceled_with_uncertain_effect")
    );
    let after = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(
        canonical_hash(&after.token_budget),
        canonical_hash(&before.token_budget)
    );
    assert_eq!(after.run.steps, before.run.steps);
    assert!(
        after
            .run
            .model_attempts
            .iter()
            .any(|a| a.state == ModelAttemptState::Interrupted
                && a.usage.input_tokens.is_none()
                && a.reserved_tokens == 16_384)
    );
    assert!(
        f.runtime
            .store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .lease
            .is_none()
    );
    let rejected = f
        .runtime
        .store
        .commit_workflow_input_answer_intent(&f.run.id, &frozen.id)
        .await
        .unwrap();
    assert_eq!(rejected.state, WorkflowInputAnswerIntentState::Rejected);
    assert!(rejected.receipt.is_none());
    assert_eq!(
        rejected.submission.input.answer,
        frozen.submission.input.answer
    );
    assert!(
        f.runtime
            .store
            .detail(&f.run.id)
            .await
            .unwrap()
            .artifacts
            .is_empty()
    );
    let events = f.runtime.store.events(&f.run.id, 0).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "cancel_requested")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "workflow_cancel_interrupted")
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind.as_str(), "run_canceled" | "workflow_input_answered"))
    );
    assert!(f.runtime.store.resume(&f.run.id).await.is_err());
    f.server.parallel.wait().await;
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.path).await.unwrap();
    assert_eq!(
        canonical_hash(&reopened.detail(&f.run.id).await.unwrap().token_budget),
        canonical_hash(&before.token_budget)
    );
    assert_eq!(
        reopened
            .workflow_input_answer_intent(&f.run.id, &frozen.id)
            .await
            .unwrap()
            .state,
        WorkflowInputAnswerIntentState::Rejected
    );
    let runtime = Runtime::new(reopened, vec![], false).unwrap();
    let stop = CancellationToken::new();
    let worker = serve(&runtime, &stop);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
    stop_worker(&stop, worker).await;
    runtime.store.close().await;
}

#[path = "workflow_models/clock_cases.rs"]
mod clock_cases;

async fn bind_private_workflow_money(f: &Fixture) {
    use cowork_storage::model_money::*;
    let provider = f
        .runtime
        .store
        .workflow_node_spec(&f.project, &f.run.id, "a")
        .await
        .unwrap()
        .provider
        .unwrap();
    let currency = Currency {
        code: "EUR".into(),
        scale: 6,
    };
    f.runtime
        .store
        .register_personal_model_deployment(
            PoolPolicy {
                scope: PoolScope::Installation("workflow-fixture-installation".into()),
                version: 1,
                currency: currency.clone(),
                limit_units: 100_000,
            },
            OperatorModelDeployment {
                id: "workflow-fixture-deployment".into(),
                provider: provider.clone(),
                model_sha256: "c".repeat(64),
                price_id: "workflow-fixture-price".into(),
                price_version: 1,
                token_mode: MonetaryTokenMode::ConservativeContextUpperBound,
            },
            PriceVersion {
                id: "workflow-fixture-price".into(),
                version: 1,
                provider_id: provider.id,
                model_sha256: "c".repeat(64),
                currency: currency.clone(),
                input_units_per_million: 1_000_000,
                output_units_per_million: 1_000_000,
                cached_input_units_per_million: None,
                source_sha256: "d".repeat(64),
            },
        )
        .await
        .unwrap();
    f.runtime
        .store
        .bind_personal_run_money(
            &f.run.id,
            "workflow-fixture-deployment",
            PoolPolicy {
                scope: PoolScope::Project(f.project.clone()),
                version: 1,
                currency: currency.clone(),
                limit_units: 100_000,
            },
            PoolPolicy {
                scope: PoolScope::Run(f.run.id.clone()),
                version: 1,
                currency,
                limit_units: 100_000,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn monetary_actual_workflow_receipt_is_bound_and_missing_cache_holds_through_replay() {
    use cowork_storage::model_money::*;
    let mut f = fixture(Mode::Text, false, 60).await;
    bind_private_workflow_money(&f).await;
    let a = claim(&f, "a").await;
    let turn = f
        .runtime
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            &f.lease,
            &a,
            "workflow-money",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let before = f.runtime.store.monetary_ledger().await.unwrap();
    let reservation = before
        .reservation(&monetary_attempt_id(&f.run.id, &turn.id))
        .unwrap();
    assert!(
        matches!(reservation.state, ReservationState::Uncertain { .. }),
        "fixture omitted cache details; no cache=0 guess even without distinct cache pricing"
    );
    assert_eq!(reservation.charged_units().unwrap(), 16384 + 256);
    let events = f.runtime.store.events(&f.run.id, 0).await.unwrap();
    let received = &events
        .iter()
        .find(|e| e.kind == "monetary_model_response")
        .unwrap()
        .payload["received"];
    assert_eq!(received["dispatch"]["run_id"], f.run.id);
    assert_eq!(received["dispatch"]["attempt_id"], "workflow-money");
    assert!(
        received["dispatch"]["admission_receipt_sha256"]
            .as_str()
            .is_some_and(|v| v.len() == 64)
    );
    let replay = f
        .runtime
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            "historical-lease",
            &a,
            "workflow-money",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(canonical_hash(&replay), canonical_hash(&turn));
    assert_eq!(f.server.requests.lock().unwrap().len(), 1);
    assert_eq!(f.runtime.store.monetary_ledger().await.unwrap(), before);
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.path).await.unwrap();
    assert_eq!(reopened.monetary_ledger().await.unwrap(), before);
    reopened.close().await;
    f.handle.abort();
    assert!(matches!((&mut f.handle).await, Err(error) if error.is_cancelled()));
}

#[tokio::test]
async fn monetary_workflow_node_deadline_after_writer_wait_rolls_back_global_hold() {
    let mut f = graph_fixture(
        Mode::Text,
        false,
        60,
        GraphOptions {
            a_seconds: Some(5),
            ..Default::default()
        },
    )
    .await;
    bind_private_workflow_money(&f).await;
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&f.path))
            .await
            .unwrap();
    // Connection setup must precede the claim that starts the node's clock.
    let a = claim(&f, "a").await;
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    let node = &graph.progress.nodes()["a"];
    let node_deadline_ms =
        (node.started_at.unwrap() + i64::try_from(node.plan.budgets.max_seconds).unwrap()) * 1000;
    let (expected, context) = f
        .runtime
        .store
        .workflow_model_context(
            &f.project,
            &f.run.id,
            &a,
            cowork_runtime::provider::tool_catalog(
                &f.runtime
                    .store
                    .workflow_node_spec(&f.project, &f.run.id, "a")
                    .await
                    .unwrap(),
            ),
        )
        .await
        .unwrap();
    let prepared =
        cowork_runtime::provider::prepare(&context.spec, &context.messages, false).unwrap();
    let wire = prepared.wire_sha256().to_owned();
    let lease_until = f
        .runtime
        .store
        .stored_run(&f.run.id)
        .await
        .unwrap()
        .lease
        .unwrap()
        .until;
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("UPDATE accounting_ledgers SET revision=revision")
        .execute(&mut *writer)
        .await
        .unwrap();
    let store = f.runtime.store.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let lease = f.lease.clone();
    // SQLite Store has one connection; WAL readers remain available while
    // this separate connection holds the global writer.
    tokio::time::timeout(
        std::time::Duration::from_millis(500),
        f.runtime.store.stored_run(&f.run.id),
    )
    .await
    .expect("fixture writer must leave independent WAL reads available")
    .unwrap();
    let mut waiting = tokio::spawn(async move {
        store
            .begin_prepared_workflow_model(
                &project,
                &run,
                &lease,
                expected,
                &a,
                "workflow-budget-wait",
                &context,
                &wire,
            )
            .await
    });
    // Observe admission holding the Store's sole connection, rather than
    // mistaking task startup or pre-admission work for a writer wait.
    loop {
        assert!(
            cowork_storage::now() < node_deadline_ms,
            "fixture must reach writer before node deadline"
        );
        assert!(
            !waiting.is_finished(),
            "admission must reach the writer before its deadline"
        );
        if tokio::time::timeout(
            std::time::Duration::from_millis(100),
            f.runtime.store.stored_run(&f.run.id),
        )
        .await
        .is_err()
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    let remaining_ms = node_deadline_ms - cowork_storage::now();
    assert!(
        remaining_ms > 0,
        "writer wait must be observed before node expiry"
    );
    let mut queued_reader = Box::pin(f.runtime.store.stored_run(&f.run.id));
    tokio::select! {
        _ = &mut waiting => panic!("admission returned while global writer was held"),
        _ = &mut queued_reader => panic!("admission did not retain the Store connection during writer wait"),
        _ = tokio::time::sleep(std::time::Duration::from_millis(u64::try_from(remaining_ms).unwrap() + 100)) => {}
    }
    assert!(cowork_storage::now() >= node_deadline_ms);
    assert!(
        !waiting.is_finished(),
        "actual admission waited on global writer"
    );
    assert!(lease_until > cowork_storage::now());
    writer.rollback().await.unwrap();
    assert!(matches!(waiting.await.unwrap(), Err(Error::BudgetExceeded)));
    queued_reader.await.unwrap();
    assert!(
        f.runtime
            .store
            .monetary_ledger()
            .await
            .unwrap()
            .reservations()
            .is_empty()
    );
    assert!(
        f.runtime
            .store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .record
            .model_attempts
            .is_empty()
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 0);
    pool.close().await;
    f.runtime.store.close().await;
    f.handle.abort();
    assert!(matches!((&mut f.handle).await, Err(error) if error.is_cancelled()));
}
