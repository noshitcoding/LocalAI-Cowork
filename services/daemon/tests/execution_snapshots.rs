use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn workflow_input_snapshot_is_owner_only_lossless_read_and_requires_plan_v2_for_reuse() {
    let f = fixture().await;
    let definition: cowork_workflow_contracts::WorkflowDefinition = serde_json::from_str(
        include_str!("../../../tests/fixtures/workflow-control.json"),
    )
    .unwrap();
    let library = f
        .runtime
        .store
        .create_workflow(&f.order.project_id, "input-source-http", &definition)
        .await
        .unwrap();
    let workflow = json!({"kind":"workflow","order_id":f.order.id,"base_revision":f.order.revision,"idempotency_key":"native-input-source-http",
        "prompt":"Keep the exact native amount","workflow_id":library.id,"version_id":library.versions[0].id,"budgets":definition.budgets,
        "inputs":{"amount":9007199254740993_u64},"folders":f.input["request"]["folders"]});
    let mut input = f.input.clone();
    input["request"] = workflow.clone();
    let prepared = f
        .runtime
        .prepare_execution_plan_v2(&serde_json::from_value(input).unwrap())
        .await
        .unwrap();
    let run = f
        .runtime
        .commit_execution_plan_v2(
            &f.order.id,
            &prepared.plan.id,
            &cowork_workflow_contracts::CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap();
    let path = format!("/api/v2/runs/{}/workflow/input-snapshot", run.id);
    assert_eq!(
        call(&f.app, "GET", &path, Value::Null, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, snapshot) = call(&f.app, "GET", &path, Value::Null, true).await;
    assert_eq!(status, StatusCode::OK, "{snapshot}");
    assert_eq!(snapshot["canonical_json"], "{\"amount\":9007199254740993}");
    assert_eq!(snapshot["source"]["run_id"], run.id);
    assert_eq!(snapshot["project_id"], f.order.project_id);
    assert_eq!(
        snapshot["source"]["sha256"],
        cowork_domain::hash(snapshot["canonical_json"].as_str().unwrap().as_bytes())
    );
    assert_eq!(
        call(
            &f.app,
            "GET",
            "/api/v2/runs/missing/workflow/input-snapshot",
            Value::Null,
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let mut replay = workflow;
    replay.as_object_mut().unwrap().remove("inputs");
    replay["input_source"] = snapshot["source"].clone();
    assert_eq!(
        call(&f.app, "POST", "/api/v2/preflight", replay, true)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(f.runtime.store.runs(&f.order.id).await.unwrap().len(), 1);
    f.runtime.store.cancel(&run.id).await.unwrap();
    f.runtime.store.close().await;
}

#[tokio::test]
async fn restore_plan_routes_require_owner_and_retirement_needs_no_model_or_workload() {
    let f = fixture().await;
    let request = json!({"order_id":f.order.id,"base_revision":f.order.revision,"idempotency_key":"restore-http",
        "prompt":"Restore into a confirmed copy","access_mode":"confirm_mutations",
        "restore":{"folder_id":"source","path":"restored.txt","source":{"run_id":"missing-source","tool_call_id":"write","version":"text_after"}}});
    let full =
        json!({"target":{"kind":"broker_controller"},"request":request,"input":f.input["input"]});
    let legacy = call(
        &f.app,
        "POST",
        "/api/v2/file-restorations",
        request.clone(),
        true,
    )
    .await;
    assert_eq!(legacy.0, StatusCode::CONFLICT);
    assert_eq!(legacy.1["code"], "execution_plan_required");
    for (path, body) in [
        ("/api/v2/execution-plans/v2/file-restorations", full.clone()),
        (
            "/api/v2/execution-plans/v2/file-restorations/retire-request",
            full.clone(),
        ),
        (
            "/api/v2/execution-plans/v2/file-restorations/retire-intent",
            request.clone(),
        ),
    ] {
        assert_eq!(
            call(&f.app, "POST", path, body, false).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let mut forged = full.clone();
    forged["actor"] = json!({"kind":"installation_owner"});
    assert_eq!(
        call(
            &f.app,
            "POST",
            "/api/v2/execution-plans/v2/file-restorations",
            forged,
            true
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let retired = call(
        &f.app,
        "POST",
        "/api/v2/execution-plans/v2/file-restorations/retire-intent",
        request.clone(),
        true,
    )
    .await;
    assert_eq!(retired.0, StatusCode::OK, "{}", retired.1);
    assert_eq!(retired.1["retired"], true);
    let mut changed = request;
    changed["restore"]["path"] = json!("different.txt");
    assert_eq!(
        call(
            &f.app,
            "POST",
            "/api/v2/execution-plans/v2/file-restorations/retire-intent",
            changed,
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
    f.runtime.store.close().await;
}

#[tokio::test]
async fn import_plan_routes_require_owner_and_retire_original_intent_without_dispatch() {
    let f = fixture().await;
    let request = json!({"order_id":f.order.id,"base_revision":f.order.revision,"idempotency_key":"import-http",
        "prompt":"Import into a confirmed copy","access_mode":"confirm_mutations",
        "import":{"folder_id":format!("workspace:{}",f.order.id),"path":"imported.bin","upload_id":"not-yet-available",
            "file":{"file_name":"imported.bin","size":4,"sha256":"0".repeat(64)},"expected_sha256":null}});
    let full =
        json!({"target":{"kind":"broker_controller"},"request":request,"input":f.input["input"]});
    let legacy = call(
        &f.app,
        "POST",
        "/api/v2/file-imports",
        request.clone(),
        true,
    )
    .await;
    assert_eq!(legacy.0, StatusCode::CONFLICT);
    assert_eq!(legacy.1["code"], "execution_plan_required");
    for (path, body) in [
        ("/api/v2/execution-plans/v2/file-imports", full.clone()),
        (
            "/api/v2/execution-plans/v2/file-imports/retire-request",
            full.clone(),
        ),
        (
            "/api/v2/execution-plans/v2/file-imports/retire-intent",
            request.clone(),
        ),
    ] {
        assert_eq!(
            call(&f.app, "POST", path, body, false).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let mut forged = full.clone();
    forged["actor"] = json!({"kind":"installation_owner"});
    assert_eq!(
        call(
            &f.app,
            "POST",
            "/api/v2/execution-plans/v2/file-imports",
            forged,
            true
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let retired = call(
        &f.app,
        "POST",
        "/api/v2/execution-plans/v2/file-imports/retire-intent",
        request.clone(),
        true,
    )
    .await;
    assert_eq!(retired.0, StatusCode::OK, "{}", retired.1);
    assert_eq!(retired.1["retired"], true);
    let mut changed = request;
    changed["import"]["path"] = json!("different.bin");
    assert_eq!(
        call(
            &f.app,
            "POST",
            "/api/v2/execution-plans/v2/file-imports/retire-intent",
            changed,
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
    f.runtime.store.close().await;
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
    owner: bool,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if owner {
        request = request.header("authorization", "Bearer snapshot-owner");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 3 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
struct Fixture {
    app: axum::Router,
    runtime: Runtime,
    input: Value,
    order: Order,
    _directory: tempfile::TempDir,
}
async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("in.txt"), "frozen input").unwrap();
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Snapshot API".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Reviewed snapshot".into(),
        })
        .await
        .unwrap();
    let profile:ProviderProfile=serde_json::from_value(json!({"id":"fixture","name":"Fixture","kind":"test","endpoint":"","model":"fixture","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true})).unwrap();
    store.insert_provider(&profile).await.unwrap();
    let broker = cowork_broker::FileBroker::open(
        &directory.path().join("broker"),
        vec![cowork_broker::FolderConfig {
            id: "source".into(),
            project_id: project.id.clone(),
            name: "Selected source".into(),
            path: source,
            writable: true,
        }],
    )
    .unwrap();
    let folders = broker.grants(&project.id).unwrap();
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
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new("snapshot-owner".into()),
            native_token: None,
            origins: Arc::new(vec!["http://127.0.0.1:5175".into()]),
        },
        None,
    );
    let capture = call(
        &app,
        "POST",
        &format!("/api/v2/orders/{}/execution-inputs", order.id),
        json!({"idempotency_key":"freeze","base_revision":order.revision,"folder_ids":["source"]}),
        true,
    )
    .await;
    assert_eq!(capture.0, StatusCode::OK, "{}", capture.1);
    let input = json!({"target":{"kind":"broker_controller"},"request":{"order_id":order.id,"base_revision":order.revision,"idempotency_key":"confirmed-copy","prompt":"Work on the frozen copy",
        "provider_id":"fixture","provider_revision":1,"access_mode":"confirm_mutations","budgets":Budgets::default(),"folders":folders},
        "input":{"snapshot_id":capture.1["snapshot"]["id"],"sha256":capture.1["sha256"],"manifest_sha256":capture.1["snapshot"]["manifest"]["sha256"]}});
    Fixture {
        app,
        runtime,
        input,
        order,
        _directory: directory,
    }
}

#[tokio::test]
async fn snapshot_plan_routes_bind_owner_confirmation_and_keep_original_stop_receipts() {
    let f = fixture().await;
    let path = "/api/v2/execution-plans/v2";
    assert_eq!(
        call(&f.app, "POST", path, f.input.clone(), false).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
    let (status, plan) = call(&f.app, "POST", path, f.input.clone(), true).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["plan"]["format_version"], 2);
    assert_eq!(plan["plan"]["actor"]["kind"], "installation_owner");
    assert_eq!(plan["plan"]["boundary"]["input_snapshot_bound"], true);
    assert_eq!(
        plan["plan"]["input"]["manifest"]["files"][0]["path"],
        "input-0001/in.txt"
    );
    assert!(!plan.to_string().contains("container_identity"));
    assert!(!plan.to_string().contains("nonce"));
    let id = plan["plan"]["id"].as_str().unwrap();
    let endpoint = format!("/api/v2/orders/{}/execution-plans/v2/{id}", f.order.id);
    assert_eq!(
        call(&f.app, "GET", &endpoint, Value::Null, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&f.app, "GET", &endpoint, Value::Null, true).await.1,
        plan
    );
    let commit = format!("{endpoint}/commit");
    let hash = json!({"sha256":plan["sha256"]});
    assert_eq!(
        call(&f.app, "POST", &commit, hash.clone(), false).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
    let (status, run) = call(&f.app, "POST", &commit, hash.clone(), true).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(run["spec"]["input_workspace"], plan["plan"]["workspace"]);
    let copy = format!(
        "/api/v2/orders/{}/runs/{}/working-copy/text?folder_id=source&path=in.txt",
        f.order.id,
        run["id"].as_str().unwrap()
    );
    assert_eq!(
        call(&f.app, "GET", &copy, Value::Null, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    let read = call(&f.app, "GET", &copy, Value::Null, true).await;
    assert_eq!(read.0, StatusCode::OK, "{}", read.1);
    assert_eq!(read.1["content"], "frozen input");
    let foreign = copy.replace(&format!("/orders/{}/", f.order.id), "/orders/foreign/");
    assert!(
        call(&f.app, "GET", &foreign, Value::Null, true)
            .await
            .0
            .is_client_error()
    );
    assert!(
        call(
            &f.app,
            "GET",
            &copy.replace("folder_id=source", "folder_id=other"),
            Value::Null,
            true
        )
        .await
        .0
        .is_client_error()
    );
    let stopped = call(
        &f.app,
        "POST",
        &format!("/api/v2/runs/{}/cancel", run["id"].as_str().unwrap()),
        json!({}),
        true,
    )
    .await;
    assert_eq!(stopped.0, StatusCode::OK);
    assert_eq!(stopped.1["state"], "canceled");
    f.runtime.providers.write().await.clear();
    assert_eq!(call(&f.app, "POST", &commit, hash, true).await.1, run);
    assert_eq!(f.runtime.store.runs(&f.order.id).await.unwrap().len(), 1);
    f.runtime.store.close().await;
}

#[tokio::test]
async fn extra_authority_and_lost_prepare_retirement_never_create_a_run() {
    let f = fixture().await;
    for (key, value) in [
        ("actor", json!({"kind":"installation_owner"})),
        ("controller", json!({"id":"other","generation":1})),
        ("workspace", json!({"id":"other"})),
    ] {
        let mut input = f.input.clone();
        input[key] = value;
        let response = call(&f.app, "POST", "/api/v2/execution-plans/v2", input, true).await;
        assert!(response.0.is_client_error(), "{key}: {}", response.1);
    }
    let retired = call(
        &f.app,
        "POST",
        "/api/v2/execution-plans/v2/retire-request",
        f.input.clone(),
        true,
    )
    .await;
    assert_eq!(retired.0, StatusCode::OK);
    assert_eq!(retired.1["retired"], true);
    let late = call(
        &f.app,
        "POST",
        "/api/v2/execution-plans/v2",
        f.input.clone(),
        true,
    )
    .await;
    assert_eq!(late.0, StatusCode::CONFLICT);
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
    f.runtime.store.close().await;
}
