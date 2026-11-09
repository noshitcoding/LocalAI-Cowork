use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use cowork_contracts::*;
use cowork_runtime::{Runtime, provider};
use cowork_storage::Store;
use serde_json::json;

async fn server() -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route("/v1/models", get(|headers: HeaderMap| async move {
            if headers.get(header::AUTHORIZATION).and_then(|v|v.to_str().ok()) != Some("Bearer fixture-key") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            Json(json!({"data":[{"id":"qwen38"},{"id":"another-model"},{"id":"qwen38"},{"id":""},{"id":"bad\nvalue"}]})).into_response()
        }))
        .route("/api/tags", get(|| async { Json(json!({"models":[{"name":"qwen:latest"}]})) }))
        .route("/empty/models", get(|| async { Json(json!({"data":[]})) }))
        .route("/invalid/models", get(|| async { Json(json!({"models":[{"name":"not-openai"}]})) }))
        .route("/denied/models", get(|| async { StatusCode::FORBIDDEN }))
        .route("/limited/models", get(|| async { StatusCode::TOO_MANY_REQUESTS }))
        .route("/redirect/models", get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/v1/models")]) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (endpoint, task)
}

#[tokio::test]
async fn discovers_without_name_model_or_persisting_credentials_and_profiles() {
    let (endpoint, task) = server().await;
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    let input = DiscoverProviderModels {
        kind: ProviderKind::OpenaiCompatible,
        endpoint: format!("{endpoint}/v1/"),
        api_key: Some("fixture-key".into()),
        explicitly_allow_no_key: false,
    };
    assert_eq!(
        runtime.discover_provider_models(&input).await.unwrap(),
        vec!["another-model", "qwen38"]
    );
    assert!(store.providers().await.unwrap().is_empty());
    assert!(runtime.profiles().await.is_empty());
    let anonymous = DiscoverProviderModels {
        kind: ProviderKind::Ollama,
        endpoint: endpoint.clone(),
        api_key: None,
        explicitly_allow_no_key: true,
    };
    assert_eq!(
        runtime.discover_provider_models(&anonymous).await.unwrap(),
        vec!["qwen:latest"]
    );
    let contradictory = DiscoverProviderModels {
        explicitly_allow_no_key: true,
        ..input
    };
    assert!(
        runtime
            .discover_provider_models(&contradictory)
            .await
            .is_err()
    );
    let missing = DiscoverProviderModels {
        explicitly_allow_no_key: false,
        ..anonymous
    };
    assert!(runtime.discover_provider_models(&missing).await.is_err());
    task.abort();
}

#[tokio::test]
async fn separates_path_auth_catalog_and_model_failures_without_echoing_secrets() {
    let (endpoint, task) = server().await;
    for (suffix, key, code) in [
        ("", Some("fixture-key"), "provider_endpoint_not_found"),
        (
            "/v1",
            Some("private-invalid-secret"),
            "provider_authentication_failed",
        ),
        ("/denied", None, "provider_access_denied"),
        ("/limited", None, "provider_rate_limited"),
        ("/invalid", None, "provider_catalog_invalid"),
        (
            "/redirect",
            Some("private-invalid-secret"),
            "provider_unavailable",
        ),
    ] {
        let error = provider::model_catalog(
            &ProviderKind::OpenaiCompatible,
            &format!("{endpoint}{suffix}"),
            key.map(str::to_owned),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), code);
        assert!(!error.to_string().contains("private-invalid-secret"));
    }
    assert!(
        provider::model_catalog(
            &ProviderKind::OpenaiCompatible,
            &format!("{endpoint}/empty"),
            None
        )
        .await
        .unwrap()
        .is_empty()
    );
    let profile = ProviderProfile {
        id: "direct".into(),
        name: "PCAI".into(),
        kind: ProviderKind::OpenaiCompatible,
        endpoint: format!("{endpoint}/v1"),
        model: "deployment-alias".into(),
        revision: 1,
        context_tokens: 32768,
        credential_required: true,
        credential_ready: true,
        generation: Default::default(),
        capabilities: Default::default(),
    };
    assert_eq!(
        provider::probe(&profile, Some("fixture-key".into()), false)
            .await
            .unwrap_err()
            .code(),
        "provider_model_missing"
    );
    let corrected = ProviderProfile {
        model: "qwen38".into(),
        ..profile
    };
    provider::probe(&corrected, Some("fixture-key".into()), false)
        .await
        .unwrap();
    for invalid in [
        "http://remote.example/v1",
        "https://user:secret@example.com/v1",
        "https://example.com/v1?key=secret",
        "https://example.com/v1#secret",
    ] {
        assert!(
            provider::model_catalog(&ProviderKind::OpenaiCompatible, invalid, None)
                .await
                .is_err()
        );
    }
    task.abort();
}
