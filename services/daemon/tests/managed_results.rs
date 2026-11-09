//! Route wiring/authentication with actual local Store/FileBroker state.
//! Native result/retention semantics are covered by the Runtime result fixtures.
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use std::sync::Arc;
use tower::ServiceExt;

async fn request(
    app: &axum::Router,
    method: Method,
    path: &str,
    authenticated: bool,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut request = Request::builder().method(method).uri(path);
    if authenticated {
        request = request.header("authorization", "Bearer managed-result-owner");
    }
    // Invalid JSON/query/body does not bypass the outer Owner middleware.
    let response = app
        .clone()
        .oneshot(request.body(Body::from("not-json")).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    (
        status,
        headers,
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
}

async fn fixture() -> (tempfile::TempDir, Runtime, Order, RunRecord, axum::Router) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Result API".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "Local scope".into(),
        })
        .await
        .unwrap();
    let profile: ProviderProfile = serde_json::from_value(serde_json::json!({"id":"fixture","name":"Fixture","kind":"test","model":"test","endpoint":"","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true})).unwrap();
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
    .with_file_broker(cowork_broker::FileBroker::open(directory.path(), vec![]).unwrap());
    runtime.workspace(&order.id).await.unwrap();
    let input: CreateRun = serde_json::from_value(serde_json::json!({"order_id":order.id,"base_revision":order.revision,"idempotency_key":"result-api","prompt":"No process launched.","provider_id":"fixture","provider_revision":1,"access_mode":"autonomous"})).unwrap();
    let run = runtime.create_run(&input).await.unwrap();
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new("managed-result-owner".into()),
            native_token: None,
            origins: Arc::new(vec![]),
        },
        None,
    );
    (directory, runtime, order, run, app)
}

#[tokio::test]
async fn managed_result_routes_authenticate_before_extraction_or_action() {
    let (_directory, runtime, order, run, app) = fixture().await;
    let base = format!("/api/v2/orders/{}/runs/{}", order.id, run.id);
    for (method, path) in [
        (Method::GET, format!("{base}/managed-results")),
        (
            Method::POST,
            format!("{base}/managed-results/unknown/receive"),
        ),
        (
            Method::GET,
            format!("{base}/managed-results/unknown/delivery"),
        ),
        (
            Method::POST,
            format!("{base}/managed-results/unknown/delivery/prepare"),
        ),
        (
            Method::POST,
            format!("{base}/managed-results/unknown/delivery/confirm"),
        ),
        (
            Method::POST,
            format!("{base}/managed-results/unknown/delivery/retirement/refresh"),
        ),
        (
            Method::GET,
            format!("{base}/managed-result-versions/unknown/manifest"),
        ),
        (
            Method::GET,
            format!("{base}/managed-result-versions/unknown/chunk?offset=invalid"),
        ),
    ] {
        let response = request(&app, method, &path, false).await;
        assert_eq!(response.0, StatusCode::UNAUTHORIZED);
        assert_eq!(response.1["cache-control"], "no-store");
        let error: ApiError = serde_json::from_slice(&response.2).unwrap();
        assert_eq!(error.code, "authentication_required");
    }
    assert!(
        runtime
            .store
            .process_attempts(&run.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn managed_result_gets_are_private_scoped_and_never_allocate_or_start() {
    let (directory, runtime, order, run, app) = fixture().await;
    let base = format!("/api/v2/orders/{}/runs/{}", order.id, run.id);
    let list = request(&app, Method::GET, &format!("{base}/managed-results"), true).await;
    assert_eq!(
        list.0,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&list.2)
    );
    assert_eq!(list.1["cache-control"], "no-store");
    assert!(
        serde_json::from_slice::<OwnerManagedResultList>(&list.2)
            .unwrap()
            .entries
            .is_empty()
    );
    for suffix in [
        "manifest".to_string(),
        format!(
            "chunk?path=result.txt&offset=0&expected_sha256={}",
            "0".repeat(64)
        ),
    ] {
        let result = request(
            &app,
            Method::GET,
            &format!("{base}/managed-result-versions/unknown/{suffix}"),
            true,
        )
        .await;
        assert_eq!(result.0, StatusCode::NOT_FOUND);
        assert_eq!(result.1["cache-control"], "no-store");
    }
    let malformed = request(
        &app,
        Method::GET,
        &format!("{base}/managed-result-versions/unknown/chunk?offset=invalid"),
        true,
    )
    .await;
    assert_eq!(malformed.0, StatusCode::BAD_REQUEST);
    assert_eq!(malformed.1["cache-control"], "no-store");
    let error: ApiError = serde_json::from_slice(&malformed.2).unwrap();
    assert_eq!(error.code, "invalid_request");
    assert!(!error.retryable);
    let foreign = base.replace(&order.id, &uuid::Uuid::new_v4().to_string());
    assert_eq!(
        request(
            &app,
            Method::GET,
            &format!("{foreign}/managed-results"),
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let receive = request(
        &app,
        Method::POST,
        &format!("{base}/managed-results/unknown/receive"),
        true,
    )
    .await;
    assert_eq!(receive.0, StatusCode::NOT_FOUND);
    assert_eq!(receive.1["cache-control"], "no-store");
    for (method, suffix) in [
        (Method::GET, ""),
        (Method::POST, "/prepare"),
        (Method::POST, "/retirement/refresh"),
    ] {
        let response = request(
            &app,
            method,
            &format!("{base}/managed-results/unknown/delivery{suffix}"),
            true,
        )
        .await;
        assert_eq!(response.0, StatusCode::NOT_FOUND);
        assert_eq!(response.1["cache-control"], "no-store");
    }
    let malformed = request(
        &app,
        Method::POST,
        &format!("{base}/managed-results/unknown/delivery/confirm"),
        true,
    )
    .await;
    assert!(malformed.0.is_client_error());
    assert_eq!(malformed.1["cache-control"], "no-store");
    assert_eq!(
        serde_json::from_slice::<ApiError>(&malformed.2)
            .unwrap()
            .code,
        "invalid_request"
    );
    assert!(!directory.path().join("managed-results").exists());
    assert!(
        runtime
            .store
            .process_attempts(&run.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(runtime.store.runs(&order.id).await.unwrap().len(), 1);
    assert_eq!(
        runtime.store.detail(&run.id).await.unwrap().run.state,
        run.state
    );
}
