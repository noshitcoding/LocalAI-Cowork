#[path = "../../../crates/storage/tests/support/native_keys.rs"]
mod native_keys;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use native_keys::*;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tower::ServiceExt;
const NATIVE: &str = "native-setup-fixture-0123456789012345678901234567890123456789";
async fn setup() -> (axum::Router, Store, Arc<FakeProvider>) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let provider = Arc::new(FakeProvider::default());
    let runtime = Runtime::new(store.clone(), vec![], false)
        .unwrap()
        .with_native_key_provider(provider.clone());
    let app = router(
        AppState {
            runtime,
            token: Arc::new("fixture-owner".into()),
            native_token: Some(Arc::new(NATIVE.into())),
            origins: Arc::new(vec!["http://127.0.0.1:5175".into()]),
        },
        None,
    );
    (app, store, provider)
}
async fn call(app: &axum::Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/api/v2/native/secrets{path}"))
                .header(header::AUTHORIZATION, "Bearer fixture-owner")
                .header("x-cowork-native", NATIVE)
                .header(header::CONTENT_TYPE, "application/json")
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    (
        response.status(),
        serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap(),
    )
}
#[tokio::test]
async fn native_setup_requires_both_authorities_and_rejects_browser_origin_before_body() {
    let (app, store, provider) = setup().await;
    for path in ["", "/reconcile", "/abandon"] {
        for (owner, native, origin) in [
            (false, true, None),
            (true, false, None),
            (true, true, Some("http://127.0.0.1:5175")),
            (true, true, Some("https://hostile.test")),
        ] {
            let polled = Arc::new(AtomicBool::new(false));
            let seen = polled.clone();
            let stream = futures_util::stream::once(async move {
                seen.store(true, Ordering::SeqCst);
                Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"{}"))
            });
            let mut request = Request::builder()
                .method("POST")
                .uri(format!("/api/v2/native/secrets{path}"))
                .header(header::CONTENT_TYPE, "application/json");
            if owner {
                request = request.header(header::AUTHORIZATION, "Bearer fixture-owner");
            }
            if native {
                request = request.header("x-cowork-native", NATIVE);
            }
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::from_stream(stream)).unwrap())
                .await
                .unwrap();
            assert!(matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ));
            assert!(!polled.load(Ordering::SeqCst));
        }
    }
    assert!(store.native_secret_state().await.unwrap().is_none());
    assert_eq!(provider.keys.creates.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn native_setup_status_request_replay_and_resolution_never_expose_key_material() {
    let (app, store, provider) = setup().await;
    let (code, empty) = call(&app, "GET", "", json!(null)).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(empty["configured"], false);
    assert!(store.native_secret_state().await.unwrap().is_none());
    let (code, _) = call(
        &app,
        "POST",
        "",
        json!({"idempotency_key":"first","base_revision":0,"key":"caller-chosen"}),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let input = json!({"idempotency_key":"first","base_revision":0});
    let (code, ready) = call(&app, "POST", "", input.clone()).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(ready["store"]["key_available"], true);
    let (code, repeated) = call(&app, "POST", "", input).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(repeated, ready);
    assert_eq!(provider.keys.creates.load(Ordering::SeqCst), 1);
    let (code, lookup) = call(&app, "GET", "/requests?key=first", json!(null)).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(lookup["receipt"], ready);
    let operation = store.native_key_request("first").await.unwrap().unwrap();
    let public = ready.to_string();
    assert!(!public.contains(&operation.key.cipher_id().unwrap()));
    assert!(!public.contains(&operation.key.instance.to_string()));
    assert!(!public.contains("tag"));
    let action = json!({"operation_id":operation.id,"base_revision":1});
    let (code, _) = call(&app, "POST", "/abandon", action).await;
    assert_eq!(code, StatusCode::CONFLICT);
    provider
        .keys
        .fail_after_create
        .store(true, Ordering::SeqCst);
    let rotation = json!({"idempotency_key":"rotation","base_revision":2});
    let (code, error) = call(&app, "POST", "", rotation.clone()).await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert_eq!(error["code"], "uncertain_effect");
    let (code, pending) = call(&app, "POST", "", rotation).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(pending["operation_state"], "pending");
    let action = json!({"operation_id":pending["operation_id"],"base_revision":pending["store"]["revision"]});
    let (code, finished) = call(&app, "POST", "/reconcile", action).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(finished["operation_state"], "activated");
    let parsed: NativeSecretsReceipt = serde_json::from_value(finished).unwrap();
    assert_eq!(parsed.store.revision, 4);
    assert_eq!(provider.keys.creates.load(Ordering::SeqCst), 2);
}
