//! Actual public workflow creation and coordinator IO with immutable role prices.
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    routing::{get, post},
};
use cowork_contracts::*;
use cowork_domain::{Error, canonical_hash, terminal, workflows::*};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::{Store, model_money::*};
use cowork_workflow_contracts::{RunRequest, WorkflowRunKind, WorkflowRunRequest};
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
struct Server {
    probe: Arc<Mutex<Option<(Store, String)>>>,
    calls: Arc<AtomicUsize>,
    wire: Arc<Mutex<Vec<String>>>,
}
async fn chat(State(server): State<Server>, bytes: Bytes) -> String {
    server.calls.fetch_add(1, Ordering::SeqCst);
    let (store, order) = server.probe.lock().unwrap().clone().unwrap();
    let runs = store.runs(&order).await.unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let workflow = store
        .stored_workflow(&run.spec.project_id, &run.id)
        .await
        .unwrap();
    let pending: Vec<_> = workflow
        .agents
        .values()
        .flat_map(|a| &a.turns)
        .filter(|turn| turn.response.is_none() && json!(turn.context.messages) == body["messages"])
        .collect();
    assert_eq!(pending.len(), 1);
    let turn = pending[0];
    let profile = turn.context.spec.provider.as_ref().unwrap();
    assert_eq!(body["model"], profile.model);
    let prepared =
        cowork_runtime::provider::prepare(&turn.context.spec, &turn.context.messages, false)
            .unwrap();
    assert_eq!(bytes.as_ref(), prepared.payload());
    assert_eq!(prepared.logical_fingerprint(), turn.context.request_sha256);
    let record = store.stored_run(&run.id).await.unwrap().record;
    let attempt = record
        .model_attempts
        .iter()
        .find(|a| a.id == turn.id)
        .unwrap();
    assert_eq!(attempt.state, ModelAttemptState::InFlight);
    let ledger = store.monetary_ledger().await.unwrap();
    let reservation = ledger
        .reservation(&monetary_attempt_id(&run.id, &turn.id))
        .unwrap();
    assert!(matches!(reservation.state, ReservationState::Held));
    assert_eq!(reservation.spec.provider_id, profile.id);
    let expected = if profile.id == "priced-author" {
        16384 + 256
    } else {
        2 * 16384 + 256
    };
    assert_eq!(reservation.receipt.reserved_units, expected);
    assert_eq!(reservation.spec.request_sha256, prepared.wire_sha256());
    assert_eq!(reservation.spec.pools.len(), 3);
    assert!(
        reservation
            .spec
            .pools
            .iter()
            .any(|p| p.scope == PoolScope::Project(run.spec.project_id.clone()))
    );
    assert!(
        reservation
            .spec
            .pools
            .iter()
            .any(|p| p.scope == PoolScope::Run(run.id.clone()))
    );
    server
        .wire
        .lock()
        .unwrap()
        .push(prepared.wire_sha256().into());
    let response = json!({"choices":[{"index":0,"delta":{"content":"actual separately priced workflow answer"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":32,"prompt_tokens_details":{"cached_tokens":0}}});
    format!("data: {response}\n\ndata: [DONE]\n\n")
}
fn budgets() -> Budgets {
    Budgets {
        max_steps: 16,
        max_seconds: 60,
        max_output_tokens: 256,
        max_total_tokens: 131072,
    }
}
struct Fixture {
    temp: tempfile::TempDir,
    runtime: Runtime,
    input: WorkflowRunRequest,
    config: PersonalMoneyConfiguration,
    server: Server,
    handle: tokio::task::JoinHandle<()>,
    stop: CancellationToken,
    worker: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.cancel();
        self.handle.abort();
    }
}
impl Fixture {
    async fn setup(all_deployments: bool) -> Self {
        let server = Server {
            probe: Default::default(),
            calls: Default::default(),
            wire: Default::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({"data":[{"id":"priced-author"},{"id":"priced-checker"}]}))
                }),
            )
            .route("/chat/completions", post(chat))
            .with_state(server.clone());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&temp.path().join("workflow-money.db"))
            .await
            .unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Operator protected multi model workflow".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Two differently priced roles".into(),
            })
            .await
            .unwrap();
        let profiles: Vec<_> = ["priced-author", "priced-checker"]
            .into_iter()
            .map(|id| ProviderProfile {
                generation: Default::default(),
                id: id.into(),
                name: id.into(),
                kind: ProviderKind::OpenaiCompatible,
                endpoint: endpoint.clone(),
                model: id.into(),
                revision: 1,
                context_tokens: 16384,
                credential_required: false,
                credential_ready: true,
                capabilities: Default::default(),
            })
            .collect();
        let currency = Currency {
            code: "EUR".into(),
            scale: 6,
        };
        let config = PersonalMoneyConfiguration {
            format_version: 1,
            installation: PoolPolicy {
                scope: PoolScope::Installation("private-workflow-installation".into()),
                version: 1,
                currency: currency.clone(),
                limit_units: 100000,
            },
            project_limit_units: 100000,
            run_limit_units: 100000,
            pool_policy_version: 1,
            deployments: profiles
                .iter()
                .enumerate()
                .filter(|(i, _)| all_deployments || *i == 0)
                .map(|(i, profile)| {
                    let price = PriceVersion {
                        id: format!("operator-price-{i}"),
                        version: 1,
                        provider_id: profile.id.clone(),
                        model_sha256: if i == 0 { "a" } else { "c" }.repeat(64),
                        currency: currency.clone(),
                        input_units_per_million: (i as u64 + 1) * 1_000_000,
                        output_units_per_million: 1_000_000,
                        cached_input_units_per_million: None,
                        source_sha256: "b".repeat(64),
                    };
                    PersonalMoneyDeployment {
                        deployment: OperatorModelDeployment {
                            id: format!("operator-deployment-{i}"),
                            provider: profile.clone(),
                            model_sha256: price.model_sha256.clone(),
                            price_id: price.id.clone(),
                            price_version: 1,
                            token_mode: MonetaryTokenMode::ConservativeContextUpperBound,
                        },
                        price,
                    }
                })
                .collect(),
        };
        for profile in &profiles {
            store.insert_provider(profile).await.unwrap();
        }
        store.configure_personal_money(&config).await.unwrap();
        let rights = WorkflowRights {
            current_workspace: false,
            tools: Default::default(),
            folders: Default::default(),
        };
        let definition = WorkflowDefinition {
            documents: Default::default(),
            iterations: Default::default(),
            subflows: Default::default(),
            controls: Default::default(),
            gates: Default::default(),
            dataflow: None,
            format_version: 1,
            name: "Actual two provider workflow".into(),
            mode: WorkflowMode::Parallel { max_parallel: 2 },
            failure: WorkflowFailure::ContinueIndependent,
            rights: rights.clone(),
            budgets: budgets(),
            roles: profiles
                .iter()
                .map(|profile| WorkflowRole {
                    agent_config: None,
                    model_binding: None,
                    output: None,
                    id: profile.id.clone(),
                    instructions: "Answer once using your exact frozen role model".into(),
                    provider_id: profile.id.clone(),
                    provider_revision: 1,
                    rights: rights.clone(),
                    budgets: budgets(),
                    delegate_to: Default::default(),
                })
                .collect(),
            steps: profiles
                .iter()
                .enumerate()
                .map(|(i, profile)| WorkflowStep {
                    execution: None,
                    zero_context: false,
                    id: format!("step-{i}"),
                    role_id: profile.id.clone(),
                    goal: format!("Actual role-specific result {i}"),
                    dependencies: vec![],
                    rights: rights.clone(),
                    budgets: budgets(),
                })
                .collect(),
        };
        let library = store
            .create_workflow(&project.id, "two-priced-models", &definition)
            .await
            .unwrap();
        let input = WorkflowRunRequest {
            input_source: None,
            inputs: None,
            agent_config: None,
            native_tools: vec![],
            folders: vec![],
            mcp_tools: vec![],
            environment: None,
            kind: WorkflowRunKind::Workflow,
            order_id: order.id.clone(),
            base_revision: order.revision,
            idempotency_key: "priced-workflow-start".into(),
            prompt: "Produce both independent results".into(),
            workflow_id: library.id,
            version_id: library.versions[0].id.clone(),
            access_mode: AccessMode::Autonomous,
            budgets: budgets(),
            expected_binding: None,
        };
        *server.probe.lock().unwrap() = Some((store.clone(), order.id));
        let runtime = Runtime::new(
            store,
            profiles
                .into_iter()
                .map(|profile| ConfiguredProvider {
                    profile,
                    credential_env: None,
                })
                .collect(),
            false,
        )
        .unwrap();
        Self {
            temp,
            runtime,
            input,
            config,
            server,
            handle,
            stop: CancellationToken::new(),
            worker: None,
        }
    }
    fn serve(&mut self) {
        self.worker = Some(tokio::spawn(
            self.runtime.clone().serve(2, self.stop.clone()),
        ));
    }
    async fn stop_workers(&mut self) {
        self.stop.cancel();
        if let Some(worker) = self.worker.take() {
            worker.await.unwrap();
        }
    }
    async fn terminal(&self, run: &str) -> RunDetail {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let detail = self.runtime.store.detail(run).await.unwrap();
                if terminal(&detail.run.state) {
                    break detail;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn close(&mut self) {
        self.stop_workers().await;
        self.runtime.store.close().await;
        self.handle.abort();
        assert!(matches!((&mut self.handle).await, Err(e) if e.is_cancelled()));
    }
}

#[tokio::test]
async fn actual_public_workflow_uses_full_role_deployment_map_before_coordinator_io_and_replays_after_reopen()
 {
    let mut f = Fixture::setup(true).await;
    f.serve();
    let request = RunRequest::Workflow(f.input.clone());
    let run = f.runtime.create_run_request(&request).await.unwrap();
    assert!(run.spec.provider.is_none());
    let detail = f.terminal(&run.id).await;
    assert_eq!(
        detail.run.state,
        RunState::Completed,
        "{:?}",
        detail.run.error_code
    );
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.server.wire.lock().unwrap().len(), 2);
    f.stop_workers().await;
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(f.temp.path().join("workflow-money.db")),
    )
    .await
    .unwrap();
    let original: (String, String) =
        sqlx::query_as("SELECT payload_sha256,payload FROM accounting_personal_runs WHERE id=$1")
            .bind(&run.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let binding: Value = serde_json::from_str(&original.1).unwrap();
    assert_eq!(binding["format_version"], 2);
    assert_eq!(binding["configuration_sha256"], canonical_hash(&f.config));
    let mapping = binding["provider_deployments"].as_array().unwrap();
    assert_eq!(mapping.len(), 2);
    for entry in &f.config.deployments {
        let matched: Vec<_> = mapping
            .iter()
            .filter(|m| m["provider"] == json!(entry.deployment.provider))
            .collect();
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0]["deployment_id"], entry.deployment.id);
        assert_eq!(
            matched[0]["deployment_sha256"],
            canonical_hash(&entry.deployment)
        );
    }
    let ledger = f.runtime.store.monetary_ledger().await.unwrap();
    assert_eq!(ledger.reservations().len(), 2);
    assert!(
        ledger
            .reservations()
            .values()
            .all(|r| matches!(r.state, ReservationState::Settled { .. }))
    );
    assert_eq!(
        ledger.charged_to(&f.config.installation.scope).unwrap(),
        132 + 232
    );
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.temp.path().join("workflow-money.db"))
        .await
        .unwrap();
    let runtime = Runtime::new(reopened, vec![], false).unwrap();
    assert_eq!(
        runtime.create_run_request(&request).await.unwrap().id,
        run.id
    );
    assert_eq!(runtime.store.monetary_ledger().await.unwrap(), ledger);
    let after: (String, String) =
        sqlx::query_as("SELECT payload_sha256,payload FROM accounting_personal_runs WHERE id=$1")
            .bind(&run.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after, original);
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 2);
    runtime.store.close().await;
    pool.close().await;
    f.close().await;
}

#[tokio::test]
async fn missing_secondary_role_deployment_rolls_back_public_workflow_start_without_queue_or_io() {
    let mut f = Fixture::setup(false).await;
    let before = f.runtime.store.monetary_ledger().await.unwrap();
    let order = f.runtime.store.order(&f.input.order_id).await.unwrap();
    f.serve();
    assert!(matches!(
        f.runtime
            .create_run_request(&RunRequest::Workflow(f.input.clone()))
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        json!(f.runtime.store.order(&order.id).await.unwrap()),
        json!(order)
    );
    assert!(f.runtime.store.runs(&order.id).await.unwrap().is_empty());
    assert_eq!(f.runtime.store.monetary_ledger().await.unwrap(), before);
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(f.temp.path().join("workflow-money.db")),
    )
    .await
    .unwrap();
    for table in [
        "accounting_personal_runs",
        "events",
        "workflow_runs",
        "workflow_execution_clocks",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 0);
    pool.close().await;
    f.close().await;
}
