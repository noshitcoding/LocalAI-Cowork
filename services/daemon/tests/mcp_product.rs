use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::Runtime;
use cowork_storage::{Store, id};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn request(
    app: &axum::Router,
    path: &str,
    body: Option<Value>,
    origin: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .uri(path)
        .method(if body.is_some() { "POST" } else { "GET" })
        .header("authorization", "Bearer owner-fixture")
        .header("content-type", "application/json");
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(body.map_or(Body::empty(), |v| Body::from(v.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn owner_catalog_import_is_scoped_idempotent_and_revision_checked() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject { name: "One".into() })
        .await
        .unwrap();
    let other = store
        .create_project(CreateProject { name: "Two".into() })
        .await
        .unwrap();
    let app = router(
        AppState {
            runtime: Runtime::new(store.clone(), vec![], false).unwrap(),
            native_token: None,
            token: Arc::new("owner-fixture".into()),
            origins: Arc::new(vec!["http://localhost:5175".into()]),
        },
        None,
    );
    // A closed local port proves registration/validation does not connect.
    let input = json!({"server_id":id(),"base_revision":0,"idempotency_key":"create","configuration":{"name":"Manual catalogue","endpoint":"http://127.0.0.1:1/mcp","credential_reference":null,"credential_revision":0,"catalog":[{"name":"external","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}],"selected_tools":["external"],"enabled":true}});
    let path = format!("/api/v2/projects/{}/mcp-servers", project.id);
    assert_eq!(
        request(
            &app,
            &path,
            Some(input.clone()),
            Some("https://hostile.invalid")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert!(store.mcp_servers(&project.id).await.unwrap().is_empty());
    let (status, saved) = request(
        &app,
        &path,
        Some(input.clone()),
        Some("http://localhost:5175"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let parsed: McpServer = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(parsed.revision, 1);
    assert_eq!(parsed.project_id, project.id);
    assert_eq!(
        request(&app, &path, Some(input.clone()), None).await.1,
        saved
    );
    let mut revoke = input.clone();
    revoke["base_revision"] = json!(1);
    revoke["idempotency_key"] = json!("revoke");
    revoke["configuration"]["enabled"] = json!(false);
    assert_eq!(
        request(&app, &path, Some(revoke), None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, &path, Some(input.clone()), None).await.1,
        saved
    );
    let current = request(&app, &path, None, None).await.1;
    assert_eq!(current[0]["revision"], 2);
    assert_eq!(current[0]["configuration"]["enabled"], false);
    let mut stale = input.clone();
    stale["idempotency_key"] = json!("different");
    assert_eq!(
        request(&app, &path, Some(stale), None).await.0,
        StatusCode::CONFLICT
    );
    let other_path = format!("/api/v2/projects/{}/mcp-servers", other.id);
    assert_eq!(request(&app, &other_path, None, None).await.1, json!([]));
    assert_eq!(
        request(&app, &other_path, Some(input.clone()), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(&app, "/api/v2/projects/missing/mcp-servers", None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let mut secret = input;
    secret["configuration"]["bearer"] = json!("must-not-leak");
    let (status, error) = request(&app, &path, Some(secret), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!error.to_string().contains("must-not-leak"));
}

#[tokio::test]
async fn catalog_validation_rejects_schema_fetching_and_embedded_secrets() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Validation".into(),
        })
        .await
        .unwrap();
    let app = router(
        AppState {
            runtime: Runtime::new(store.clone(), vec![], false).unwrap(),
            native_token: None,
            token: Arc::new("owner-fixture".into()),
            origins: Arc::new(vec![]),
        },
        None,
    );
    let path = format!("/api/v2/projects/{}/mcp-servers", project.id);
    let mut input = json!({"server_id":id(),"base_revision":0,"idempotency_key":id(),"configuration":{"name":"Fixture","endpoint":"http://127.0.0.1:1/mcp","credential_reference":null,"credential_revision":0,"catalog":[{"name":"external","inputSchema":{"$ref":"https://example.invalid/schema.json"}}],"selected_tools":["external"],"enabled":true}});
    assert_eq!(
        request(&app, &path, Some(input.clone()), None).await.0,
        StatusCode::BAD_REQUEST
    );
    input["configuration"]["catalog"] = json!([]);
    input["configuration"]["selected_tools"] = json!([]);
    input["configuration"]["endpoint"] = json!("https://user:secret@example.invalid/mcp");
    assert_eq!(
        request(&app, &path, Some(input), None).await.0,
        StatusCode::BAD_REQUEST
    );
    assert!(store.mcp_servers(&project.id).await.unwrap().is_empty());
    assert_eq!(
        request(&app, "/api/v2/runs/missing/mcp-attempts", None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
