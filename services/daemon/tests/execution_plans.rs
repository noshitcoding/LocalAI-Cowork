use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn fixture() -> (axum::Router, Runtime, Value) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Plan API".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "Review".into(),
        })
        .await
        .unwrap();
    let profile = ProviderProfile {
        id: "fixture".into(),
        name: "Fixture".into(),
        kind: ProviderKind::Test,
        endpoint: "".into(),
        model: "fixture".into(),
        revision: 1,
        context_tokens: 8192,
        credential_required: false,
        credential_ready: true,
        generation: Default::default(),
        capabilities: Default::default(),
    };
    store.insert_provider(&profile).await.unwrap();
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        true,
    )
    .unwrap();
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new("private-owner".into()),
            native_token: None,
            origins: Arc::new(vec!["http://127.0.0.1:5175".into()]),
        },
        None,
    );
    let input = json!({"order_id":order.id,"base_revision":order.revision,"idempotency_key":"original",
        "prompt":"Produce a result","provider_id":"fixture","provider_revision":1});
    (app, runtime, input)
}

async fn call(
    app: &axum::Router,
    path: &str,
    body: Value,
    authenticated: bool,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if authenticated {
        request = request.header(header::AUTHORIZATION, "Bearer private-owner");
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    (status, body)
}

#[tokio::test]
async fn authenticated_review_is_required_and_confirmation_preserves_the_original_ack() {
    let (app, runtime, input) = fixture().await;
    assert_eq!(
        call(&app, "/api/v2/execution-plans", input.clone(), false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, error) = call(&app, "/api/v2/runs", input.clone(), true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["code"], "execution_plan_required");
    let (status, prepared) = call(&app, "/api/v2/execution-plans", input.clone(), true).await;
    assert_eq!(status, StatusCode::OK);
    let order = input["order_id"].as_str().unwrap();
    assert!(runtime.store.runs(order).await.unwrap().is_empty());
    let path = format!(
        "/api/v2/orders/{order}/execution-plans/{}/commit",
        prepared["plan"]["id"].as_str().unwrap()
    );
    let body = json!({"sha256":prepared["sha256"]});
    let (status, run) = call(&app, &path, body.clone(), true).await;
    assert_eq!(status, StatusCode::OK);
    runtime
        .store
        .cancel(run["id"].as_str().unwrap())
        .await
        .unwrap();
    runtime.providers.write().await.clear();
    assert_eq!(
        call(&app, &path, body, true).await,
        (StatusCode::OK, run.clone())
    );
    assert_eq!(
        call(&app, "/api/v2/runs", input, true).await,
        (StatusCode::OK, run)
    );
}

#[tokio::test]
async fn caller_cannot_supply_scope_or_substitute_another_plan_hash() {
    let (app, runtime, input) = fixture().await;
    let mut forged = input.clone();
    forged["actor"] = json!("somebody-else");
    assert_eq!(
        call(&app, "/api/v2/execution-plans", forged, true).await.0,
        StatusCode::BAD_REQUEST
    );
    let (_, prepared) = call(&app, "/api/v2/execution-plans", input.clone(), true).await;
    let order = input["order_id"].as_str().unwrap();
    let path = format!(
        "/api/v2/orders/{order}/execution-plans/{}/commit",
        prepared["plan"]["id"].as_str().unwrap()
    );
    assert_eq!(
        call(&app, &path, json!({"sha256":"0".repeat(64)}), true)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            &path,
            json!({"sha256":prepared["sha256"],"spec":{"executor":"host"}}),
            true
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(runtime.store.runs(order).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_unacknowledged_prepare_can_be_retired_without_enabling_a_late_start() {
    let (app, runtime, input) = fixture().await;
    let (status, receipt) = call(
        &app,
        "/api/v2/execution-plans/retire-request",
        input.clone(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(receipt["retired"], true);
    assert_eq!(
        call(&app, "/api/v2/execution-plans", input.clone(), true)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert!(
        runtime
            .store
            .runs(input["order_id"].as_str().unwrap())
            .await
            .unwrap()
            .is_empty()
    );
}
