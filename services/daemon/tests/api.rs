use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use cowork_daemon::{AppState, router};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use std::sync::Arc;
use tower::ServiceExt;
async fn app() -> axum::Router {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let runtime = Runtime::new(store, vec![], false).unwrap();
    router(
        AppState {
            native_token: None,
            runtime,
            token: Arc::new("a-private-token-for-tests".into()),
            origins: Arc::new(vec!["http://127.0.0.1:5175".into()]),
        },
        None,
    )
}

#[tokio::test]
async fn model_discovery_requires_owner_auth_but_no_saved_profile_or_model() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let upstream = axum::Router::new().route(
        "/v1/models",
        axum::routing::get(|| async { axum::Json(serde_json::json!({"data":[{"id":"qwen38"}]})) }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let app = app().await;
    let payload = serde_json::json!({"kind":"openai_compatible", "endpoint":endpoint, "explicitly_allow_no_key":true});
    for authorized in [false, true] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v2/providers/discover-models")
            .header(header::CONTENT_TYPE, "application/json");
        if authorized {
            request = request.header(header::AUTHORIZATION, "Bearer a-private-token-for-tests");
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(payload.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if authorized {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            }
        );
        if authorized {
            let value: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            assert_eq!(value, serde_json::json!({"models":["qwen38"]}));
        }
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v2/providers")
                .header(header::AUTHORIZATION, "Bearer a-private-token-for-tests")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
        b"[]"
    );
    server.abort();
}
#[tokio::test]
async fn token_and_origin_are_both_checked() {
    let app = app().await;
    let no_token = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v2/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_token.status(), StatusCode::UNAUTHORIZED);
    let hostile_origin = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v2/projects")
                .header(header::AUTHORIZATION, "Bearer a-private-token-for-tests")
                .header(header::ORIGIN, "https://malicious.invalid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hostile_origin.status(), StatusCode::FORBIDDEN);
    let allowed = app
        .oneshot(
            Request::builder()
                .uri("/api/v2/projects")
                .header(header::AUTHORIZATION, "Bearer a-private-token-for-tests")
                .header(header::ORIGIN, "http://127.0.0.1:5175")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
}
#[tokio::test]
async fn invalid_input_has_a_stable_error_and_no_secret() {
    let response = app()
        .await
        .oneshot(
            Request::builder()
                .uri("/api/v2/projects")
                .method("POST")
                .header(header::AUTHORIZATION, "Bearer a-private-token-for-tests")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name":"x","unexpected":"secret"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(response.into_body(), 10_000).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["code"], "invalid_request");
    assert!(value["correlation_id"].is_string());
    assert!(
        !String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("secret")
    );
}
#[tokio::test]
async fn v1_is_explicitly_unavailable_until_qualified() {
    let response = app()
        .await
        .oneshot(
            Request::builder()
                .uri("/api/v1/threads")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn binary_download_requires_auth_and_returns_safe_attachment_headers() {
    use cowork_contracts::{CreateOrder, CreateProject};
    let temp = tempfile::tempdir().unwrap();
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Download fixture".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "Binary file".into(),
        })
        .await
        .unwrap();
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_file_broker(cowork_broker::FileBroker::open(temp.path(), vec![]).unwrap());
    runtime.workspace(&order.id).await.unwrap();
    let bytes = [0, 255, 128, 42];
    std::fs::write(
        temp.path()
            .join("workspaces")
            .join(&order.id)
            .join("files/Prüfung.bin"),
        bytes,
    )
    .unwrap();
    let app = router(
        AppState {
            native_token: None,
            runtime,
            token: Arc::new("private-test-token".into()),
            origins: Arc::new(vec!["http://127.0.0.1:5175".into()]),
        },
        None,
    );
    let path = format!(
        "/api/v2/orders/{}/workspace/download?path=Pr%C3%BCfung.bin",
        order.id
    );
    let response = app
        .clone()
        .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header(header::AUTHORIZATION, "Bearer private-test-token")
                .header(header::ORIGIN, "https://hostile.invalid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header(header::AUTHORIZATION, "Bearer private-test-token")
                .header(header::ORIGIN, "http://127.0.0.1:5175")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "4");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.headers()["x-cowork-sha256"],
        cowork_domain::hash(&bytes)
    );
    assert_eq!(
        response.headers()["x-cowork-file-name"],
        "Pr%C3%BCfung%2Ebin"
    );
    assert!(
        response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    assert!(
        response.headers()[header::ACCESS_CONTROL_EXPOSE_HEADERS]
            .to_str()
            .unwrap()
            .contains("x-cowork-sha256")
    );
    assert_eq!(
        to_bytes(response.into_body(), 100).await.unwrap().as_ref(),
        bytes
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("{path}&expected_sha256={}", "0".repeat(64)))
                .header(header::AUTHORIZATION, "Bearer private-test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let failure: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
    assert_eq!(failure["code"], "file_conflict");
}
