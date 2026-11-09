use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use cowork_daemon::{AppState, router, server_config};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn configured_https_origin_still_requires_exact_personal_authority() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let app = router(
        AppState {
            runtime: Runtime::new(store.clone(), vec![], false).unwrap(),
            token: Arc::new("private-origin-personal-fixture".into()),
            native_token: Some(Arc::new("private-origin-native-fixture".into())),
            origins: Arc::new(
                server_config::origins(43821, Some("https://cowork.example")).unwrap(),
            ),
        },
        None,
    );
    for (origin, bearer, expected) in [
        (
            "https://cowork.example",
            "private-origin-personal-fixture",
            StatusCode::OK,
        ),
        (
            "https://cowork.example",
            "private-origin-native-fixture",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "https://cowork.example",
            "tenant-session",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "https://cowork.example:8443",
            "private-origin-personal-fixture",
            StatusCode::FORBIDDEN,
        ),
        (
            "https://other.example",
            "private-origin-personal-fixture",
            StatusCode::FORBIDDEN,
        ),
        (
            "http://cowork.example",
            "private-origin-personal-fixture",
            StatusCode::FORBIDDEN,
        ),
        (
            "null",
            "private-origin-personal-fixture",
            StatusCode::FORBIDDEN,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/personal-money")
                    .header(header::ORIGIN, origin)
                    .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let csp = response.headers()["content-security-policy"]
            .to_str()
            .unwrap();
        let connect = csp
            .split(';')
            .find(|directive| directive.trim().starts_with("connect-src "))
            .unwrap();
        assert_eq!(
            connect.trim(),
            "connect-src 'self' https: http://127.0.0.1:* http://localhost:*"
        );
        assert!(
            !csp.contains("connect-src *")
                && !connect.split_whitespace().any(|source| source == "http:")
        );
        assert!(
            csp.contains("script-src 'self'")
                && !csp.contains("'unsafe-inline'")
                && !csp.contains("'unsafe-eval'")
        );
        if expected == StatusCode::OK {
            assert_eq!(
                response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
                "https://cowork.example"
            );
        }
    }
    for origin in ["https://cowork.example", "https://other.example"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/v2/personal-money")
                    .header(header::ORIGIN, origin)
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|v| v.to_str().ok()),
            if origin == "https://cowork.example" {
                Some(origin)
            } else {
                None
            }
        );
    }
    store.close().await;
}
