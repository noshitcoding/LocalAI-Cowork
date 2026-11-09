use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use cowork_contracts::*;
use cowork_domain::*;
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Fixture {
    reply: Value,
    requests: Arc<Mutex<Vec<Value>>>,
    truncated: bool,
    admission_probe: Arc<Mutex<Option<(Store, String)>>>,
    observed_wire: Arc<Mutex<Vec<String>>>,
}
async fn chat(State(f): State<Fixture>, Json(body): Json<Value>) -> String {
    let probe = f.admission_probe.lock().unwrap().clone();
    if let Some((store, run_id)) = probe {
        // Observe the actual committed reservation while the provider request
        // is still waiting for its response, rather than inspecting afterwards.
        let run = store.stored_run(&run_id).await.unwrap();
        let attempt = run.record.model_attempts.last().unwrap();
        assert_eq!(attempt.state, ModelAttemptState::InFlight);
        let prepared =
            cowork_runtime::provider::prepare(&run.record.spec, &run.checkpoint.messages, false)
                .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(prepared.payload()).unwrap(),
            body
        );
        assert_eq!(
            attempt.request_sha256.as_deref(),
            Some(prepared.logical_fingerprint())
        );
        let events = store.events(&run_id, 0).await.unwrap();
        let started = events
            .iter()
            .find(|event| {
                event.kind == "model_started" && event.payload["attempt"]["id"] == attempt.id
            })
            .unwrap();
        assert_eq!(started.payload["prepared_request"]["format"], 1);
        assert_eq!(
            started.payload["prepared_request"]["wire_sha256"],
            prepared.wire_sha256()
        );
        f.observed_wire
            .lock()
            .unwrap()
            .push(prepared.wire_sha256().to_owned());
    }
    f.requests.lock().unwrap().push(body);
    format!(
        "data: {}\n\n{}",
        f.reply,
        if f.truncated { "" } else { "data: [DONE]\n\n" }
    )
}
async fn setup(
    reply: Value,
    truncated: bool,
    total: u64,
) -> (
    tempfile::TempDir,
    Runtime,
    CreateRun,
    Fixture,
    tokio::task::JoinHandle<()>,
) {
    let fixture = Fixture {
        reply,
        requests: Arc::new(Mutex::new(vec![])),
        truncated,
        admission_probe: Default::default(),
        observed_wire: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route(
            "/models",
            get(|| async { Json(json!({"data":[{"id":"fixture"}]})) }),
        )
        .route("/chat/completions", post(chat))
        .with_state(fixture.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&temp.path().join("test.db")).await.unwrap();
    let p = store
        .create_project(CreateProject {
            name: "Budget fixture".into(),
        })
        .await
        .unwrap();
    let o = store
        .create_order(CreateOrder {
            project_id: p.id,
            title: "Budget fixture".into(),
        })
        .await
        .unwrap();
    let provider = ProviderProfile {
        generation: Default::default(),
        id: "fixture".into(),
        name: "Fixture".into(),
        kind: ProviderKind::OpenaiCompatible,
        endpoint,
        model: "fixture".into(),
        revision: 1,
        context_tokens: 16384,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile: provider,
            credential_env: None,
        }],
        false,
    )
    .unwrap();
    let input = CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        environment: None,
        mcp_tools: vec![],
        budgets: Budgets {
            max_total_tokens: total,
            ..Default::default()
        },
        folders: vec![],
        order_id: o.id,
        base_revision: 1,
        idempotency_key: "once".into(),
        prompt: "Fixture".into(),
        provider_id: "fixture".into(),
        provider_revision: 1,
        access_mode: AccessMode::Autonomous,
    };
    (temp, runtime, input, fixture, server)
}
#[tokio::test]
async fn exact_prepared_request_is_durable_before_single_agent_provider_io() {
    let (temp, runtime, input, fixture, server) = setup(
        response(json!({"prompt_tokens":123,"completion_tokens":7}), false),
        false,
        16384,
    )
    .await;
    let run = runtime.create_run(&input).await.unwrap();
    *fixture.admission_probe.lock().unwrap() = Some((runtime.store.clone(), run.id.clone()));
    let detail = finish(&runtime, &input).await;
    assert_eq!(detail.run.state, RunState::Completed);
    assert_eq!(fixture.observed_wire.lock().unwrap().len(), 1);
    let wire = fixture.observed_wire.lock().unwrap()[0].clone();
    runtime.store.close().await;
    let reopened = Store::sqlite(&temp.path().join("test.db")).await.unwrap();
    let events = reopened.events(&run.id, 0).await.unwrap();
    let started = events
        .iter()
        .find(|event| event.kind == "model_started")
        .unwrap();
    assert_eq!(started.payload["prepared_request"]["wire_sha256"], wire);
    assert_eq!(
        reopened
            .detail(&run.id)
            .await
            .unwrap()
            .run
            .model_attempts
            .len(),
        1
    );
    reopened.close().await;
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
async fn finish(runtime: &Runtime, input: &CreateRun) -> RunDetail {
    let run = runtime.create_run(input).await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(runtime.clone().serve(2, stop.clone()));
    let detail = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let detail = runtime.store.detail(&run.id).await.unwrap();
            if terminal(&detail.run.state) {
                break detail;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    stop.cancel();
    worker.await.unwrap();
    detail
}
fn response(usage: Value, tool: bool) -> Value {
    let delta = if tool {
        json!({"content":"Create fixture","tool_calls":[{"index":0,"id":"effect","function":{"name":"create_artifact","arguments":"{\"name\":\"test.txt\",\"content\":\"fixture\"}"}}]})
    } else {
        json!({"content":"Fixture complete"})
    };
    json!({"choices":[{"delta":delta,"finish_reason":if tool{"tool_calls"}else{"stop"}}],"usage":usage})
}
#[tokio::test]
async fn measured_usage_replaces_reservation_and_total_limit_is_frozen() {
    let (_temp, runtime, input, f, server) = setup(
        response(json!({"prompt_tokens":123,"completion_tokens":7}), false),
        false,
        16384,
    )
    .await;
    let detail = finish(&runtime, &input).await;
    assert_eq!(detail.run.state, RunState::Completed);
    assert_eq!(detail.token_budget.charged_tokens, 130);
    assert_eq!(detail.token_budget.unconfirmed_tokens, 0);
    assert!(!detail.run.usage.incomplete);
    assert_eq!(
        detail.run.model_attempts[0].state,
        ModelAttemptState::Measured
    );
    assert_eq!(f.requests.lock().unwrap()[0]["max_tokens"], 4096);
    assert_eq!(runtime.create_run(&input).await.unwrap().id, detail.run.id);
    let mut changed = input.clone();
    changed.budgets.max_total_tokens += 1;
    assert!(matches!(
        runtime.create_run(&changed).await,
        Err(Error::Conflict)
    ));
    server.abort();
}
#[tokio::test]
async fn missing_measurements_hold_capacity_and_prevent_a_second_request() {
    let (_temp, runtime, input, f, server) = setup(response(Value::Null, true), false, 16384).await;
    let detail = finish(&runtime, &input).await;
    assert_eq!(detail.run.state, RunState::Expired);
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    assert_eq!(detail.artifacts.len(), 1);
    assert_eq!(detail.token_budget.charged_tokens, 16384);
    assert_eq!(detail.token_budget.unconfirmed_tokens, 16384);
    assert!(detail.run.usage.incomplete);
    assert_eq!(
        detail.run.model_attempts[0].state,
        ModelAttemptState::Unmeasured
    );
    server.abort();
}
#[tokio::test]
async fn provider_overrun_is_recorded_before_any_returned_tool_executes() {
    let (_temp, runtime, input, f, server) = setup(
        response(json!({"prompt_tokens":100,"completion_tokens":5000}), true),
        false,
        16384,
    )
    .await;
    let detail = finish(&runtime, &input).await;
    assert_eq!(detail.run.state, RunState::Expired);
    assert!(detail.artifacts.is_empty());
    assert!(detail.approvals.is_empty());
    assert!(detail.token_budget.limit_exceeded);
    assert_eq!(detail.run.usage.output_tokens, Some(5000));
    assert_eq!(detail.token_budget.charged_tokens, 5100);
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    server.abort();
}
#[tokio::test]
async fn incomplete_stream_never_releases_reserved_capacity() {
    let (_temp, runtime, input, f, server) = setup(
        response(json!({"prompt_tokens":5,"completion_tokens":2}), true),
        true,
        16384,
    )
    .await;
    let detail = finish(&runtime, &input).await;
    assert_eq!(detail.run.state, RunState::Failed);
    assert!(detail.artifacts.is_empty());
    assert_eq!(detail.token_budget.charged_tokens, 16384);
    assert!(detail.run.usage.incomplete);
    assert_eq!(
        detail.run.model_attempts[0].state,
        ModelAttemptState::Interrupted
    );
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    server.abort();
}
#[tokio::test]
async fn preflight_rejects_invalid_or_insufficient_budgets_without_a_model_request() {
    let (_temp, runtime, mut input, f, server) =
        setup(response(Value::Null, false), false, 1024).await;
    assert!(matches!(
        runtime.create_run(&input).await,
        Err(Error::BudgetConfiguration)
    ));
    input.budgets.max_total_tokens = 16384;
    input.budgets.max_steps = 0;
    assert!(matches!(
        runtime.preflight(&input).await,
        Err(Error::Validation(_))
    ));
    input.budgets.max_steps = 64;
    input.prompt = "語".repeat(6000);
    assert!(matches!(
        runtime.preflight(&input).await,
        Err(Error::ContextLimit)
    ));
    assert!(f.requests.lock().unwrap().is_empty());
    assert!(
        runtime
            .store
            .runs(&input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    server.abort();
}
