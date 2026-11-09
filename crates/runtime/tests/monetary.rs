use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    routing::{get, post},
};
use cowork_contracts::*;
use cowork_domain::{Error, canonical_hash, reserve_model, terminal};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::{Change, Store, model_money::*};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const HELD: u64 = 16384 + 256;
#[derive(Clone)]
struct Server {
    runs: Arc<Mutex<BTreeMap<String, (Store, String)>>>,
    seen: Arc<Mutex<Vec<String>>>,
    calls: Arc<AtomicUsize>,
    usage_case: Arc<AtomicUsize>,
    gate: Arc<tokio::sync::Semaphore>,
    held: bool,
    truncated: bool,
    cached: bool,
}

async fn chat(State(server): State<Server>, bytes: Bytes) -> axum::response::Response {
    server.calls.fetch_add(1, Ordering::SeqCst);
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let (store, run_id) = {
        let runs = server.runs.lock().unwrap();
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|message| {
                message["content"]
                    .as_str()
                    .and_then(|prompt| runs.get(prompt).cloned())
            })
            .expect("actual request belongs to one registered fixture run")
    };
    let run_id = if let Some(order) = run_id.strip_prefix("order:") {
        let runs = store.runs(order).await.unwrap();
        assert_eq!(runs.len(), 1);
        runs[0].id.clone()
    } else {
        run_id
    };
    let run = store.stored_run(&run_id).await.unwrap();
    let attempt = run.record.model_attempts.last().unwrap();
    assert_eq!(attempt.state, ModelAttemptState::InFlight);
    let prepared =
        cowork_runtime::provider::prepare(&run.record.spec, &run.checkpoint.messages, false)
            .unwrap();
    assert_eq!(bytes.as_ref(), prepared.payload());
    let ledger = store.monetary_ledger().await.unwrap();
    let reservation = ledger
        .reservation(&monetary_attempt_id(&run_id, &attempt.id))
        .unwrap();
    assert!(matches!(reservation.state, ReservationState::Held));
    assert_eq!(reservation.receipt.reserved_units, HELD);
    assert_eq!(reservation.spec.request_sha256, prepared.wire_sha256());
    assert_eq!(
        attempt.request_sha256.as_deref(),
        Some(prepared.logical_fingerprint())
    );
    assert_eq!(reservation.spec.pools.len(), 3);
    assert!(
        reservation
            .spec
            .pools
            .iter()
            .any(|p| p.scope == PoolScope::Installation("private-installation".into()))
    );
    assert!(
        !reservation
            .spec
            .pools
            .iter()
            .any(|p| matches!(p.scope, PoolScope::Instance(_)))
    );
    let events = store.events(&run_id, 0).await.unwrap();
    let monetary = events
        .iter()
        .find(|e| e.kind == "monetary_model_reserved")
        .unwrap();
    assert_eq!(monetary.payload["wire_sha256"], prepared.wire_sha256());
    assert_eq!(
        monetary.payload["logical_sha256"],
        prepared.logical_fingerprint()
    );
    assert_eq!(monetary.payload["exact_tokenizer_qualified"], false);
    server.seen.lock().unwrap().push(run_id);
    if server.held {
        server.gate.acquire().await.unwrap().forget();
    }
    let mut usage = json!({"prompt_tokens":100,"completion_tokens":32});
    if server.cached {
        usage["prompt_tokens_details"] = json!({"cached_tokens":0});
    }
    match server.usage_case.load(Ordering::SeqCst) {
        1 => usage = Value::Null,
        2 => usage = json!({"prompt_tokens":100}),
        4..=6 => usage = json!({"completion_tokens":100000}),
        _ => {}
    }
    let mut response = json!({"choices":[{"index":0,"delta":{"content":"actual monetary answer"},"finish_reason":"stop"}],"usage":usage});
    if matches!(server.usage_case.load(Ordering::SeqCst), 4..=6) {
        response["choices"][0]["delta"]["tool_calls"] = json!([{"index":0,"id":"overrun-call","type":"function","function":{"name":"read_file","arguments":"{}"}}]);
        response["choices"][0]["finish_reason"] = json!("tool_calls");
    }
    let prefix = if server.usage_case.load(Ordering::SeqCst) == 3 {
        format!(
            "data: {}\n\n",
            json!({"choices":[],"usage":{"prompt_tokens":1,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":0}}})
        )
    } else {
        String::new()
    };
    let text = format!(
        "{prefix}data: {response}\n\n{}",
        if server.truncated || matches!(server.usage_case.load(Ordering::SeqCst), 5..=6) {
            ""
        } else {
            "data: [DONE]\n\n"
        }
    );
    if server.usage_case.load(Ordering::SeqCst) == 6 {
        let stream =
            futures_util::stream::once(
                async move { Ok::<Bytes, std::io::Error>(Bytes::from(text)) },
            )
            .chain(futures_util::stream::pending());
        axum::response::Response::new(axum::body::Body::from_stream(stream))
    } else {
        axum::response::Response::new(axum::body::Body::from(text))
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    runtime: Runtime,
    profile: ProviderProfile,
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
    fn configuration(&self, cap: u64) -> PersonalMoneyConfiguration {
        personal_configuration(&self.profile, cap)
    }
    async fn setup(
        cap: u64,
        held: bool,
        truncated: bool,
        cached: bool,
        mode: MonetaryTokenMode,
    ) -> Self {
        let server = Server {
            runs: Default::default(),
            seen: Default::default(),
            calls: Default::default(),
            usage_case: Default::default(),
            gate: Arc::new(tokio::sync::Semaphore::new(0)),
            held,
            truncated,
            cached,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route(
                "/models",
                get(|| async { Json(json!({"data":[{"id":"money-fixture"}]})) }),
            )
            .route("/chat/completions", post(chat))
            .with_state(server.clone());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&temp.path().join("money.db")).await.unwrap();
        let profile = ProviderProfile {
            generation: Default::default(),
            id: "money-fixture".into(),
            name: "Private money fixture".into(),
            kind: ProviderKind::OpenaiCompatible,
            endpoint,
            model: "money-fixture".into(),
            revision: 1,
            context_tokens: 16384,
            credential_required: false,
            credential_ready: true,
            capabilities: Default::default(),
        };
        let currency = Currency {
            code: "EUR".into(),
            scale: 6,
        };
        let price = PriceVersion {
            id: "operator-price".into(),
            version: 1,
            provider_id: profile.id.clone(),
            model_sha256: "a".repeat(64),
            currency: currency.clone(),
            input_units_per_million: 1_000_000,
            output_units_per_million: 1_000_000,
            cached_input_units_per_million: Some(500_000),
            source_sha256: "b".repeat(64),
        };
        store
            .register_personal_model_deployment(
                PoolPolicy {
                    scope: PoolScope::Installation("private-installation".into()),
                    version: 1,
                    currency,
                    limit_units: cap,
                },
                OperatorModelDeployment {
                    id: "operator-deployment".into(),
                    provider: profile.clone(),
                    model_sha256: price.model_sha256.clone(),
                    price_id: price.id.clone(),
                    price_version: 1,
                    token_mode: mode,
                },
                price,
            )
            .await
            .unwrap();
        let runtime = Runtime::new(
            store,
            vec![ConfiguredProvider {
                profile: profile.clone(),
                credential_env: None,
            }],
            false,
        )
        .unwrap();
        Self {
            temp,
            runtime,
            profile,
            server,
            handle,
            stop: CancellationToken::new(),
            worker: None,
        }
    }
    async fn run(&self, name: &str, bind: bool) -> (RunRecord, CreateRun) {
        self.run_seconds(name, bind, 30).await
    }
    async fn run_seconds(&self, name: &str, bind: bool, seconds: u64) -> (RunRecord, CreateRun) {
        let project = self
            .runtime
            .store
            .create_project(CreateProject { name: name.into() })
            .await
            .unwrap();
        let order = self
            .runtime
            .store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: name.into(),
            })
            .await
            .unwrap();
        let input = CreateRun {
            agent_config: None,
            model_binding: None,
            output: None,
            native_tools: vec![],
            environment: None,
            mcp_tools: vec![],
            budgets: Budgets {
                max_steps: 8,
                max_seconds: seconds,
                max_output_tokens: 256,
                max_total_tokens: 32768,
            },
            folders: vec![],
            order_id: order.id,
            base_revision: 1,
            idempotency_key: name.into(),
            prompt: name.into(),
            provider_id: self.profile.id.clone(),
            provider_revision: 1,
            access_mode: AccessMode::Autonomous,
        };
        self.server.runs.lock().unwrap().insert(
            name.into(),
            (
                self.runtime.store.clone(),
                format!("order:{}", input.order_id),
            ),
        );
        let run = self.runtime.create_run(&input).await.unwrap();
        self.server
            .runs
            .lock()
            .unwrap()
            .insert(name.into(), (self.runtime.store.clone(), run.id.clone()));
        if bind {
            let currency = Currency {
                code: "EUR".into(),
                scale: 6,
            };
            self.runtime
                .store
                .bind_personal_run_money(
                    &run.id,
                    "operator-deployment",
                    PoolPolicy {
                        scope: PoolScope::Project(project.id),
                        version: 1,
                        currency: currency.clone(),
                        limit_units: 1_000_000,
                    },
                    PoolPolicy {
                        scope: PoolScope::Run(run.id.clone()),
                        version: 1,
                        currency,
                        limit_units: 1_000_000,
                    },
                )
                .await
                .unwrap();
        }
        (run, input)
    }
    fn serve(&mut self, concurrency: usize) {
        self.worker = Some(tokio::spawn(
            self.runtime.clone().serve(concurrency, self.stop.clone()),
        ));
    }
    async fn terminal(&self, run: &str) -> RunDetail {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let detail = self.runtime.store.detail(run).await.unwrap();
                if terminal(&detail.run.state) || detail.run.state == RunState::Interrupted {
                    break detail;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn stop_workers(&mut self) {
        self.stop.cancel();
        if let Some(worker) = self.worker.take() {
            worker.await.unwrap();
        }
    }
    async fn close(&mut self) {
        self.stop_workers().await;
        self.runtime.store.close().await;
        self.handle.abort();
        assert!(matches!((&mut self.handle).await, Err(e) if e.is_cancelled()));
    }
}

fn personal_configuration(profile: &ProviderProfile, cap: u64) -> PersonalMoneyConfiguration {
    let currency = Currency {
        code: "EUR".into(),
        scale: 6,
    };
    PersonalMoneyConfiguration {
        format_version: 1,
        installation: PoolPolicy {
            scope: PoolScope::Installation("private-installation".into()),
            version: 1,
            currency: currency.clone(),
            limit_units: cap,
        },
        project_limit_units: 1_000_000,
        run_limit_units: 1_000_000,
        pool_policy_version: 1,
        deployments: vec![PersonalMoneyDeployment {
            deployment: OperatorModelDeployment {
                id: "operator-deployment".into(),
                provider: profile.clone(),
                model_sha256: "a".repeat(64),
                price_id: "operator-price".into(),
                price_version: 1,
                token_mode: MonetaryTokenMode::ConservativeContextUpperBound,
            },
            price: PriceVersion {
                id: "operator-price".into(),
                version: 1,
                provider_id: profile.id.clone(),
                model_sha256: "a".repeat(64),
                currency,
                input_units_per_million: 1_000_000,
                output_units_per_million: 1_000_000,
                cached_input_units_per_million: Some(500_000),
                source_sha256: "b".repeat(64),
            },
        }],
    }
}

#[tokio::test]
async fn configured_zero_installation_cap_denies_actual_worker_before_dispatch() {
    let mut f = Fixture::setup(
        0,
        false,
        false,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    f.runtime
        .store
        .configure_personal_money(&f.configuration(0))
        .await
        .unwrap();
    f.serve(1);
    let (run, _) = f.run("configured-deny-all", false).await;
    let detail = f.terminal(&run.id).await;
    assert_eq!(detail.run.state, RunState::Expired);
    assert!(detail.run.model_attempts.is_empty());
    assert!(
        f.runtime
            .store
            .monetary_ledger()
            .await
            .unwrap()
            .reservations()
            .is_empty()
    );
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 0);
    f.close().await;
}

#[tokio::test]
async fn configured_workers_observe_atomic_binding_during_concurrent_creation_and_reopen_replay() {
    let mut f = Fixture::setup(
        HELD + 132,
        true,
        false,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    let config = f.configuration(HELD + 132);
    f.runtime
        .store
        .configure_personal_money(&config)
        .await
        .unwrap();
    f.serve(2);
    let (a, b) = tokio::join!(
        f.run("configured-race-a", false),
        f.run("configured-race-b", false)
    );
    let admitted = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(id) = f.server.seen.lock().unwrap().first().cloned() {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let rejected = if admitted == a.0.id { &b.0.id } else { &a.0.id };
    assert_eq!(f.terminal(rejected).await.run.state, RunState::Expired);
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(f.temp.path().join("money.db")),
    )
    .await
    .unwrap();
    let bindings: Vec<String> =
        sqlx::query_scalar("SELECT payload FROM accounting_personal_runs ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(bindings.len(), 2);
    for raw in &bindings {
        let binding: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(binding["format_version"], 2);
        assert_eq!(binding["configuration_sha256"], canonical_hash(&config));
        assert_eq!(
            binding["provider_deployments"][0]["provider"],
            json!(f.profile)
        );
        assert_eq!(binding["provider_deployments"].as_array().unwrap().len(), 1);
    }
    f.server.gate.add_permits(1);
    assert_eq!(f.terminal(&admitted).await.run.state, RunState::Completed);
    f.stop_workers().await;
    let ledger = f.runtime.store.monetary_ledger().await.unwrap();
    assert_eq!(ledger.charged_to(&config.installation.scope).unwrap(), 132);
    let mut changed = config.clone();
    changed.installation.limit_units += 1;
    assert!(matches!(
        f.runtime.store.configure_personal_money(&changed).await,
        Err(Error::Conflict)
    ));
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.temp.path().join("money.db"))
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(
            &reopened
                .personal_money_configuration()
                .await
                .unwrap()
                .unwrap()
        ),
        canonical_hash(&config)
    );
    let mut altered = f.profile.clone();
    altered.model = "unconfigured-after-restart".into();
    let runtime = Runtime::new(
        reopened,
        vec![ConfiguredProvider {
            profile: altered,
            credential_env: None,
        }],
        false,
    )
    .unwrap();
    for (run, input) in [&a, &b] {
        assert_eq!(runtime.create_run(input).await.unwrap().id, run.id);
    }
    assert_eq!(runtime.store.monetary_ledger().await.unwrap(), ledger);
    let after: Vec<String> =
        sqlx::query_scalar("SELECT payload FROM accounting_personal_runs ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(after, bindings);
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
    runtime.store.close().await;
    pool.close().await;
    f.close().await;
}

#[tokio::test]
async fn activation_final_sql_failure_rolls_back_all_operator_state_and_zero_caps_are_durable() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&temp.path().join("money.db")).await.unwrap();
    let profile = ProviderProfile {
        generation: Default::default(),
        id: "money-fixture".into(),
        name: "Private operator".into(),
        kind: ProviderKind::OpenaiCompatible,
        endpoint: "http://127.0.0.1:1".into(),
        model: "money-fixture".into(),
        revision: 1,
        context_tokens: 16384,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    let config = personal_configuration(&profile, 0);
    let before = store.monetary_ledger().await.unwrap();
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(temp.path().join("money.db")),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER reject_operator_activation BEFORE INSERT ON accounting_personal_configuration BEGIN SELECT RAISE(ABORT,'activation last SQL rollback'); END").execute(&pool).await.unwrap();
    assert!(store.configure_personal_money(&config).await.is_err());
    assert_eq!(store.monetary_ledger().await.unwrap(), before);
    for table in [
        "accounting_personal_configuration",
        "accounting_personal_installation",
        "accounting_provider_deployments",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    sqlx::query("DROP TRIGGER reject_operator_activation")
        .execute(&pool)
        .await
        .unwrap();
    store.configure_personal_money(&config).await.unwrap();
    let ledger = store.monetary_ledger().await.unwrap();
    store.configure_personal_money(&config).await.unwrap();
    assert_eq!(store.monetary_ledger().await.unwrap(), ledger);
    store.close().await;
    let reopened = Store::sqlite(&temp.path().join("money.db")).await.unwrap();
    assert_eq!(
        canonical_hash(
            &reopened
                .personal_money_configuration()
                .await
                .unwrap()
                .unwrap()
        ),
        canonical_hash(&config)
    );
    assert_eq!(reopened.monetary_ledger().await.unwrap(), ledger);
    reopened.close().await;
    pool.close().await;
}

#[tokio::test]
async fn strict_operator_profile_and_price_validation_has_no_partial_activation() {
    let mut f = Fixture::setup(
        100_000,
        false,
        false,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    let config = f.configuration(100_000);
    let before = f.runtime.store.monetary_ledger().await.unwrap();
    let mut malformed = Vec::new();
    let mut value = config.clone();
    value.format_version = 2;
    malformed.push(value);
    let mut value = config.clone();
    value.deployments.push(value.deployments[0].clone());
    malformed.push(value);
    let mut value = config.clone();
    value.deployments[0].deployment.provider.endpoint = "http://example.org".into();
    malformed.push(value);
    let mut value = config.clone();
    value.deployments[0].price.currency.code = "USD".into();
    malformed.push(value);
    let mut value = config.clone();
    value.deployments[0].price.model_sha256 = "c".repeat(64);
    malformed.push(value);
    let mut value = config.clone();
    value.deployments[0].price.source_sha256 = "unknown".into();
    malformed.push(value);
    for value in malformed {
        assert!(
            f.runtime
                .store
                .configure_personal_money(&value)
                .await
                .is_err()
        );
        assert!(
            f.runtime
                .store
                .personal_money_configuration()
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(f.runtime.store.monetary_ledger().await.unwrap(), before);
    }
    let mut json_value = json!(config);
    json_value["deployments"][0]["deployment"]["provider"]["money_authority"] = json!(true);
    assert!(serde_json::from_value::<PersonalMoneyConfiguration>(json_value).is_err());
    let mut json_value = json!(config);
    json_value["installation"]["scope"]["money_authority"] = json!(true);
    assert!(serde_json::from_value::<PersonalMoneyConfiguration>(json_value).is_err());
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 0);
    f.close().await;
}

#[tokio::test]
async fn configured_creation_final_event_failure_rolls_back_order_run_pools_binding_and_key() {
    let mut f = Fixture::setup(
        100_000,
        false,
        false,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    f.runtime
        .store
        .configure_personal_money(&f.configuration(100_000))
        .await
        .unwrap();
    let (prior, _) = f.run("seed-request", false).await;
    let project = f
        .runtime
        .store
        .create_project(CreateProject {
            name: "Rollback order".into(),
        })
        .await
        .unwrap();
    let order = f
        .runtime
        .store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "Original draft".into(),
        })
        .await
        .unwrap();
    let order = f
        .runtime
        .store
        .save_draft(
            &order.id,
            SaveDraft {
                base_revision: order.revision,
                draft: "retained original draft".into(),
            },
        )
        .await
        .unwrap();
    let input = CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        order_id: order.id.clone(),
        base_revision: order.revision,
        idempotency_key: "rollback-configured-create".into(),
        prompt: "new configured request".into(),
        provider_id: f.profile.id.clone(),
        provider_revision: 1,
        access_mode: AccessMode::Autonomous,
        budgets: prior.spec.budgets,
        folders: vec![],
        mcp_tools: vec![],
        environment: None,
    };
    let before = f.runtime.store.monetary_ledger().await.unwrap();
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(f.temp.path().join("money.db")),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER reject_configured_creation BEFORE INSERT ON events WHEN json_extract(NEW.payload,'$.kind')='run_queued' BEGIN SELECT RAISE(ABORT,'configured last event rollback'); END").execute(&pool).await.unwrap();
    assert!(f.runtime.create_run(&input).await.is_err());
    assert_eq!(
        json!(f.runtime.store.order(&order.id).await.unwrap()),
        json!(order)
    );
    assert!(f.runtime.store.runs(&order.id).await.unwrap().is_empty());
    assert_eq!(f.runtime.store.monetary_ledger().await.unwrap(), before);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounting_personal_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "only the existing seed binding remains");
    sqlx::query("DROP TRIGGER reject_configured_creation")
        .execute(&pool)
        .await
        .unwrap();
    let created = f.runtime.create_run(&input).await.unwrap();
    assert_eq!(f.runtime.create_run(&input).await.unwrap().id, created.id);
    assert_eq!(
        f.runtime.store.events(&created.id, 0).await.unwrap().len(),
        1
    );
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 0);
    pool.close().await;
    f.close().await;
}

#[tokio::test]
async fn activation_rejects_preexisting_unbound_run_preserves_legacy_binding_and_blocks_profile_substitution()
 {
    let mut f = Fixture::setup(
        100_000,
        false,
        false,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    let (legacy, _) = f.run("legacy-explicit", true).await;
    let (unbound, _) = f.run("legacy-unbound", false).await;
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(f.temp.path().join("money.db")),
    )
    .await
    .unwrap();
    let original: (String, String) =
        sqlx::query_as("SELECT payload_sha256,payload FROM accounting_personal_runs WHERE id=$1")
            .bind(&legacy.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    f.runtime
        .store
        .configure_personal_money(&f.configuration(100_000))
        .await
        .unwrap();
    f.serve(2);
    assert_eq!(f.terminal(&legacy.id).await.run.state, RunState::Completed);
    assert_eq!(f.terminal(&unbound.id).await.run.state, RunState::Failed);
    assert!(
        f.runtime
            .store
            .stored_run(&unbound.id)
            .await
            .unwrap()
            .record
            .model_attempts
            .is_empty()
    );
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
    let after: (String, String) =
        sqlx::query_as("SELECT payload_sha256,payload FROM accounting_personal_runs WHERE id=$1")
            .bind(&legacy.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after, original);
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM accounting_personal_runs WHERE id=$1")
            .bind(&unbound.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    f.stop_workers().await;
    let project = f
        .runtime
        .store
        .create_project(CreateProject {
            name: "Substitution".into(),
        })
        .await
        .unwrap();
    let order = f
        .runtime
        .store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "same ID different complete profile".into(),
        })
        .await
        .unwrap();
    let input = CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        order_id: order.id.clone(),
        base_revision: 1,
        idempotency_key: "same-id-profile-change".into(),
        prompt: "missing exact deployment".into(),
        provider_id: f.profile.id.clone(),
        provider_revision: 1,
        access_mode: AccessMode::Autonomous,
        budgets: legacy.spec.budgets,
        folders: vec![],
        mcp_tools: vec![],
        environment: None,
    };
    let mut profile = f.profile.clone();
    profile.name = "Changed frozen full profile".into();
    let runtime = Runtime::new(
        f.runtime.store.clone(),
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        false,
    )
    .unwrap();
    assert!(matches!(
        runtime.create_run(&input).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        json!(f.runtime.store.order(&order.id).await.unwrap()),
        json!(order)
    );
    assert!(f.runtime.store.runs(&order.id).await.unwrap().is_empty());
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
    pool.close().await;
    f.close().await;
}

#[tokio::test]
async fn actual_two_projects_compete_for_one_global_installation_hold_then_settle() {
    let mut f = Fixture::setup(
        HELD + 132,
        true,
        false,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    let (a, _) = f.run("money-a", true).await;
    let (b, _) = f.run("money-b", true).await;
    assert_ne!(a.spec.project_id, b.spec.project_id);
    f.serve(2);
    let admitted = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(id) = f.server.seen.lock().unwrap().first().cloned() {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let rejected = if admitted == a.id { &b.id } else { &a.id };
    let denied = f.terminal(rejected).await;
    assert_eq!(denied.run.state, RunState::Expired);
    assert!(
        denied.run.model_attempts.is_empty(),
        "global cap rollback includes actual attempt"
    );
    let ledger = f.runtime.store.monetary_ledger().await.unwrap();
    assert_eq!(ledger.reservations().len(), 1);
    assert_eq!(
        ledger
            .charged_to(&PoolScope::Installation("private-installation".into()))
            .unwrap(),
        HELD
    );
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
    f.server.gate.add_permits(1);
    assert_eq!(f.terminal(&admitted).await.run.state, RunState::Completed);
    let ledger = f.runtime.store.monetary_ledger().await.unwrap();
    assert_eq!(
        ledger
            .charged_to(&PoolScope::Installation("private-installation".into()))
            .unwrap(),
        132
    );
    assert!(matches!(
        ledger.reservations().values().next().unwrap().state,
        ReservationState::Settled { .. }
    ));
    let (third, _) = f.run("money-third", true).await;
    f.server.gate.add_permits(1);
    assert_eq!(f.terminal(&third.id).await.run.state, RunState::Completed);
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        f.runtime
            .store
            .monetary_ledger()
            .await
            .unwrap()
            .charged_to(&PoolScope::Installation("private-installation".into()))
            .unwrap(),
        264
    );
    f.close().await;
}

#[tokio::test]
async fn truncated_actual_provider_retains_global_hold_across_reopen_without_replay() {
    let mut f = Fixture::setup(
        HELD,
        false,
        true,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    let (run, input) = f.run("money-truncated", true).await;
    f.serve(1);
    let detail = f.terminal(&run.id).await;
    assert_eq!(detail.run.model_attempts.len(), 1);
    f.stop_workers().await;
    let before = f.runtime.store.monetary_ledger().await.unwrap();
    assert_eq!(
        before
            .charged_to(&PoolScope::Installation("private-installation".into()))
            .unwrap(),
        HELD
    );
    assert!(matches!(
        before.reservations().values().next().unwrap().state,
        ReservationState::Uncertain {
            evidence: Some(_),
            ..
        }
    ));
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.temp.path().join("money.db"))
        .await
        .unwrap();
    assert_eq!(reopened.monetary_ledger().await.unwrap(), before);
    assert!(reopened.resume(&run.id).await.is_err());
    let runtime = Runtime::new(
        reopened,
        vec![ConfiguredProvider {
            profile: f.profile.clone(),
            credential_env: None,
        }],
        false,
    )
    .unwrap();
    assert_eq!(runtime.create_run(&input).await.unwrap().id, run.id);
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
    runtime.store.close().await;
    f.close().await;
}

#[tokio::test]
async fn actual_partial_output_overrun_charges_shared_cap_without_tools_or_reopen_release() {
    let mut f = Fixture::setup(
        100000,
        true,
        false,
        false,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    f.server.usage_case.store(4, Ordering::SeqCst);
    let (run, input) = f.run("partial-overrun", true).await;
    f.serve(1);
    tokio::time::timeout(Duration::from_secs(10), async {
        while f.server.seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.server.seen.lock().unwrap().as_slice(),
        std::slice::from_ref(&run.id)
    );
    let (other, _) = f.run("blocked-after-partial-overrun", true).await;
    f.server.gate.add_permits(1);
    let detail = f.terminal(&run.id).await;
    assert_eq!(detail.run.state, RunState::Expired);
    assert_eq!(detail.run.error_code.as_deref(), Some("budget_exceeded"));
    let stored = f.runtime.store.stored_run(&run.id).await.unwrap();
    assert!(stored.checkpoint.pending_tools.is_empty());
    assert!(detail.mcp_attempts.is_empty());
    assert!(detail.process_attempts.is_empty());
    let ledger = f.runtime.store.monetary_ledger().await.unwrap();
    let reservation = ledger.reservations().values().next().unwrap();
    assert!(
        matches!(&reservation.state, ReservationState::Uncertain { evidence: Some(usage), .. } if usage.input_tokens.is_none() && usage.output_tokens == Some(100000) && usage.cached_input_tokens.is_none())
    );
    assert!(!reservation.complete_usage());
    assert!(reservation.overrun());
    assert_eq!(reservation.charged_units().unwrap(), 100000);
    assert!(reservation.charged_units().unwrap() > reservation.receipt.reserved_units);
    assert_eq!(
        ledger
            .charged_to(&PoolScope::Installation("private-installation".into()))
            .unwrap(),
        100000
    );
    let events = f.runtime.store.events(&run.id, 0).await.unwrap();
    let response = events
        .iter()
        .find(|e| e.kind == "monetary_model_response")
        .unwrap();
    assert_eq!(response.payload["complete_usage"], false);
    assert_eq!(response.payload["charged_units"], 100000);
    assert_eq!(response.payload["overrun"], true);
    assert_eq!(
        response.payload["received"]["completed_usage"]["completion_tokens"],
        100000
    );
    assert!(
        events
            .iter()
            .any(|e| e.kind == "model_response_rejected" && e.payload["tools_executed"] == false)
    );
    assert!(!events.iter().any(|e| e.kind == "model_completed"));
    assert_eq!(f.terminal(&other.id).await.run.state, RunState::Expired);
    assert!(
        f.runtime
            .store
            .stored_run(&other.id)
            .await
            .unwrap()
            .record
            .model_attempts
            .is_empty()
    );
    assert_eq!(f.runtime.store.monetary_ledger().await.unwrap(), ledger);
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
    f.stop_workers().await;
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.temp.path().join("money.db"))
        .await
        .unwrap();
    assert_eq!(reopened.monetary_ledger().await.unwrap(), ledger);
    assert!(reopened.resume(&run.id).await.is_err());
    let runtime = Runtime::new(
        reopened,
        vec![ConfiguredProvider {
            profile: f.profile.clone(),
            credential_env: None,
        }],
        false,
    )
    .unwrap();
    assert_eq!(runtime.create_run(&input).await.unwrap().id, run.id);
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
    runtime.store.close().await;
    f.close().await;
}

#[tokio::test]
async fn missing_actual_cache_details_keeps_money_uncertain_without_zero_cache_guess() {
    let mut f = Fixture::setup(
        HELD,
        false,
        false,
        false,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    let (run, _) = f.run("money-cache-unknown", true).await;
    f.serve(1);
    assert_eq!(f.terminal(&run.id).await.run.state, RunState::Completed);
    let ledger = f.runtime.store.monetary_ledger().await.unwrap();
    let reservation = ledger.reservations().values().next().unwrap();
    assert!(matches!(
        reservation.state,
        ReservationState::Uncertain { .. }
    ));
    assert_eq!(reservation.charged_units().unwrap(), HELD);
    let events = f.runtime.store.events(&run.id, 0).await.unwrap();
    let response = events
        .iter()
        .find(|e| e.kind == "monetary_model_response")
        .unwrap();
    assert_eq!(response.payload["complete_usage"], false);
    assert!(response.payload["received"]["usage"]["cached_input_tokens"].is_null());
    assert_eq!(
        response.payload["received"]["completed_usage"]["prompt_tokens"],
        100
    );
    assert!(
        response.payload["received"]["raw_response_sha256"]
            .as_str()
            .is_some_and(|v| v.len() == 64)
    );
    f.close().await;
}

#[tokio::test]
async fn actual_response_transaction_failure_rolls_back_settlement_and_checkpoint() {
    let mut f = Fixture::setup(
        HELD,
        false,
        false,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    let (run, _) = f.run("money-commit-failure", true).await;
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(f.temp.path().join("money.db")),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER reject_money_answer BEFORE INSERT ON accounting_operations WHEN NEW.kind='settlement' BEGIN SELECT RAISE(ABORT,'private monetary settlement failure after ledger and run writes'); END").execute(&pool).await.unwrap();
    f.serve(1);
    f.terminal(&run.id).await;
    f.stop_workers().await;
    let ledger = f.runtime.store.monetary_ledger().await.unwrap();
    assert!(matches!(
        ledger.reservations().values().next().unwrap().state,
        ReservationState::Uncertain {
            evidence: Some(_),
            ..
        }
    ));
    assert_eq!(
        ledger
            .charged_to(&PoolScope::Installation("private-installation".into()))
            .unwrap(),
        HELD
    );
    let stored = f.runtime.store.stored_run(&run.id).await.unwrap();
    assert!(
        !stored
            .checkpoint
            .messages
            .iter()
            .any(|m| m["content"] == "actual monetary answer")
    );
    assert!(
        !f.runtime
            .store
            .events(&run.id, 0)
            .await
            .unwrap()
            .iter()
            .any(|e| e.kind == "model_completed" || e.kind == "monetary_model_response")
    );
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
    sqlx::query("DROP TRIGGER reject_money_answer")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.temp.path().join("money.db"))
        .await
        .unwrap();
    assert_eq!(reopened.monetary_ledger().await.unwrap(), ledger);
    assert!(reopened.resume(&run.id).await.is_err());
    reopened.close().await;
    f.close().await;
}

#[tokio::test]
async fn unknown_exact_renderer_and_unbound_active_run_fail_before_actual_io() {
    let mut f = Fixture::setup(
        HELD * 2,
        false,
        false,
        true,
        MonetaryTokenMode::ExactArtifactRenderer,
    )
    .await;
    let (exact, _) = f.run("money-exact-unknown", true).await;
    let (unbound, _) = f.run("money-unbound", false).await;
    f.serve(2);
    assert_eq!(f.terminal(&exact.id).await.run.state, RunState::Failed);
    assert_eq!(f.terminal(&unbound.id).await.run.state, RunState::Failed);
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 0);
    assert!(
        f.runtime
            .store
            .monetary_ledger()
            .await
            .unwrap()
            .reservations()
            .is_empty()
    );
    f.close().await;
}

#[tokio::test]
async fn ledger_writer_wait_rechecks_actual_lease_and_rolls_back_admission() {
    let mut f = Fixture::setup(
        HELD,
        false,
        false,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    let (run, _) = f.run("money-lock-wait", true).await;
    let claimed = f.runtime.store.claim_for_worker().await.unwrap().unwrap();
    assert_eq!(claimed.record.id, run.id);
    let token = claimed.lease.as_ref().unwrap().token.clone();
    f.runtime
        .store
        .mutate(&run.id, Some(&token), |r| {
            r.lease.as_mut().unwrap().until = cowork_storage::now() + 250;
            Ok(Change::default())
        })
        .await
        .unwrap();
    let prepared = cowork_runtime::provider::prepare(
        &claimed.record.spec,
        &claimed.checkpoint.messages,
        false,
    )
    .unwrap();
    let wire = prepared.wire_sha256().to_owned();
    let logical = prepared.logical_fingerprint().to_owned();
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(f.temp.path().join("money.db")),
    )
    .await
    .unwrap();
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("UPDATE accounting_ledgers SET revision=revision")
        .execute(&mut *writer)
        .await
        .unwrap();
    let store = f.runtime.store.clone();
    let run_id = run.id.clone();
    let waiting = tokio::spawn(async move {
        store
            .admit_prepared_model_attempt(
                &run_id,
                &token,
                "lock-wait-attempt",
                &logical,
                &wire,
                |r| {
                    reserve_model(
                        &mut r.record,
                        &logical,
                        "lock-wait-attempt",
                        cowork_storage::now(),
                    )?;
                    Ok(Change::event("model_started", json!({})))
                },
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !waiting.is_finished(),
        "writer wait actually precedes lease admission"
    );
    writer.rollback().await.unwrap();
    assert!(matches!(waiting.await.unwrap(), Err(Error::StaleLease)));
    assert!(
        f.runtime
            .store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .model_attempts
            .is_empty()
    );
    assert!(
        f.runtime
            .store
            .monetary_ledger()
            .await
            .unwrap()
            .reservations()
            .is_empty()
    );
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 0);
    pool.close().await;
    f.close().await;
}

#[tokio::test]
async fn copied_actual_receipt_cannot_settle_a_second_identical_provider_request() {
    let mut f = Fixture::setup(
        HELD * 2,
        false,
        false,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    let (first, _) = f.run("identical-body", true).await;
    let claimed = f.runtime.store.claim_for_worker().await.unwrap().unwrap();
    let token = claimed.lease.as_ref().unwrap().token.clone();
    let prepared = cowork_runtime::provider::prepare(
        &claimed.record.spec,
        &claimed.checkpoint.messages,
        false,
    )
    .unwrap();
    let first_payload = prepared.payload().to_vec();
    let logical = prepared.logical_fingerprint().to_owned();
    let wire = prepared.wire_sha256().to_owned();
    f.runtime
        .store
        .admit_prepared_model_attempt(&first.id, &token, "same-turn", &logical, &wire, |r| {
            reserve_model(&mut r.record, &logical, "same-turn", cowork_storage::now())?;
            Ok(Change::event("model_started", json!({})))
        })
        .await
        .unwrap();
    let scope = f
        .runtime
        .store
        .monetary_dispatch_scope(&first.id, &token, "same-turn")
        .await
        .unwrap()
        .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let actual =
        cowork_runtime::provider::complete_prepared_with_dispatch(prepared, None, tx, Some(scope))
            .await
            .unwrap();
    let received = actual.received.as_ref().unwrap();
    f.runtime
        .store
        .settle_received_model_attempt(&first.id, &token, "same-turn", Some(received), |r| {
            cowork_domain::settle_model(
                &mut r.record,
                "same-turn",
                &actual.usage,
                cowork_storage::now(),
            )?;
            Ok(Change::event("model_completed", json!({})))
        })
        .await
        .unwrap();
    let (second, _) = f.run("identical-body", true).await;
    let second_claim = f.runtime.store.claim_for_worker().await.unwrap().unwrap();
    assert_eq!(second_claim.record.id, second.id);
    let token2 = second_claim.lease.as_ref().unwrap().token.clone();
    let prepared2 = cowork_runtime::provider::prepare(
        &second_claim.record.spec,
        &second_claim.checkpoint.messages,
        false,
    )
    .unwrap();
    assert_eq!(prepared2.payload(), first_payload);
    assert_eq!(prepared2.wire_sha256(), wire);
    assert_eq!(prepared2.logical_fingerprint(), logical);
    f.runtime
        .store
        .admit_prepared_model_attempt(&second.id, &token2, "same-turn", &logical, &wire, |r| {
            reserve_model(&mut r.record, &logical, "same-turn", cowork_storage::now())?;
            Ok(Change::event("model_started", json!({})))
        })
        .await
        .unwrap();
    let before = f.runtime.store.monetary_ledger().await.unwrap();
    let events = canonical_hash(&f.runtime.store.events(&second.id, 0).await.unwrap());
    let copied = received.clone();
    let rejected = f
        .runtime
        .store
        .settle_received_model_attempt(&second.id, &token2, "same-turn", Some(&copied), |r| {
            cowork_domain::settle_model(
                &mut r.record,
                "same-turn",
                &actual.usage,
                cowork_storage::now(),
            )?;
            Ok(Change::event("model_completed", json!({})))
        })
        .await;
    assert!(matches!(rejected, Err(Error::Forbidden)));
    assert_eq!(f.runtime.store.monetary_ledger().await.unwrap(), before);
    assert_eq!(
        canonical_hash(&f.runtime.store.events(&second.id, 0).await.unwrap()),
        events
    );
    assert_eq!(
        f.runtime
            .store
            .stored_run(&second.id)
            .await
            .unwrap()
            .record
            .model_attempts[0]
            .state,
        ModelAttemptState::InFlight
    );
    assert_eq!(
        f.server.calls.load(Ordering::SeqCst),
        1,
        "copied receipt granted no second HTTP and released no second hold"
    );
    f.close().await;
}

#[tokio::test]
async fn null_partial_ambiguous_or_unterminated_actual_usage_never_releases_hold() {
    for (usage_case, truncated, uncertain) in [
        (1, false, true),
        (2, false, true),
        (3, false, false),
        (0, true, false),
    ] {
        let mut f = Fixture::setup(
            HELD,
            false,
            truncated,
            true,
            MonetaryTokenMode::ConservativeContextUpperBound,
        )
        .await;
        f.server.usage_case.store(usage_case, Ordering::SeqCst);
        let (run, _) = f.run("money-usage-negative", true).await;
        f.serve(1);
        f.terminal(&run.id).await;
        let ledger = f.runtime.store.monetary_ledger().await.unwrap();
        let reservation = ledger.reservations().values().next().unwrap();
        assert_eq!(reservation.charged_units().unwrap(), HELD);
        assert!(matches!(
            reservation.state,
            ReservationState::Uncertain {
                evidence: Some(_),
                ..
            }
        ));
        if !uncertain {
            assert!(
                !f.runtime
                    .store
                    .events(&run.id, 0)
                    .await
                    .unwrap()
                    .iter()
                    .any(|event| event.kind == "monetary_model_response"
                        || event.kind == "model_completed")
            );
        }
        assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
        f.close().await;
    }
}

#[tokio::test]
async fn ledger_writer_wait_rechecks_original_run_budget_with_lease_still_valid() {
    let mut f = Fixture::setup(
        HELD,
        false,
        false,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    let (run, _) = f.run_seconds("money-run-budget-wait", true, 1).await;
    let claimed = f.runtime.store.claim_for_worker().await.unwrap().unwrap();
    let token = claimed.lease.as_ref().unwrap().token.clone();
    let until = claimed.lease.as_ref().unwrap().until;
    let prepared = cowork_runtime::provider::prepare(
        &claimed.record.spec,
        &claimed.checkpoint.messages,
        false,
    )
    .unwrap();
    let wire = prepared.wire_sha256().to_owned();
    let logical = prepared.logical_fingerprint().to_owned();
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(f.temp.path().join("money.db")),
    )
    .await
    .unwrap();
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("UPDATE accounting_ledgers SET revision=revision")
        .execute(&mut *writer)
        .await
        .unwrap();
    let store = f.runtime.store.clone();
    let run_id = run.id.clone();
    let waiting = tokio::spawn(async move {
        store
            .admit_prepared_model_attempt(&run_id, &token, "budget-wait", &logical, &wire, |r| {
                reserve_model(
                    &mut r.record,
                    &logical,
                    "budget-wait",
                    cowork_storage::now(),
                )?;
                Ok(Change::event("model_started", json!({})))
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(!waiting.is_finished());
    assert!(
        cowork_storage::now() < until,
        "failure is run deadline, not lease expiry"
    );
    writer.rollback().await.unwrap();
    assert!(matches!(waiting.await.unwrap(), Err(Error::BudgetExceeded)));
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
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .model_attempts
            .is_empty()
    );
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 0);
    pool.close().await;
    f.close().await;
}

#[tokio::test]
async fn ordinary_change_and_event_json_cannot_create_an_accounted_model_permit() {
    let mut f = Fixture::setup(
        HELD,
        false,
        false,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    let (run, _) = f.run("money-plain-change", true).await;
    let claimed = f.runtime.store.claim_for_worker().await.unwrap().unwrap();
    let token = claimed.lease.as_ref().unwrap().token.clone();
    let prepared = cowork_runtime::provider::prepare(
        &claimed.record.spec,
        &claimed.checkpoint.messages,
        false,
    )
    .unwrap();
    let result = f.runtime.store.mutate(&run.id, Some(&token), |r| {
        reserve_model(&mut r.record, prepared.logical_fingerprint(), "forged-money", cowork_storage::now())?;
        Ok(Change::event("monetary_model_reserved", json!({"approved":true,"prepared_request":{"format":1,"wire_sha256":prepared.wire_sha256()}})))
    }).await;
    assert!(matches!(result, Err(Error::Forbidden)));
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
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .model_attempts
            .is_empty()
    );
    f.close().await;
}

#[tokio::test]
async fn actual_eof_and_timeout_after_received_overrun_retain_cost_without_answer_or_tools() {
    for usage_case in [5, 6] {
        let mut f = Fixture::setup(
            100000,
            true,
            false,
            true,
            MonetaryTokenMode::ConservativeContextUpperBound,
        )
        .await;
        f.server.usage_case.store(usage_case, Ordering::SeqCst);
        let (run, input) = f
            .run_seconds(
                "receive-overrun",
                true,
                if usage_case == 6 { 1 } else { 30 },
            )
            .await;
        f.serve(1);
        tokio::time::timeout(Duration::from_secs(10), async {
            while f.server.seen.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            f.server.seen.lock().unwrap().as_slice(),
            std::slice::from_ref(&run.id)
        );
        let (other, _) = f.run("blocked-after-receive-overrun", true).await;
        f.server.gate.add_permits(1);
        f.terminal(&run.id).await;
        assert_eq!(
            f.terminal(&other.id).await.run.error_code.as_deref(),
            Some("budget_exceeded")
        );
        f.stop_workers().await;
        let ledger = f.runtime.store.monetary_ledger().await.unwrap();
        let reservation = ledger.reservations().values().next().unwrap();
        assert!(
            matches!(&reservation.state, ReservationState::Uncertain { evidence: Some(usage), .. } if usage.input_tokens.is_none() && usage.output_tokens == Some(100000) && usage.cached_input_tokens.is_none())
        );
        assert!(reservation.overrun());
        assert_eq!(reservation.charged_units().unwrap(), 100000);
        assert_eq!(
            ledger
                .charged_to(&PoolScope::Installation("private-installation".into()))
                .unwrap(),
            100000
        );
        let detail = f.runtime.store.detail(&run.id).await.unwrap();
        assert!(detail.mcp_attempts.is_empty() && detail.process_attempts.is_empty());
        let stored = f.runtime.store.stored_run(&run.id).await.unwrap();
        assert!(stored.checkpoint.pending_tools.is_empty());
        assert!(
            !stored
                .checkpoint
                .messages
                .iter()
                .any(|value| value["content"] == "actual monetary answer")
        );
        assert!(
            !f.runtime
                .store
                .events(&run.id, 0)
                .await
                .unwrap()
                .iter()
                .any(|event| event.kind == "model_completed"
                    || event.kind == "monetary_model_response")
        );
        assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
        f.runtime.store.close().await;
        let reopened = Store::sqlite(&f.temp.path().join("money.db"))
            .await
            .unwrap();
        assert_eq!(reopened.monetary_ledger().await.unwrap(), ledger);
        assert!(reopened.resume(&run.id).await.is_err());
        let runtime = Runtime::new(
            reopened,
            vec![ConfiguredProvider {
                profile: f.profile.clone(),
                credential_env: None,
            }],
            false,
        )
        .unwrap();
        assert_eq!(runtime.create_run(&input).await.unwrap().id, run.id);
        assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
        runtime.store.close().await;
        f.close().await;
    }
}

#[tokio::test]
async fn copied_interrupted_receive_receipt_cannot_charge_or_release_a_different_attempt() {
    let mut f = Fixture::setup(
        HELD * 2,
        false,
        true,
        true,
        MonetaryTokenMode::ConservativeContextUpperBound,
    )
    .await;
    let (first, _) = f.run("same-receive-body", true).await;
    let claimed = f.runtime.store.claim_for_worker().await.unwrap().unwrap();
    let lease = claimed.lease.as_ref().unwrap().token.clone();
    let prepared = cowork_runtime::provider::prepare(
        &claimed.record.spec,
        &claimed.checkpoint.messages,
        false,
    )
    .unwrap();
    let logical = prepared.logical_fingerprint().to_owned();
    let wire = prepared.wire_sha256().to_owned();
    f.runtime
        .store
        .admit_prepared_model_attempt(&first.id, &lease, "same-turn", &logical, &wire, |r| {
            reserve_model(&mut r.record, &logical, "same-turn", cowork_storage::now())?;
            Ok(Change::event("model_started", json!({})))
        })
        .await
        .unwrap();
    let scope = f
        .runtime
        .store
        .monetary_dispatch_scope(&first.id, &lease, "same-turn")
        .await
        .unwrap();
    let observer = cowork_runtime::provider::ProviderReceiveObserver::new(scope);
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    assert!(
        cowork_runtime::provider::complete_prepared_observed(prepared, None, tx, observer.clone())
            .await
            .is_err()
    );
    let receipt = observer.interrupted_receipt().unwrap().unwrap();
    f.runtime
        .store
        .note_received_model_uncertainty(&first.id, "same-turn", &receipt)
        .await
        .unwrap();
    let (second, _) = f.run("same-receive-body", true).await;
    let claimed = f.runtime.store.claim_for_worker().await.unwrap().unwrap();
    assert_eq!(claimed.record.id, second.id);
    let lease = claimed.lease.as_ref().unwrap().token.clone();
    let prepared = cowork_runtime::provider::prepare(
        &claimed.record.spec,
        &claimed.checkpoint.messages,
        false,
    )
    .unwrap();
    assert_eq!(prepared.wire_sha256(), wire);
    assert_eq!(prepared.logical_fingerprint(), logical);
    f.runtime
        .store
        .admit_prepared_model_attempt(&second.id, &lease, "same-turn", &logical, &wire, |r| {
            reserve_model(&mut r.record, &logical, "same-turn", cowork_storage::now())?;
            Ok(Change::event("model_started", json!({})))
        })
        .await
        .unwrap();
    let before = f.runtime.store.monetary_ledger().await.unwrap();
    let history = canonical_hash(&f.runtime.store.events(&second.id, 0).await.unwrap());
    assert!(matches!(
        f.runtime
            .store
            .note_received_model_uncertainty(&second.id, "same-turn", &receipt.clone())
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(f.runtime.store.monetary_ledger().await.unwrap(), before);
    assert_eq!(
        canonical_hash(&f.runtime.store.events(&second.id, 0).await.unwrap()),
        history
    );
    let first_lease = f
        .runtime
        .store
        .stored_run(&first.id)
        .await
        .unwrap()
        .lease
        .unwrap()
        .token;
    f.runtime
        .store
        .mutate(&first.id, Some(&first_lease), |stored| {
            stored.lease.as_mut().unwrap().until = cowork_storage::now() - 1;
            Ok(Change::event("private_lease_expiry", json!({})))
        })
        .await
        .unwrap();
    let after_expiry = f.runtime.store.monetary_ledger().await.unwrap();
    f.runtime
        .store
        .note_received_model_uncertainty(&first.id, "same-turn", &receipt)
        .await
        .unwrap();
    assert_eq!(
        f.runtime.store.monetary_ledger().await.unwrap(),
        after_expiry,
        "original accepted receipt replay retains its original ledger revision after a sibling admission and lease expiry"
    );
    assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn received_overrun_survives_user_cancellation_lease_loss_and_delta_journal_failure() {
    for control in 0..3 {
        let mut f = Fixture::setup(
            100000,
            true,
            false,
            true,
            MonetaryTokenMode::ConservativeContextUpperBound,
        )
        .await;
        f.server.usage_case.store(6, Ordering::SeqCst);
        let (run, _) = f.run("controlled-receive-overrun", true).await;
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(f.temp.path().join("money.db")),
        )
        .await
        .unwrap();
        if control == 2 {
            sqlx::query("CREATE TRIGGER reject_received_delta BEFORE INSERT ON events WHEN json_extract(NEW.payload,'$.kind')='model_delta' BEGIN SELECT RAISE(ABORT,'controlled received delta journal failure'); END").execute(&pool).await.unwrap();
        }
        f.serve(1);
        tokio::time::timeout(Duration::from_secs(10), async {
            while f.server.seen.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let (other, _) = f.run("blocked-after-controlled-overrun", true).await;
        f.server.gate.add_permits(1);
        if control != 2 {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if f.runtime
                        .store
                        .events(&run.id, 0)
                        .await
                        .unwrap()
                        .iter()
                        .any(|event| event.kind == "model_delta")
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            if control == 0 {
                f.runtime.store.cancel(&run.id).await.unwrap();
            } else {
                let lease = f
                    .runtime
                    .store
                    .stored_run(&run.id)
                    .await
                    .unwrap()
                    .lease
                    .unwrap()
                    .token;
                f.runtime
                    .store
                    .mutate(&run.id, Some(&lease), |stored| {
                        stored.lease.as_mut().unwrap().until = cowork_storage::now() - 1;
                        Ok(Change::event("private_lease_loss", json!({})))
                    })
                    .await
                    .unwrap();
            }
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let ledger = f.runtime.store.monetary_ledger().await.unwrap();
                if ledger
                    .charged_to(&PoolScope::Installation("private-installation".into()))
                    .unwrap()
                    == 100000
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            f.terminal(&other.id).await.run.error_code.as_deref(),
            Some("budget_exceeded")
        );
        f.stop_workers().await;
        let ledger = f.runtime.store.monetary_ledger().await.unwrap();
        let reservation = ledger.reservations().values().next().unwrap();
        assert!(matches!(
            reservation.state,
            ReservationState::Uncertain {
                evidence: Some(_),
                ..
            }
        ));
        assert!(reservation.overrun());
        assert_eq!(reservation.charged_units().unwrap(), 100000);
        let detail = f.runtime.store.detail(&run.id).await.unwrap();
        assert!(detail.mcp_attempts.is_empty() && detail.process_attempts.is_empty());
        assert!(
            !f.runtime
                .store
                .events(&run.id, 0)
                .await
                .unwrap()
                .iter()
                .any(|event| event.kind == "model_completed"
                    || event.kind == "monetary_model_response")
        );
        assert_eq!(f.server.calls.load(Ordering::SeqCst), 1);
        pool.close().await;
        f.runtime.store.close().await;
        let reopened = Store::sqlite(&f.temp.path().join("money.db"))
            .await
            .unwrap();
        assert_eq!(reopened.monetary_ledger().await.unwrap(), ledger);
        reopened.close().await;
        f.close().await;
    }
}
