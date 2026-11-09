use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use cowork_daemon::{AppState, router_with_identity};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn request(
    app: &axum::Router,
    path: &str,
    method: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().uri(path).method(method);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = if let Some(body) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn owner_and_actor_routes_keep_authentication_content_and_preferences_separate() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let identity = store.identity();
    let bootstrap = "b".repeat(64);
    identity
        .initialize_credentials(&bootstrap, &"s".repeat(64))
        .await
        .unwrap();
    let (_, issued) = identity
        .bootstrap(
            &bootstrap,
            "library-route-owner",
            "Route owner",
            "SyntheticPassword-2026!",
            "Library routes",
        )
        .await
        .unwrap();
    identity
        .register_local_identity(
            "library-route-other",
            "Other",
            "OtherSyntheticPassword-2026!",
        )
        .await
        .unwrap();
    let other = identity
        .login(
            "library-route-other",
            "OtherSyntheticPassword-2026!",
            3_600_000,
        )
        .await
        .unwrap();
    let app = router_with_identity(
        AppState {
            runtime: Runtime::new(store, vec![], false).unwrap(),
            token: Arc::new("library-installation-owner-token".into()),
            native_token: None,
            origins: Arc::new(vec!["http://127.0.0.1:5175".into()]),
        },
        None,
        Some(identity.clone()),
    );
    let local = "library-installation-owner-token";
    let owner_path = "/api/v2/library/personalities";
    let actor_path = "/api/v1/library/personalities";
    assert_eq!(
        request(&app, owner_path, "GET", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, actor_path, "GET", Some(local), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, owner_path, "GET", Some(&issued.token), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let body = json!({"idempotency_key":"create","expected_revision":0,"value":{"name":"Owner personality","system_prompt":"Local prompt"}});
    let (status, created) =
        request(&app, owner_path, "POST", Some(local), Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK);
    let (status, retry) = request(&app, owner_path, "POST", Some(local), Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created, retry);
    let foreign = format!("{actor_path}/{}", created["id"].as_str().unwrap());
    assert_eq!(
        request(&app, &foreign, "GET", Some(&issued.token), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let body = json!({"idempotency_key":"create","expected_revision":0,"value":{"name":"Private actor","system_prompt":"Actor prompt"}});
    let (status, private) =
        request(&app, actor_path, "POST", Some(&issued.token), Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        request(&app, actor_path, "GET", Some(&other.token), None)
            .await
            .1,
        json!([])
    );
    let path = format!("{actor_path}/{}", private["id"].as_str().unwrap());
    assert_eq!(
        request(
            &app,
            &path,
            "PUT",
            Some(&issued.token),
            Some(json!({"idempotency_key":"stale","expected_revision":0,"value":{"name":"Stale"}}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status,prefs)=request(&app,"/api/v1/preferences","PUT",Some(&issued.token),Some(json!({"idempotency_key":"prefs","expected_revision":0,"value":{"system_prompt":"Private default","font_scale":125}}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(prefs["value"]["font_scale"], 125);
    assert_eq!(
        request(&app, "/api/v2/preferences", "GET", Some(local), None)
            .await
            .1["value"]["system_prompt"],
        ""
    );
    assert_eq!(request(&app,owner_path,"POST",Some(local),Some(json!({"idempotency_key":"bad","expected_revision":0,"value":{"name":"bad","api_key":"secret"}}))).await.0,StatusCode::BAD_REQUEST);
    let (status, runtime) = request(
        &app,
        "/api/v1/codex/runtime",
        "GET",
        Some(&issued.token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(runtime["available"], false);
    assert_eq!(
        request(&app, "/api/v1/codex/accounts", "GET", Some(local), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status,codex)=request(&app,"/api/v1/codex/accounts","POST",Some(&issued.token),Some(json!({"idempotency_key":"codex-create","expected_revision":0,"value":{"name":"Private Codex","priority":1}}))).await;
    assert_eq!(status, StatusCode::OK);
    let codex_path = format!("/api/v1/codex/accounts/{}", codex["id"].as_str().unwrap());
    assert_eq!(
        request(&app, &codex_path, "GET", Some(&other.token), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            &format!("{codex_path}/login"),
            "POST",
            Some(&issued.token),
            Some(
                json!({"idempotency_key":"login-unavailable","expected_revision":1,"flow":"device"})
            )
        )
        .await
        .0,
        StatusCode::BAD_GATEWAY
    );
    let deletion = json!({"idempotency_key":"codex-delete","expected_revision":1});
    let (status, deleted) = request(
        &app,
        &codex_path,
        "DELETE",
        Some(&issued.token),
        Some(deletion.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        request(
            &app,
            &codex_path,
            "DELETE",
            Some(&issued.token),
            Some(deletion)
        )
        .await,
        (StatusCode::OK, deleted)
    );
    let actor = identity.authenticate_session(&issued.token).await.unwrap();
    identity.logout(&actor).await.unwrap();
    assert_eq!(
        request(&app, &path, "GET", Some(&issued.token), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}
