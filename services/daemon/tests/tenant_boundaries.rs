use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use cowork_daemon::{AppState, router, router_with_identity};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    bearer: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {bearer}"))
        .header("content-type", "application/json")
        .body(Body::from(
            body.map(|body| serde_json::to_vec(&body).unwrap())
                .unwrap_or_default(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn real_product_router_keeps_tenant_sessions_and_personal_authority_separate() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&directory.path().join("separate-authorities.sqlite3"))
        .await
        .unwrap();
    let identity = store.identity();
    let bootstrap = "synthetic-bootstrap-authority-fixture-only-minimum-32";
    identity
        .initialize_credentials(
            bootstrap,
            "synthetic-settlement-authority-fixture-only-minimum-32",
        )
        .await
        .unwrap();
    let (org, session) = identity
        .bootstrap(
            bootstrap,
            "tenant-owner",
            "Owner",
            "synthetic-password-fixture-only",
            "Own organization",
        )
        .await
        .unwrap();
    let personal_token = "p".repeat(64);
    let state = AppState {
        runtime: Runtime::new(store.clone(), vec![], false).unwrap(),
        token: Arc::new(personal_token.clone()),
        native_token: Some(Arc::new("n".repeat(64))),
        origins: Arc::new(vec![]),
    };
    let disabled = router(state.clone(), None);
    let app = router_with_identity(state, None, Some(identity.clone()));
    let path = format!("/api/v2/tenant/organizations/{}/projects", org.id);
    let input = json!({"name":"Scoped product project", "idempotency_key":"create-once"});
    assert_ne!(
        call(
            &disabled,
            "POST",
            &path,
            &session.token,
            Some(input.clone())
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "POST", &path, &personal_token, Some(input.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, project) = call(&app, "POST", &path, &session.token, Some(input.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(&app, "POST", &path, &session.token, Some(input))
            .await
            .1,
        project
    );
    assert_eq!(
        call(&app, "GET", "/api/v2/projects", &session.token, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "GET", "/api/v2/projects", &personal_token, None)
            .await
            .1,
        json!([])
    );
    let (status, me) = call(&app, "GET", "/api/v2/identity/me", &session.token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(me.to_string().contains("tenant-owner"));
    assert_eq!(
        call(&app, "GET", "/api/v2/identity/me", &personal_token, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let identity2 = identity
        .register_local_identity("other-owner", "Other", "synthetic-password-fixture-other")
        .await
        .unwrap();
    let other = identity
        .login(
            &identity2.username,
            "synthetic-password-fixture-other",
            60_000,
        )
        .await
        .unwrap();
    let detail = format!("{path}/{}", project["id"].as_str().unwrap());
    let (status, hidden) = call(&app, "GET", &detail, &other.token, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!hidden.to_string().contains("Scoped product project"));
    let actor = identity.authenticate_session(&session.token).await.unwrap();
    identity.logout(&actor).await.unwrap();
    assert_eq!(
        call(&app, "GET", &detail, &session.token, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    store.close().await;
}
