//! Public route authentication/extraction plus real local Store rejection.
//! Signed settlement and physical stop are qualified separately.
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

async fn request(
    app: &axum::Router,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap())
}
async fn fixture() -> (tempfile::TempDir, Runtime, axum::Router) {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(
        Store::connect("sqlite::memory:").await.unwrap(),
        vec![],
        true,
    )
    .unwrap()
    .with_file_broker(cowork_broker::FileBroker::open(directory.path(), vec![]).unwrap());
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new("reconcile-owner".into()),
            native_token: Some(Arc::new("reconcile-native".into())),
            origins: Arc::new(vec![]),
        },
        None,
    );
    (directory, runtime, app)
}
const PATH: &str =
    "/api/v2/orders/unknown-order/runs/unknown-run/managed-processes/unknown-attempt/reconcile";

#[tokio::test]
async fn reconciliation_authenticates_owner_before_json_extraction() {
    let (_directory, runtime, app) = fixture().await;
    for token in [None, Some("wrong"), Some("reconcile-native")] {
        let (status, headers, body) = request(&app, PATH, token, "not-json").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(body["code"], "authentication_required");
    }
    runtime.store.close().await;
}

#[tokio::test]
async fn reconciliation_rejects_renderer_evidence_and_malformed_intent_without_cache() {
    let (_directory, runtime, app) = fixture().await;
    let valid = json!({"expected_run_revision":1,"expected_request_sha256":"a".repeat(64),"finish_cancellation":true});
    for body in [
        "not-json".to_owned(),
        json!({}).to_string(),
        {
            let mut body = valid.clone();
            body["result"] = json!({"termination":"canceled"});
            body.to_string()
        },
        {
            let mut body = valid.clone();
            body["runner_signed_receipt"] = json!({});
            body.to_string()
        },
        {
            let mut body = valid.clone();
            body["expected_run_revision"] = json!(0);
            body.to_string()
        },
        {
            let mut body = valid.clone();
            body["expected_request_sha256"] = json!("invalid");
            body.to_string()
        },
    ] {
        let (status, headers, body) = request(&app, PATH, Some("reconcile-owner"), &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(body["code"], "invalid_request");
        assert_eq!(body["retryable"], false);
    }
    runtime.store.close().await;
}

#[tokio::test]
async fn reconciliation_unknown_scope_never_creates_a_run_attempt_or_result() {
    let (directory, runtime, app) = fixture().await;
    let intent = ReconcileOwnerManagedProcess {
        expected_run_revision: 1,
        expected_request_sha256: "a".repeat(64),
        finish_cancellation: true,
    };
    let (status, headers, body) = request(
        &app,
        PATH,
        Some("reconcile-owner"),
        &serde_json::to_string(&intent).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(headers["cache-control"], "no-store");
    assert!(!directory.path().join("managed-results").exists());
    assert!(
        runtime
            .store
            .runs("unknown-order")
            .await
            .unwrap()
            .is_empty()
    );
    runtime.store.close().await;
}
