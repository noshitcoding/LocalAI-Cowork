use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_domain::hash;
use cowork_runtime::Runtime;
use cowork_storage::Store;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
const TOKEN: &str = "Bearer upload-fixture-token";

async fn fixture() -> (tempfile::TempDir, axum::Router, Runtime, Order) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Upload HTTP".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "Upload target".into(),
        })
        .await
        .unwrap();
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_file_broker(cowork_broker::FileBroker::open(temp.path(), vec![]).unwrap());
    let app = router(
        AppState {
            native_token: None,
            runtime: runtime.clone(),
            token: Arc::new("upload-fixture-token".into()),
            origins: Arc::new(vec!["http://127.0.0.1:5175".into()]),
        },
        None,
    );
    (temp, app, runtime, order)
}
async fn reserve(app: &axum::Router, order: &str, bytes: &[u8]) -> FileUpload {
    let body = json!({"idempotency_key":cowork_storage::id(),"file":{"file_name":"Prüfung 📄.bin","size":bytes.len(),"sha256":hash(bytes)}});
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v2/orders/{order}/uploads"))
                .header(header::AUTHORIZATION, TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap()
}
async fn put(
    app: &axum::Router,
    order: &str,
    upload: &str,
    bytes: Vec<u8>,
    media: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v2/orders/{order}/uploads/{upload}/content"))
                .header(header::AUTHORIZATION, TOKEN)
                .header(header::CONTENT_TYPE, media)
                .body(Body::from(bytes))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn upload_body_auth_integrity_scope_empty_and_binary_contract() {
    let (_temp, app, runtime, order) = fixture().await;
    let bytes: Vec<u8> = (0..=255).cycle().take(2 * 1024 * 1024).collect();
    let upload = reserve(&app, &order.id, &bytes).await;
    let path = format!("/api/v2/orders/{}/uploads/{}/content", order.id, upload.id);
    let polled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = polled.clone();
    let body = Body::from_stream(futures_util::stream::once(async move {
        observed.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, std::io::Error>(vec![0u8; 4])
    }));
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(&path)
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
    let hostile = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(&path)
                .header(header::AUTHORIZATION, TOKEN)
                .header(header::ORIGIN, "https://hostile.invalid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hostile.status(), StatusCode::FORBIDDEN);
    for (body, media) in [
        (vec![0; bytes.len() - 1], "application/octet-stream"),
        (vec![0; bytes.len() + 1], "application/octet-stream"),
        (vec![0; bytes.len()], "application/octet-stream"),
        (bytes.clone(), "text/plain"),
    ] {
        let response = put(&app, &order.id, &upload.id, body, media).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error: ApiError =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(error.code, "invalid_request");
    }
    assert_eq!(
        runtime
            .store
            .upload(&order.id, &upload.id)
            .await
            .unwrap()
            .state,
        UploadState::Reserved
    );
    let foreign = runtime
        .store
        .create_order(CreateOrder {
            project_id: order.project_id.clone(),
            title: "Foreign".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        put(
            &app,
            &foreign.id,
            &upload.id,
            bytes.clone(),
            "application/octet-stream"
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    for _ in 0..2 {
        let response = put(
            &app,
            &order.id,
            &upload.id,
            bytes.clone(),
            "application/octet-stream",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let ready: FileUpload =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(ready.state, UploadState::Ready);
        assert_eq!(ready.revision, 2);
    }
    assert!(
        runtime
            .workspace_files(&order.id, ".".into())
            .await
            .unwrap()
            .entries
            .is_empty()
    );
    let empty = reserve(&app, &order.id, b"").await;
    assert_eq!(
        put(
            &app,
            &order.id,
            &empty.id,
            vec![],
            "application/octet-stream"
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn invalid_reservation_capacity_and_truncated_stream_do_not_confirm_upload() {
    let (_temp, app, runtime, order) = fixture().await;
    let upload = reserve(&app, &order.id, b"abcd").await;
    for file in [
        json!({"file_name":"../outside","size":0,"sha256":hash(b"")}),
        json!({"file_name":"large.bin","size":67108865u64,"sha256":hash(b"")}),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v2/orders/{}/uploads", order.id))
                    .header(header::AUTHORIZATION, TOKEN)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"idempotency_key":"bad","file":file}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let one = runtime.begin_upload_transfer().unwrap();
    let two = runtime.begin_upload_transfer().unwrap();
    let response = put(
        &app,
        &order.id,
        &upload.id,
        b"abcd".to_vec(),
        "application/octet-stream",
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ApiError =
        serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
    assert_eq!(error.code, "transfer_busy");
    drop(one);
    drop(two);
    let body = Body::from_stream(futures_util::stream::iter([
        Ok(vec![b'a']),
        Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "fixture interruption",
        )),
    ]));
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v2/orders/{}/uploads/{}/content",
                    order.id, upload.id
                ))
                .header(header::AUTHORIZATION, TOKEN)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        runtime
            .store
            .upload(&order.id, &upload.id)
            .await
            .unwrap()
            .state,
        UploadState::Reserved
    );
}
