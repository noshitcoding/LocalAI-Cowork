//! Real storage/broker/worker paths; HTTP model fixtures are deliberately local
//! and synthetic and do not qualify an actual model provider or process runner.
use cowork_broker::{FileBroker, FolderConfig};
use cowork_contracts::*;
use cowork_domain::{Error, canonical_hash, hash};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use cowork_workflow_contracts::{
    CommitExecutionPlan, OwnerExecutionTargetV2, PrepareOwnerExecutionPlanV2,
    PreparedExecutionPlanV2, RunRequest,
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
#[path = "execution_snapshots/import.rs"]
mod import;
#[path = "execution_snapshots/restore.rs"]
mod restore;
#[path = "execution_snapshots/workflow_inputs.rs"]
mod workflow_inputs;

struct Fixture {
    runtime: Runtime,
    input: PrepareOwnerExecutionPlanV2,
    pool: sqlx::AnyPool,
    dir: tempfile::TempDir,
}
impl Fixture {
    async fn new(endpoint: Option<String>) -> Self {
        Self::database(endpoint, None).await
    }
    async fn database(endpoint: Option<String>, database: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("originals");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("note.txt"), "before").unwrap();
        let url = database.map(str::to_owned).unwrap_or_else(|| {
            format!(
                "sqlite:{}?mode=rwc",
                dir.path().join("snapshot.sqlite").display()
            )
        });
        let store = Store::connect(&url).await.unwrap();
        let pool = sqlx::AnyPool::connect(&url).await.unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Snapshot execution".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Copied input".into(),
            })
            .await
            .unwrap();
        let broker = FileBroker::open(
            &dir.path().join("broker"),
            vec![FolderConfig {
                id: "source".into(),
                project_id: project.id.clone(),
                name: "Selected source".into(),
                path,
                writable: true,
            }],
        )
        .unwrap();
        let folders = broker.grants(&project.id).unwrap();
        let profile:ProviderProfile=serde_json::from_value(json!({"id":"fixture","name":"Fixture","kind":if endpoint.is_some(){"openai_compatible"}else{"test"},
            "endpoint":endpoint.unwrap_or_default(),"model":"fixture","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true})).unwrap();
        store.insert_provider(&profile).await.unwrap();
        let runtime = Runtime::new(
            store,
            vec![ConfiguredProvider {
                profile,
                credential_env: None,
            }],
            true,
        )
        .unwrap()
        .with_file_broker(broker);
        let request:CreateRun=serde_json::from_value(json!({"order_id":order.id,"base_revision":order.revision,"idempotency_key":"snapshot-run","prompt":"Edit the selected working copy",
            "provider_id":"fixture","provider_revision":1,"access_mode":"confirm_mutations","budgets":Budgets::default(),"folders":folders})).unwrap();
        let frozen = runtime
            .prepare_execution_input(
                &order.id,
                &PrepareExecutionInput {
                    idempotency_key: "freeze-input".into(),
                    base_revision: order.revision,
                    folder_ids: vec!["source".into()],
                },
            )
            .await
            .unwrap();
        let input = PrepareOwnerExecutionPlanV2 {
            target: OwnerExecutionTargetV2::BrokerController {},
            request: RunRequest::Single(request),
            input: ExecutionInputRef {
                snapshot_id: frozen.snapshot.id,
                sha256: frozen.sha256,
                manifest_sha256: frozen.snapshot.manifest.sha256,
            },
        };
        Self {
            dir,
            runtime,
            input,
            pool,
        }
    }
    fn order(&self) -> &str {
        match &self.input.request {
            RunRequest::Single(r) => &r.order_id,
            RunRequest::Workflow(r) => &r.order_id,
        }
    }
    async fn prepare(&self) -> PreparedExecutionPlanV2 {
        self.runtime
            .prepare_execution_plan_v2(&self.input)
            .await
            .unwrap()
    }
    async fn commit(&self, p: &PreparedExecutionPlanV2) -> cowork_domain::Result<RunRecord> {
        self.runtime
            .commit_execution_plan_v2(
                self.order(),
                &p.plan.id,
                &CommitExecutionPlan {
                    sha256: p.sha256.clone(),
                },
            )
            .await
    }
    async fn close(self) {
        let Self {
            runtime, pool, dir, ..
        } = self;
        pool.close().await;
        runtime.store.close().await;
        drop(pool);
        drop(runtime);
        dir.close()
            .expect("private test state must be physically removable after its handles close");
    }
}

#[tokio::test]
#[ignore = "Requires a fresh private cowork_execution_plan_<uuid> PostgreSQL database"]
async fn postgres_execution_snapshots_serialize_confirmation_and_retirement() {
    let url = std::env::var("COWORK_TEST_EXECUTION_PLAN_POSTGRES")
        .expect("private PostgreSQL fixture URL");
    sqlx::any::install_default_drivers();
    let pool = sqlx::AnyPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database.strip_prefix("cowork_execution_plan_").is_some_and(
            |suffix| suffix.len() == 32 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
        )
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pg_tables WHERE schemaname='public'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    pool.close().await;
    let f = Fixture::database(None, Some(&url)).await;
    let plan = f.prepare().await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let runtime = f.runtime.clone();
        let order = f.order().to_owned();
        let plan = plan.clone();
        tasks.spawn(async move {
            runtime
                .commit_execution_plan_v2(
                    &order,
                    &plan.plan.id,
                    &CommitExecutionPlan {
                        sha256: plan.sha256,
                    },
                )
                .await
        });
    }
    let mut original = None;
    while let Some(result) = tasks.join_next().await {
        let run = result.unwrap().unwrap();
        if let Some(ref old) = original {
            assert_eq!(serde_json::to_value(&run).unwrap(), *old)
        } else {
            original = Some(serde_json::to_value(run).unwrap())
        }
    }
    let original = original.unwrap();
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    f.runtime
        .store
        .cancel(original["id"].as_str().unwrap())
        .await
        .unwrap();
    let RunRequest::Single(template) = &f.input.request else {
        unreachable!()
    };
    for index in 0..12 {
        let order = f
            .runtime
            .store
            .create_order(CreateOrder {
                project_id: plan.plan.project_id.clone(),
                title: format!("Snapshot retirement race {index}"),
            })
            .await
            .unwrap();
        let input = f
            .runtime
            .prepare_execution_input(
                &order.id,
                &PrepareExecutionInput {
                    idempotency_key: "freeze".into(),
                    base_revision: order.revision,
                    folder_ids: vec!["source".into()],
                },
            )
            .await
            .unwrap();
        let request = PrepareOwnerExecutionPlanV2 {
            target: OwnerExecutionTargetV2::BrokerController {},
            request: RunRequest::Single(CreateRun {
                order_id: order.id.clone(),
                base_revision: order.revision,
                idempotency_key: "race".into(),
                ..template.clone()
            }),
            input: ExecutionInputRef {
                snapshot_id: input.snapshot.id,
                sha256: input.sha256,
                manifest_sha256: input.snapshot.manifest.sha256,
            },
        };
        let plan = f.runtime.prepare_execution_plan_v2(&request).await.unwrap();
        let confirm = CommitExecutionPlan {
            sha256: plan.sha256.clone(),
        };
        let (accepted, retired) = tokio::join!(
            f.runtime
                .commit_execution_plan_v2(&order.id, &plan.plan.id, &confirm),
            f.runtime.retire_execution_request_v2(&request)
        );
        match (accepted, retired) {
            (Ok(run), Err(Error::Conflict)) => {
                assert_eq!(f.runtime.store.runs(&order.id).await.unwrap().len(), 1);
                f.runtime.store.cancel(&run.id).await.unwrap();
            }
            (Err(_), Ok(receipt)) => {
                assert!(receipt.retired);
                assert!(f.runtime.store.runs(&order.id).await.unwrap().is_empty());
            }
            (accepted, retired) => panic!(
                "PostgreSQL snapshot retirement diverged: accepted={},retired={}",
                accepted.is_ok(),
                retired.is_ok()
            ),
        }
    }
    f.runtime.providers.write().await.clear();
    assert_eq!(
        serde_json::to_value(f.commit(&plan).await.unwrap()).unwrap(),
        original
    );
    let reopened = Runtime::new(Store::connect(&url).await.unwrap(), vec![], false).unwrap();
    assert_eq!(
        serde_json::to_value(
            reopened
                .commit_execution_plan_v2(
                    f.order(),
                    &plan.plan.id,
                    &CommitExecutionPlan {
                        sha256: plan.sha256
                    }
                )
                .await
                .unwrap()
        )
        .unwrap(),
        original
    );
    reopened.store.close().await;
    f.close().await;
}

#[tokio::test]
async fn review_freezes_public_input_and_creates_no_run_or_private_working_copy() {
    let f = Fixture::new(None).await;
    let plan = f.prepare().await;
    assert_eq!(plan.plan.format_version, 2);
    assert!(plan.plan.boundary.input_snapshot_bound);
    assert_eq!(plan.sha256, canonical_hash(&plan.plan));
    assert_eq!(plan.plan.request_sha256, canonical_hash(&f.input));
    assert_eq!(plan.plan.input.manifest.files[0].sha256, hash(b"before"));
    assert!(f.runtime.store.runs(f.order()).await.unwrap().is_empty());
    assert!(!f.dir.path().join("broker/run-workspaces").exists());
    let native = match &f.input.request {
        RunRequest::Single(r) => &r.folders[0],
        _ => unreachable!(),
    };
    let bytes = serde_json::to_string(&plan).unwrap();
    assert!(!bytes.contains(&native.resource_id));
    assert!(!bytes.contains(&native.revision));
    assert_eq!(f.prepare().await.sha256, plan.sha256);
    f.close().await;
}

#[tokio::test]
async fn concurrent_confirmation_returns_one_original_ack_and_replay_needs_no_current_provider() {
    let f = Fixture::new(None).await;
    let plan = f.prepare().await;
    let mut starts = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let r = f.runtime.clone();
        let p = plan.clone();
        let order = f.order().to_string();
        starts.spawn(async move {
            r.commit_execution_plan_v2(
                &order,
                &p.plan.id,
                &CommitExecutionPlan { sha256: p.sha256 },
            )
            .await
        });
    }
    let mut original = None;
    while let Some(result) = starts.join_next().await {
        let ack = serde_json::to_value(result.unwrap().unwrap()).unwrap();
        if let Some(v) = &original {
            assert_eq!(&ack, v)
        } else {
            original = Some(ack)
        }
    }
    let original = original.unwrap();
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    f.runtime
        .store
        .cancel(original["id"].as_str().unwrap())
        .await
        .unwrap();
    f.runtime
        .retire_execution_input(
            f.order(),
            &f.input.input.snapshot_id,
            &RetireExecutionInput {
                sha256: f.input.input.sha256.clone(),
            },
        )
        .await
        .unwrap();
    f.runtime.providers.write().await.clear();
    assert_eq!(
        serde_json::to_value(f.commit(&plan).await.unwrap()).unwrap(),
        original
    );
    let store = Store::sqlite(&f.dir.path().join("snapshot.sqlite"))
        .await
        .unwrap();
    let reopened = Runtime::new(store, vec![], false).unwrap();
    let replay = reopened
        .commit_execution_plan_v2(
            f.order(),
            &plan.plan.id,
            &CommitExecutionPlan {
                sha256: plan.sha256,
            },
        )
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(replay).unwrap(), original);
    reopened.store.close().await;
    f.close().await;
}

#[tokio::test]
async fn retirement_of_a_lost_prepare_ack_fences_the_original_request() {
    let f = Fixture::new(None).await;
    f.runtime
        .retire_execution_request_v2(&f.input)
        .await
        .unwrap();
    assert!(matches!(
        f.runtime.prepare_execution_plan_v2(&f.input).await,
        Err(Error::Conflict)
    ));
    assert!(f.runtime.store.runs(f.order()).await.unwrap().is_empty());
    f.close().await;
    let f = Fixture::new(None).await;
    let p = f.prepare().await;
    let retired = f
        .runtime
        .retire_execution_request_v2(&f.input)
        .await
        .unwrap();
    assert!(retired.retired);
    assert!(f.commit(&p).await.is_err());
    assert!(f.prepare().await.retired);
    f.close().await;
}

#[tokio::test]
async fn stale_input_controller_foreign_order_and_forged_hash_never_dispatch() {
    let f = Fixture::new(None).await;
    let p = f.prepare().await;
    assert!(
        f.runtime
            .commit_execution_plan_v2(
                "foreign",
                &p.plan.id,
                &CommitExecutionPlan {
                    sha256: p.sha256.clone()
                }
            )
            .await
            .is_err()
    );
    assert!(
        f.runtime
            .commit_execution_plan_v2(
                f.order(),
                &p.plan.id,
                &CommitExecutionPlan {
                    sha256: hash(b"forged")
                }
            )
            .await
            .is_err()
    );
    sqlx::query("UPDATE broker_controller SET generation=generation+1 WHERE singleton=1")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(matches!(
        f.commit(&p).await,
        Err(Error::ExecutionPlanChanged)
    ));
    assert!(f.runtime.store.runs(f.order()).await.unwrap().is_empty());
    f.close().await;
    let f = Fixture::new(None).await;
    let p = f.prepare().await;
    f.runtime
        .retire_execution_input(
            f.order(),
            &f.input.input.snapshot_id,
            &RetireExecutionInput {
                sha256: f.input.input.sha256.clone(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        f.commit(&p).await,
        Err(Error::ExecutionPlanChanged)
    ));
    assert!(!f.dir.path().join("broker/run-workspaces").exists());
    f.close().await;
}

#[tokio::test]
async fn controller_never_rebinds_a_different_native_origin_and_legacy_v1_cannot_claim_copies() {
    let f = Fixture::new(None).await;
    let p = f.prepare().await;
    assert!(matches!(
        f.runtime
            .store
            .broker_controller(&hash(b"different native controller"))
            .await,
        Err(Error::ExecutionPlanChanged)
    ));
    let request = match &f.input.request {
        RunRequest::Single(r) => RunRequest::Single(CreateRun {
            idempotency_key: "legacy".into(),
            ..r.clone()
        }),
        _ => unreachable!(),
    };
    let v1 = f.runtime.prepare_execution_plan(&request).await.unwrap();
    let mut private = f
        .runtime
        .store
        .execution_plan(f.order(), &v1.plan.id)
        .await
        .unwrap();
    private.spec.input_workspace = Some(p.plan.workspace);
    private.plan.spec_sha256 = canonical_hash(&private.spec);
    private.sha256 = canonical_hash(&private.plan);
    assert!(private.validate().is_err());
    f.close().await;
}

#[tokio::test]
async fn broker_target_rejects_requested_native_programs_without_weakening_the_destination() {
    let mut f = Fixture::new(None).await;
    let RunRequest::Single(request) = &mut f.input.request else {
        unreachable!()
    };
    request.native_tools = vec!["terminal".into()];
    assert!(
        matches!(f.runtime.prepare_execution_plan_v2(&f.input).await,Err(Error::Validation(reason)) if reason=="snapshot_process_or_connector_adapter_required")
    );
    assert!(f.runtime.store.runs(f.order()).await.unwrap().is_empty());
    assert!(!f.dir.path().join("broker/run-workspaces").exists());
    f.close().await;
}

#[derive(Clone, Default)]
struct ModelFixture {
    requests: Arc<Mutex<Vec<Value>>>,
}
async fn model_chat(
    axum::extract::State(f): axum::extract::State<ModelFixture>,
    axum::Json(body): axum::Json<Value>,
) -> String {
    let index = {
        let mut requests = f.requests.lock().unwrap();
        let index = requests.len();
        requests.push(body);
        index
    };
    let delta = match index {
        0 => {
            json!({"tool_calls":[{"index":0,"id":"read-frozen","function":{"name":"read_text_file","arguments":json!({"folder_id":"source","path":"note.txt"}).to_string()}}]})
        }
        1 => {
            json!({"tool_calls":[{"index":0,"id":"write-copy","function":{"name":"write_text_file","arguments":json!({"folder_id":"source","path":"note.txt","content":"after","expected_sha256":hash(b"before")}).to_string()}}]})
        }
        _ => json!({"content":"The working copy is ready for review."}),
    };
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"delta":delta,"finish_reason":if index<2{"tool_calls"}else{"stop"}}],"usage":{"prompt_tokens":200,"completion_tokens":20}})
    )
}
async fn model_server() -> (String, ModelFixture, tokio::task::JoinHandle<()>) {
    let fixture = ModelFixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .route(
            "/models",
            axum::routing::get(|| async { axum::Json(json!({"data":[{"id":"fixture"}]})) }),
        )
        .route("/chat/completions", axum::routing::post(model_chat))
        .with_state(fixture.clone());
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, fixture, handle)
}
async fn state(runtime: &Runtime, id: &str, wanted: RunState) {
    let outcome = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let detail = runtime.store.detail(id).await.unwrap();
            if detail.run.state == wanted {
                return;
            }
            if cowork_domain::terminal(&detail.run.state) {
                panic!(
                    "expected {wanted:?}, got {:?}: {:?}",
                    detail.run.state, detail.run.error_code
                )
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if outcome.is_err() {
        let detail = runtime.store.detail(id).await.unwrap();
        let events = runtime.store.events(id, 0).await.unwrap();
        let view = if detail.run.spec.workflow.is_some() {
            runtime
                .store
                .workflow_run_view(&detail.run.spec.project_id, id)
                .await
                .ok()
        } else {
            None
        };
        panic!(
            "snapshot execution deadline: wanted={wanted:?} state={:?} error={:?} view={} tail={}",
            detail.run.state,
            detail.run.error_code,
            serde_json::to_string(&view).unwrap(),
            serde_json::to_string(&events[events.len().saturating_sub(6)..]).unwrap()
        );
    }
}

#[tokio::test]
async fn actual_worker_reads_frozen_bytes_and_keeps_an_approved_edit_across_restart_without_touching_originals()
 {
    let (endpoint, model, server) = model_server().await;
    let f = Fixture::new(Some(endpoint)).await;
    let p = f.prepare().await;
    std::fs::write(f.dir.path().join("originals/note.txt"), "changed original").unwrap();
    let run = f.commit(&p).await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f.runtime, &run.id, RunState::WaitingApproval).await;
    assert_eq!(model.requests.lock().unwrap().len(), 2);
    assert!(
        model.requests.lock().unwrap()[1]["messages"]
            .to_string()
            .contains("before")
    );
    assert!(
        !model.requests.lock().unwrap()[1]["messages"]
            .to_string()
            .contains("changed original")
    );
    stop.cancel();
    worker.await.unwrap();
    let detail = f.runtime.store.detail(&run.id).await.unwrap();
    let approval = &detail.approvals[0];
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
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f.runtime, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(model.requests.lock().unwrap().len(), 3);
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("originals/note.txt")).unwrap(),
        "changed original"
    );
    let output = f
        .runtime
        .download_snapshot(
            f.order(),
            &run.id,
            DownloadFile {
                path: "input-0001/note.txt".into(),
                expected_sha256: Some(hash(b"after")),
            },
        )
        .await
        .unwrap();
    assert_eq!(output.bytes, b"after");
    let events = f.runtime.store.events(&run.id, 0).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "file_write_started")
            .count(),
        1
    );
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn a_controller_revision_change_before_worker_dispatch_sends_no_model_and_stop_still_works() {
    let (endpoint, model, server) = model_server().await;
    let f = Fixture::new(Some(endpoint)).await;
    let p = f.prepare().await;
    let run = f.commit(&p).await.unwrap();
    sqlx::query("UPDATE broker_controller SET generation=generation+1 WHERE singleton=1")
        .execute(&f.pool)
        .await
        .unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f.runtime, &run.id, RunState::Failed).await;
    stop.cancel();
    worker.await.unwrap();
    assert!(model.requests.lock().unwrap().is_empty());
    let replay = f.commit(&p).await.unwrap();
    assert_eq!(replay.id, run.id);
    assert_eq!(replay.state, RunState::Queued);
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn confirmation_and_retirement_have_one_serialized_outcome() {
    for _ in 0..8 {
        let f = Fixture::new(None).await;
        let plan = f.prepare().await;
        let (accepted, retired) = tokio::join!(
            f.commit(&plan),
            f.runtime.retire_execution_request_v2(&f.input)
        );
        match (accepted, retired) {
            (Ok(run), Err(Error::Conflict)) => {
                assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
                assert_eq!(f.commit(&plan).await.unwrap().id, run.id);
            }
            (Err(_), Ok(receipt)) => {
                assert!(receipt.retired);
                assert!(f.runtime.store.runs(f.order()).await.unwrap().is_empty());
                assert!(f.commit(&plan).await.is_err());
            }
            (accepted, retired) => panic!(
                "non-serialized plan retirement: accepted={},retired={}",
                accepted.is_ok(),
                retired.is_ok()
            ),
        }
        f.close().await;
    }
}

#[tokio::test]
async fn workflow_role_specs_keep_the_confirmed_copy_binding_and_file_rights() {
    use cowork_workflow_contracts::WorkflowRunRequest;
    let (endpoint, model, server) = model_server().await;
    let mut f = Fixture::new(Some(endpoint)).await;
    let RunRequest::Single(single) = &f.input.request else {
        unreachable!()
    };
    let single = single.clone();
    let rights = json!({"tools":["read_text_file","write_text_file"],"folders":["source"]});
    let definition=serde_json::from_value(json!({"format_version":1,"name":"Edit a snapshot","mode":{"mode":"sequential"},"failure":"stop","rights":rights,
        "budgets":single.budgets,"roles":[{"id":"editor","instructions":"Edit the copied source","provider_id":"fixture","provider_revision":1,
            "rights":rights,"budgets":single.budgets,"delegate_to":[]}],
        "steps":[{"id":"edit","role_id":"editor","goal":"Read then edit the selected copy","dependencies":[],"rights":rights,"budgets":single.budgets}]})).unwrap();
    let project = f.runtime.store.order(f.order()).await.unwrap().project_id;
    let library = f
        .runtime
        .store
        .create_workflow(&project, "snapshot-workflow", &definition)
        .await
        .unwrap();
    let request:WorkflowRunRequest=serde_json::from_value(json!({"kind":"workflow","order_id":single.order_id,"base_revision":single.base_revision,
        "idempotency_key":single.idempotency_key,"prompt":single.prompt,"workflow_id":library.id,"version_id":library.versions[0].id,
        "access_mode":"autonomous","budgets":single.budgets,"folders":single.folders})).unwrap();
    f.input.request = RunRequest::Workflow(request);
    let plan = f.prepare().await;
    let run = f.commit(&plan).await.unwrap();
    std::fs::write(f.dir.path().join("originals/note.txt"), "changed original").unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(2, stop.clone()));
    state(&f.runtime, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(model.requests.lock().unwrap().len(), 3);
    assert!(
        model.requests.lock().unwrap()[1]["messages"]
            .to_string()
            .contains("before")
    );
    let output = f
        .runtime
        .download_snapshot(
            f.order(),
            &run.id,
            DownloadFile {
                path: "input-0001/note.txt".into(),
                expected_sha256: Some(hash(b"after")),
            },
        )
        .await
        .unwrap();
    assert_eq!(output.bytes, b"after");
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("originals/note.txt")).unwrap(),
        "changed original"
    );
    server.abort();
    let _ = server.await;
    f.close().await;
}

/// Explicit opt-in: uses the already installed local model, synthetic private
/// files only, and never starts/stops the user's Ollama service or pulls models.
#[tokio::test]
#[ignore = "Requires an already running loopback Ollama with llama3.1:8b installed"]
async fn ollama_execution_snapshot_reads_frozen_input_and_retains_its_output() {
    assert_eq!(
        std::env::var("COWORK_TEST_LOCAL_OLLAMA").as_deref(),
        Ok("1"),
        "Explicit local model qualification is required"
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let tags: Value = client
        .get("http://127.0.0.1:11434/api/tags")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let model = "llama3.1:8b";
    let installed = tags["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == model)
        .expect("installed llama3.1:8b");
    let digest = installed["digest"].as_str().unwrap().to_owned();
    assert!(digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()));
    let mut f = Fixture::new(None).await;
    let profile: ProviderProfile = serde_json::from_value(json!({
        "id":"local-snapshot-qualification", "name":"Local snapshot qualification",
        "kind":"ollama", "endpoint":"http://127.0.0.1:11434", "model":model,
        "revision":1, "context_tokens":32768, "credential_required":false,
        "credential_ready":true, "generation":{"temperature":0.0,"timeout_ms":60000}
    }))
    .unwrap();
    f.runtime.store.insert_provider(&profile).await.unwrap();
    f.runtime.providers.write().await.push(ConfiguredProvider {
        profile,
        credential_env: None,
    });
    let RunRequest::Single(request) = &mut f.input.request else {
        unreachable!()
    };
    request.provider_id = "local-snapshot-qualification".into();
    request.access_mode = AccessMode::Autonomous;
    request.budgets = Budgets {
        max_steps: 8,
        max_seconds: 120,
        max_output_tokens: 1024,
        max_total_tokens: 32768,
    };
    request.agent_config = Some(EffectiveAgentConfig {
        tool_filter: Some(vec!["read_text_file".into(), "write_text_file".into()]),
        ..Default::default()
    });
    request.prompt = "Use the tools now: first read note.txt in folder source. Then create snapshot-proof.txt in the same folder with exactly the read text, no extra text or newline. Do not modify note.txt. Finally reply done. /no_think".into();
    let plan = f.prepare().await;
    std::fs::write(f.dir.path().join("originals/note.txt"), "changed original").unwrap();
    let run = f.commit(&plan).await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    let outcome = tokio::time::timeout(Duration::from_secs(135), async {
        loop {
            let detail = f.runtime.store.detail(&run.id).await.unwrap();
            if cowork_domain::terminal(&detail.run.state) {
                return detail;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    // Cleanup precedes assertions so even a provider failure leaves no worker.
    if outcome.is_err() {
        f.runtime.store.cancel(&run.id).await.unwrap();
    }
    stop.cancel();
    worker.await.unwrap();
    let detail = outcome.expect("local model qualification deadline");
    let events = f.runtime.store.events(&run.id, 0).await.unwrap();
    assert_eq!(
        detail.run.state,
        RunState::Completed,
        "{:?}: {}",
        detail.run.error_code,
        serde_json::to_string(&events[events.len().saturating_sub(8)..]).unwrap()
    );
    assert!(events.iter().any(|e| e.kind == "tool_completed" && e.payload.to_string().contains("read_text_file")));
    let output = f
        .runtime
        .download_snapshot(
            f.order(),
            &run.id,
            DownloadFile {
                path: "input-0001/snapshot-proof.txt".into(),
                expected_sha256: Some(hash(b"before")),
            },
        )
        .await
        .unwrap();
    assert_eq!(output.bytes, b"before");
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("originals/note.txt")).unwrap(),
        "changed original"
    );
    assert!(!f.dir.path().join("originals/snapshot-proof.txt").exists());
    assert!(detail.run.usage.input_tokens.is_some_and(|n| n > 0));
    assert!(detail.run.usage.output_tokens.is_some_and(|n| n > 0));
    assert!(!detail.run.usage.incomplete);
    println!(
        "{}",
        json!({"provider":"local_ollama", "model":model, "digest":digest,
        "plan_sha256":plan.sha256, "usage":detail.run.usage, "retained_output_sha256":hash(&output.bytes),
        "originals_unchanged_by_workload":true})
    );
    f.close().await;
}
