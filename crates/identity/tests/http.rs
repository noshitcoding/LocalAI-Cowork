use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use cowork_identity::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

const BOOTSTRAP: &str = "http-synthetic-bootstrap-credential-32-minimum";
const SETTLEMENT: &str = "http-synthetic-settlement-credential-32-minimum";
const PASSWORD: &str = "HTTP-synthetic-password-not-for-real-use";

async fn fixture() -> (TempDir, IdentityStore, Router, String, String) {
    let directory = tempfile::tempdir().unwrap();
    let store = IdentityStore::sqlite(&directory.path().join("identity.sqlite3"))
        .await
        .unwrap();
    store
        .initialize_credentials(BOOTSTRAP, SETTLEMENT)
        .await
        .unwrap();
    let (org, session) = store
        .bootstrap(BOOTSTRAP, "http-root", "Root", PASSWORD, "HTTP tenant")
        .await
        .unwrap();
    let app = Router::new().nest(
        "/api/v2/identity",
        cowork_identity::http::router(store.clone()),
    );
    (directory, store, app, session.token, org.id.to_string())
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(format!("/api/v2/identity{path}"));
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Body::from(
            body.map(|v| serde_json::to_vec(&v).unwrap())
                .unwrap_or_default(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn tenant_http_rejects_daemon_bearer_impersonation_and_unrecognized_fields() {
    let (_dir, store, app, token, org) = fixture().await;
    for authority in ["daemon-local-owner-token", BOOTSTRAP, SETTLEMENT] {
        let (status, body) = call(&app, "GET", "/me", Some(authority), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, json!({"error":"authentication_required"}));
    }
    let (status, me) = call(&app, "GET", "/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["username"], "http-root");
    let (status, body) = call(&app, "POST", &format!("/organizations/{org}/resources"), Some(&token), Some(json!({
        "kind":"project", "idempotency_key":"foreign-id-adoption", "id":"a038b214-8cdb-4d67-b6ea-0dce99b076b6"
    }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, json!({"error":"invalid_request"}));
    let (status, body) = call(&app, "POST", "/register", None, Some(json!({
        "username":"unauthorized-admin", "display_name":"User", "password":PASSWORD, "platform_admin":true
    }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, json!({"error":"invalid_request"}));
    let request = Request::builder()
        .uri("/api/v2/identity/me")
        .header("authorization", format!("Bearer {token}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    store.close().await;
}

#[tokio::test]
async fn http_org_membership_requires_explicit_content_acl_and_logout_revokes_access() {
    let (_dir, store, app, admin_token, org) = fixture().await;
    let (status, registered) = call(
        &app,
        "POST",
        "/register",
        None,
        Some(json!({
            "username":"http-user", "display_name":"User", "password":PASSWORD
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(registered["platform_admin"], false);
    let (status, issued) = call(
        &app,
        "POST",
        "/login",
        None,
        Some(json!({
            "username":"http-user", "password":PASSWORD, "ttl_millis":60_000
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let user_token = issued["token"].as_str().unwrap();
    let (status, organizations) = call(&app, "GET", "/organizations", Some(user_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(organizations, json!([]));
    let (status, member) = call(
        &app,
        "POST",
        &format!("/organizations/{org}/members"),
        Some(&admin_token),
        Some(json!({
            "identity_id":registered["id"], "role":"admin"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, resource) = call(
        &app,
        "POST",
        &format!("/organizations/{org}/resources"),
        Some(&admin_token),
        Some(json!({
            "kind":"project", "idempotency_key":"actual-tenant-object"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let resource_id = resource["id"].as_str().unwrap();
    let (status, resources) = call(
        &app,
        "GET",
        &format!("/organizations/{org}/resources"),
        Some(user_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resources, json!([]));
    let (status, denied) = call(
        &app,
        "GET",
        &format!("/organizations/{org}/resources/{resource_id}/grants"),
        Some(user_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(denied, json!({"error":"access_denied"}));
    let (status, _) = call(
        &app,
        "PUT",
        &format!(
            "/organizations/{org}/resources/{resource_id}/grants/{}",
            member["id"].as_str().unwrap()
        ),
        Some(&admin_token),
        Some(json!({
            "expected_revision":0, "permissions":1, "active":true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, resources) = call(
        &app,
        "GET",
        &format!("/organizations/{org}/resources"),
        Some(user_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resources[0]["id"], resource["id"]);
    let (status, _) = call(&app, "POST", "/logout", Some(user_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, "GET", "/me", Some(user_token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    store.close().await;
}

#[tokio::test]
async fn http_body_and_credential_rate_limits_have_redacted_responses() {
    let (_dir, store, app, _token, _org) = fixture().await;
    let body =
        json!({"username":"missing-user", "password":"x".repeat(33 * 1024), "ttl_millis":60_000});
    let (status, reply) = call(&app, "POST", "/login", None, Some(body)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(reply, json!({"error":"invalid_request"}));
    for _ in 0..19 {
        let (status, body) = call(
            &app,
            "POST",
            "/login",
            None,
            Some(json!({"username":"missing-user", "password":PASSWORD, "ttl_millis":60_000})),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, json!({"error":"authentication_required"}));
    }
    let request = Request::builder()
        .method("POST")
        .uri("/api/v2/identity/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "127.0.0.1")
        .body(Body::from("{}"))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        json!({"error":"rate_limited"})
    );
    store.close().await;
}
