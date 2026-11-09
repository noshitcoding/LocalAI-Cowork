use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
    authenticated: bool,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json");
    if authenticated {
        request = request.header("Authorization", "Bearer private-input-owner");
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

#[tokio::test]
async fn input_preparation_is_authenticated_durable_local_and_has_no_byte_or_run_endpoint() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("proof.bin"), b"frozen").unwrap();
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Input API".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Local capture".into(),
        })
        .await
        .unwrap();
    let other = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Other order".into(),
        })
        .await
        .unwrap();
    let broker = cowork_broker::FileBroker::open(
        &directory.path().join("state"),
        vec![cowork_broker::FolderConfig {
            id: "selected".into(),
            project_id: project.id,
            name: "Selected folder".into(),
            path: source.clone(),
            writable: true,
        }],
    )
    .unwrap();
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_file_broker(broker);
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new("private-input-owner".into()),
            native_token: None,
            origins: Arc::new(vec!["http://127.0.0.1:5175".into()]),
        },
        None,
    );
    let path = format!("/api/v2/orders/{}/execution-inputs", order.id);
    let request = json!({"idempotency_key":"freeze-once","base_revision":order.revision,"folder_ids":["selected"]});
    assert_eq!(
        call(&app, "POST", &path, request.clone(), false).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(runtime.store.runs(&order.id).await.unwrap().is_empty());
    let first = runtime.begin_upload_transfer().unwrap();
    let second = runtime.begin_upload_transfer().unwrap();
    let busy = call(&app, "POST", &path, request.clone(), true).await;
    assert_eq!(busy.0, StatusCode::CONFLICT);
    assert_eq!(busy.1["code"], "transfer_busy");
    drop(first);
    drop(second);
    let (status, value) = call(&app, "POST", &path, request.clone(), true).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let prepared: PreparedExecutionInput = serde_json::from_value(value.clone()).unwrap();
    cowork_domain::workspace_transfer::validate(&prepared.snapshot.manifest).unwrap();
    assert_eq!(
        prepared.snapshot.manifest.files[0].sha256,
        cowork_domain::hash(b"frozen")
    );
    assert!(
        !value.to_string().contains("resource_id")
            && !value.to_string().contains(&source.display().to_string())
    );
    std::fs::write(source.join("proof.bin"), b"changed").unwrap();
    assert_eq!(
        call(&app, "POST", &path, request.clone(), true).await.1,
        value
    );
    let read_path = format!("{path}/{}", prepared.snapshot.id);
    assert_eq!(
        call(&app, "GET", &read_path, Value::Null, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "GET", &read_path, Value::Null, true).await.1,
        value
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!(
                "/api/v2/orders/{}/execution-inputs/{}",
                other.id, prepared.snapshot.id
            ),
            Value::Null,
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{read_path}/chunk?path=input-0001/proof.bin&offset=0"),
            Value::Null,
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let mut forged = request.clone();
    forged["manifest"] = json!({"files":[]});
    assert_eq!(
        call(&app, "POST", &path, forged, true).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{read_path}/retire"),
            json!({"sha256":"0".repeat(64)}),
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, retired) = call(
        &app,
        "POST",
        &format!("{read_path}/retire"),
        json!({"sha256":prepared.sha256}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{retired}");
    assert_eq!(retired["retired"], true);
    assert_eq!(call(&app, "POST", &path, request, true).await.1, retired);
    assert!(runtime.store.runs(&order.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_explicit_empty_input_needs_no_native_file_broker_or_model() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "No files".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "Explicit empty input".into(),
        })
        .await
        .unwrap();
    let runtime = Runtime::new(store, vec![], false).unwrap();
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new("private-input-owner".into()),
            native_token: None,
            origins: Arc::new(vec![]),
        },
        None,
    );
    let path = format!("/api/v2/orders/{}/execution-inputs", order.id);
    let (status, value) = call(
        &app,
        "POST",
        &path,
        json!({"idempotency_key":"empty-once","base_revision":order.revision,"folder_ids":[]}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let prepared: PreparedExecutionInput = serde_json::from_value(value).unwrap();
    assert!(prepared.snapshot.sources.is_empty());
    assert!(prepared.snapshot.manifest.files.is_empty());
    assert!(prepared.snapshot.manifest.directories.is_empty());
    assert_eq!(prepared.snapshot.manifest.total_bytes, 0);
    assert!(runtime.store.runs(&order.id).await.unwrap().is_empty());
}
