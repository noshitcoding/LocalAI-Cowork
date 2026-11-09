#[path = "../../storage/tests/support/native_keys.rs"]
mod native_keys;
#[path = "../../storage/tests/support/mcp.rs"]
mod remote;
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use cowork_contracts::*;
use cowork_domain::{Error, mcp::mcp_alias};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::{Store, id};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Model {
    alias: String,
    requests: Arc<Mutex<Vec<Value>>>,
}
async fn completion(State(model): State<Model>, Json(body): Json<Value>) -> String {
    let finished = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool");
    model.requests.lock().unwrap().push(body);
    let delta = if finished {
        json!({"content":"Recorded external result."})
    } else {
        json!({"tool_calls":[{"index":0,"id":"external-action","function":{"name":model.alias,"arguments":"{\"text\":\"exact product input\"}"}}]})
    };
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"delta":delta,"finish_reason":if finished{"stop"}else{"tool_calls"}}],"usage":{"prompt_tokens":100,"completion_tokens":20}})
    )
}
struct Fixture {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    runtime: Runtime,
    input: CreateRun,
    remote: remote::Remote,
    model: Model,
    task: tokio::task::JoinHandle<()>,
    keys: Arc<native_keys::FakeProvider>,
    server: McpServer,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn fixture(mode: AccessMode, tool_error: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let p = store
        .create_project(CreateProject {
            name: "MCP product fixture".into(),
        })
        .await
        .unwrap();
    let o = store
        .create_order(CreateOrder {
            project_id: p.id.clone(),
            title: "Complete agent path".into(),
        })
        .await
        .unwrap();
    let remote = remote::Remote::new(dir.path().join("effects"), Duration::ZERO, tool_error).await;
    let server = store
        .configure_mcp_server(
            &p.id,
            &id(),
            0,
            "configure",
            &McpServerConfiguration {
                name: "Synthetic remote".into(),
                endpoint: remote.endpoint.clone(),
                credential_reference: None,
                credential_revision: 0,
                catalog: vec![remote::definition()],
                selected_tools: vec!["append_note".into()],
                enabled: true,
            },
        )
        .await
        .unwrap()
        .public_view();
    let model = Model {
        alias: mcp_alias(&server.id, "append_note"),
        requests: Arc::new(Mutex::new(vec![])),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route(
            "/models",
            get(|| async { Json(json!({"data":[{"id":"fixture"}]})) }),
        )
        .route("/chat/completions", post(completion))
        .with_state(model.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let keys = Arc::new(native_keys::FakeProvider::default());
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile: ProviderProfile {
                generation: Default::default(),
                id: "fixture".into(),
                name: "Fixture".into(),
                kind: ProviderKind::OpenaiCompatible,
                endpoint,
                model: "fixture".into(),
                revision: 1,
                context_tokens: 32768,
                credential_required: false,
                credential_ready: true,
                capabilities: Default::default(),
            },
            credential_env: None,
        }],
        false,
    )
    .unwrap()
    .with_native_key_provider(keys.clone());
    runtime
        .setup_native_secrets(&NativeSecretsSetup {
            idempotency_key: "setup".into(),
            base_revision: 0,
        })
        .await
        .unwrap();
    let input = CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        environment: None,
        mcp_tools: vec![McpToolSelection {
            server_id: server.id.clone(),
            server_revision: server.revision,
            tool_name: "append_note".into(),
        }],
        budgets: Budgets::default(),
        order_id: o.id,
        base_revision: o.revision,
        idempotency_key: id(),
        prompt: "Use my selected tool".into(),
        provider_id: "fixture".into(),
        provider_revision: 1,
        access_mode: mode,
        folders: vec![],
    };
    Fixture {
        _dir: dir,
        path,
        runtime,
        input,
        remote,
        model,
        task,
        keys,
        server,
    }
}
async fn wait(runtime: &Runtime, run: &str, state: RunState) -> RunDetail {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let detail = runtime.store.detail(run).await.unwrap();
            if detail.run.state == state {
                break detail;
            }
            assert!(
                !matches!(
                    detail.run.state,
                    RunState::Failed | RunState::Expired | RunState::Canceled
                ),
                "unexpected {:?}: {:?}",
                detail.run.state,
                detail.run.error_code
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("run state deadline")
}
fn start(runtime: &Runtime) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let stop = CancellationToken::new();
    let task = tokio::spawn(runtime.clone().serve(2, stop.clone()));
    (stop, task)
}

#[tokio::test]
async fn explicit_selection_default_approval_and_restart_use_the_model_loop() {
    let mut f = fixture(AccessMode::ConfirmMutations, false).await;
    let flight = f.runtime.preflight(&f.input).await.unwrap();
    assert_eq!(flight.mcp_tools.len(), 1);
    let run = f.runtime.create_run(&f.input).await.unwrap();
    let (stop, worker) = start(&f.runtime);
    let waiting = wait(&f.runtime, &run.id, RunState::WaitingApproval).await;
    assert_eq!(f.remote.calls(), 0);
    assert_eq!(
        f.remote
            .state
            .initializations
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    let approval = &waiting.approvals[0];
    assert_eq!(approval.tool.name, f.model.alias);
    stop.cancel();
    worker.await.unwrap();
    let providers = f.runtime.providers.read().await.clone();
    f.runtime.store.close().await;
    f.runtime = Runtime::new(Store::sqlite(&f.path).await.unwrap(), providers, false)
        .unwrap()
        .with_native_key_provider(f.keys.clone());
    f.runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                allow: true,
                fingerprint: approval.fingerprint.clone(),
            },
        )
        .await
        .unwrap();
    let (stop, worker) = start(&f.runtime);
    let completed = wait(&f.runtime, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(f.remote.calls(), 1);
    assert_eq!(f.runtime.create_run(&f.input).await.unwrap().id, run.id);
    assert_eq!(completed.run.spec.mcp_tools, flight.mcp_tools);
    let attempts = f.runtime.store.mcp_attempts(&run.id).await.unwrap();
    assert_eq!(attempts.len(), 1);
    assert!(matches!(
        attempts[0].state,
        McpAttemptState::Completed { .. }
    ));
    {
        let requests = f.model.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let tools = requests[0]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["function"]["name"] == f.model.alias
            && t["function"]["parameters"] == remote::definition()["inputSchema"]));
        let messages = requests[1]["messages"].as_array().unwrap();
        assert_eq!(messages.iter().filter(|m| m["role"] == "tool").count(), 1);
    }
    let messages = f.runtime.store.messages(&f.input.order_id).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.role == "assistant" && m.content == "Recorded external result.")
            .count(),
        1
    );
    assert!(completed.artifacts.is_empty()); // Remote structured output stays in its verified journal receipt.
}

#[tokio::test]
async fn read_only_denies_even_a_server_readonly_hint_without_connecting() {
    let f = fixture(AccessMode::ReadOnly, false).await;
    let run = f.runtime.create_run(&f.input).await.unwrap();
    let (stop, worker) = start(&f.runtime);
    wait(&f.runtime, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(f.remote.calls(), 0);
    assert_eq!(
        f.remote
            .state
            .initializations
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(
        f.runtime
            .store
            .mcp_attempts(&run.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn uncertain_remote_mutation_is_visible_and_never_repeated() {
    let f = fixture(AccessMode::Autonomous, true).await;
    let run = f.runtime.create_run(&f.input).await.unwrap();
    let (stop, worker) = start(&f.runtime);
    wait(&f.runtime, &run.id, RunState::Interrupted).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(f.remote.calls(), 1);
    assert_eq!(
        f.remote
            .state
            .durable_effects
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert!(matches!(
        f.runtime.store.mcp_attempts(&run.id).await.unwrap()[0].state,
        McpAttemptState::OutcomeUncertain { .. }
    ));
    assert!(f.runtime.store.resume(&run.id).await.is_err());
    assert_eq!(f.runtime.create_run(&f.input).await.unwrap().id, run.id);
    let (stop, worker) = start(&f.runtime);
    tokio::time::sleep(Duration::from_millis(250)).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(f.remote.calls(), 1);
}

#[tokio::test]
async fn selection_and_acceptance_reject_stale_foreign_or_duplicate_references() {
    let f = fixture(AccessMode::Autonomous, false).await;
    let mut input = f.input.clone();
    input.mcp_tools.push(input.mcp_tools[0].clone());
    assert!(matches!(
        f.runtime.preflight(&input).await,
        Err(Error::Conflict)
    ));
    input = f.input.clone();
    input.mcp_tools[0].tool_name = "other".into();
    assert!(matches!(
        f.runtime.preflight(&input).await,
        Err(Error::Forbidden)
    ));
    let p = f
        .runtime
        .store
        .create_project(CreateProject {
            name: "Other project".into(),
        })
        .await
        .unwrap();
    let o = f
        .runtime
        .store
        .create_order(CreateOrder {
            project_id: p.id,
            title: "Foreign".into(),
        })
        .await
        .unwrap();
    input = f.input.clone();
    input.order_id = o.id;
    assert!(matches!(
        f.runtime.preflight(&input).await,
        Err(Error::NotFound)
    ));
    let flight = f.runtime.preflight(&f.input).await.unwrap();
    let mut config = f.server.configuration.clone();
    config.enabled = false;
    f.runtime
        .store
        .configure_mcp_server(&f.server.project_id, &f.server.id, 1, "disable", &config)
        .await
        .unwrap();
    assert!(matches!(
        f.runtime.create_run(&f.input).await,
        Err(Error::Conflict)
    ));
    // Simulate a revision change after preflight but before the start transaction.
    let spec = RunSpec {
        input_workspace: None,
        agent_config: None,
        model_binding: None,
        output: None,
        workflow: None,
        mcp_tools: flight.mcp_tools,
        environment: None,
        order_id: f.input.order_id.clone(),
        project_id: f.server.project_id.clone(),
        prompt: f.input.prompt.clone(),
        provider: Some(flight.provider),
        file_action: None,
        access_mode: f.input.access_mode.clone(),
        executor: flight.executor,
        policy_revision: flight.policy_revision,
        allowed_tools: flight.allowed_tools,
        budgets: flight.budgets,
        folders: flight.folders,
    };
    assert!(matches!(
        f.runtime.store.create_run(&f.input, spec).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        f.runtime
            .store
            .order(&f.input.order_id)
            .await
            .unwrap()
            .revision,
        f.input.base_revision
    );
    assert_eq!(f.remote.calls(), 0);
}

#[tokio::test]
async fn revocation_after_approval_wait_is_checked_before_any_remote_io() {
    let f = fixture(AccessMode::ConfirmMutations, false).await;
    let run = f.runtime.create_run(&f.input).await.unwrap();
    let (stop, worker) = start(&f.runtime);
    let waiting = wait(&f.runtime, &run.id, RunState::WaitingApproval).await;
    let mut config = f.server.configuration.clone();
    config.enabled = false;
    f.runtime
        .store
        .configure_mcp_server(&f.server.project_id, &f.server.id, 1, "revoke", &config)
        .await
        .unwrap();
    let a = &waiting.approvals[0];
    f.runtime
        .store
        .resolve_approval(
            &run.id,
            &a.id,
            ResolveApproval {
                allow: true,
                fingerprint: a.fingerprint.clone(),
            },
        )
        .await
        .unwrap();
    wait(&f.runtime, &run.id, RunState::Failed).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(f.remote.calls(), 0);
    assert_eq!(
        f.remote
            .state
            .initializations
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn no_implicit_tool_selection_or_key_creation_at_preflight() {
    let f = fixture(AccessMode::ConfirmMutations, false).await;
    let active = f
        .runtime
        .store
        .native_secret_state()
        .await
        .unwrap()
        .unwrap()
        .active
        .unwrap();
    f.keys.corrupt(&active.key);
    assert!(matches!(
        f.runtime.preflight(&f.input).await,
        Err(Error::NativeKeyUnavailable)
    ));
    let mut without = f.input.clone();
    without.mcp_tools.clear();
    let flight = f.runtime.preflight(&without).await.unwrap();
    assert!(flight.mcp_tools.is_empty());
    assert!(!flight.allowed_tools.iter().any(|t| t.starts_with("mcp_")));
    assert_eq!(
        f.keys
            .keys
            .creates
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        f.remote
            .state
            .initializations
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}
