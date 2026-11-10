use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use cowork_contracts::{ProviderKind, ProviderProfile};
use cowork_daemon::{AppState, router};
use cowork_runtime::Runtime;
use cowork_storage::{Store, model_money::*};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

fn configuration() -> PersonalMoneyConfiguration {
    let currency = Currency {
        code: "USD".into(),
        scale: 6,
    };
    let provider = ProviderProfile {
        generation: Default::default(),
        id: "private-status-api-provider".into(),
        name: "Private fixture".into(),
        kind: ProviderKind::Ollama,
        endpoint: "http://127.0.0.1:9".into(),
        model: "private-model".into(),
        revision: 1,
        context_tokens: 1024,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    let price = PriceVersion {
        id: "private-api-price".into(),
        version: 1,
        provider_id: provider.id.clone(),
        model_sha256: "a".repeat(64),
        currency: currency.clone(),
        input_units_per_million: 0,
        output_units_per_million: 0,
        cached_input_units_per_million: None,
        source_sha256: "b".repeat(64),
    };
    PersonalMoneyConfiguration {
        format_version: 1,
        installation: PoolPolicy {
            scope: PoolScope::Installation("private-status-api-installation".into()),
            version: 1,
            currency,
            limit_units: u64::MAX,
        },
        project_limit_units: u64::MAX,
        run_limit_units: 9_007_199_254_740_993,
        pool_policy_version: 1,
        deployments: vec![PersonalMoneyDeployment {
            deployment: OperatorModelDeployment {
                id: "private-api-deployment".into(),
                provider,
                model_sha256: price.model_sha256.clone(),
                price_id: price.id.clone(),
                price_version: 1,
                token_mode: MonetaryTokenMode::ConservativeContextUpperBound,
            },
            price,
        }],
    }
}
fn app(store: Store) -> axum::Router {
    router(
        AppState {
            runtime: Runtime::new(store, vec![], false).unwrap(),
            token: Arc::new("personal-status-fixture-bearer".into()),
            native_token: Some(Arc::new("separate-native-fixture-bearer".into())),
            origins: Arc::new(vec!["http://127.0.0.1:5175".into()]),
        },
        None,
    )
}
fn request(method: &str, bearer: Option<&str>, origin: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri("/api/v2/personal-money");
    if let Some(bearer) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    builder.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn status_uses_only_existing_personal_auth_and_exposes_no_mutation() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let app = app(store.clone());
    for bearer in [
        None,
        Some("wrong"),
        Some("separate-native-fixture-bearer"),
        Some("tenant-session-is-not-personal-authority"),
    ] {
        let response = app
            .clone()
            .oneshot(request("GET", bearer, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    assert_eq!(
        app.clone()
            .oneshot(request(
                "GET",
                Some("personal-status-fixture-bearer"),
                Some("https://hostile.invalid")
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        assert_eq!(
            app.clone()
                .oneshot(request(
                    method,
                    Some("personal-status-fixture-bearer"),
                    None
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    let response = app
        .oneshot(request(
            "GET",
            Some("personal-status-fixture-bearer"),
            Some("http://127.0.0.1:5175"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        serde_json::from_slice::<Value>(&to_bytes(response.into_body(), 8192).await.unwrap())
            .unwrap(),
        json!({"origin":"not_configured"})
    );
    store.close().await;
}

#[tokio::test]
async fn legacy_is_active_and_configured_status_redacts_private_operator_data() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let c = configuration();
    store
        .register_personal_model_deployment(
            c.installation.clone(),
            c.deployments[0].deployment.clone(),
            c.deployments[0].price.clone(),
        )
        .await
        .unwrap();
    let app = app(store.clone());
    let response = app
        .clone()
        .oneshot(request("GET", Some("personal-status-fixture-bearer"), None))
        .await
        .unwrap();
    let legacy: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(legacy["origin"], "legacy_manual");
    assert!(legacy.get("defaults").is_none());
    store.configure_personal_money(&c).await.unwrap();
    let response = app
        .oneshot(request("GET", Some("personal-status-fixture-bearer"), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["origin"], "operator_configuration");
    assert_eq!(value["installation"]["limit_units"], u64::MAX.to_string());
    assert_eq!(value["defaults"]["run_limit_units"], "9007199254740993");
    for private in [
        "private-model",
        "private-status-api",
        "127.0.0.1",
        "sha256",
        "credential",
        "deployment",
        "ledger_revision",
        "token",
    ] {
        assert!(!text.contains(private), "{private}");
    }
    store.close().await;
}
