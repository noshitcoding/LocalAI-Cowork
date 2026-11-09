use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use cowork_broker::{FileBroker, inspect_native_folder};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tower::ServiceExt;
const NATIVE: &str = "native-only-fixture-secret-012345678901234567890123456789";
const OWNER: &str = "Bearer ordinary-browser-fixture";

async fn setup() -> (
    tempfile::TempDir,
    axum::Router,
    Runtime,
    NativeFolderSelection,
) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("selected")).unwrap();
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Native folder fixture".into(),
        })
        .await
        .unwrap();
    let input = NativeFolderSelection {
        idempotency_key: "explicit-picker-choice".into(),
        project_id: project.id,
        name: "Selected folder".into(),
        path: temp.path().join("selected"),
        expected_resource_id: inspect_native_folder(&temp.path().join("selected"))
            .unwrap()
            .1,
        writable: false,
    };
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_file_broker(FileBroker::open(&temp.path().join("state"), vec![]).unwrap());
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new("ordinary-browser-fixture".into()),
            native_token: Some(Arc::new(NATIVE.into())),
            origins: Arc::new(vec!["http://127.0.0.1:5175".into()]),
        },
        None,
    );
    (temp, app, runtime, input)
}
async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: serde_json::Value,
    native: bool,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/api/v2{path}"))
        .header(header::AUTHORIZATION, OWNER)
        .header(header::CONTENT_TYPE, "application/json");
    if native {
        request = request.header("x-cowork-native", NATIVE);
    }
    app.clone()
        .oneshot(
            request
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn native_authority_is_required_before_body_poll_and_browser_origin_is_never_accepted() {
    let (_temp, app, _runtime, _input) = setup().await;
    for (owner, native, origin) in [
        (false, true, None),
        (true, false, None),
        (true, true, Some("http://127.0.0.1:5175")),
        (true, true, Some("https://hostile.invalid")),
    ] {
        let polled = Arc::new(AtomicBool::new(false));
        let seen = polled.clone();
        let stream = futures_util::stream::once(async move {
            seen.store(true, Ordering::SeqCst);
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"{}"))
        });
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v2/native/folders")
            .header(header::CONTENT_TYPE, "application/json");
        if owner {
            request = request.header(header::AUTHORIZATION, OWNER);
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
            StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
        ));
        assert!(!polled.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn native_registration_and_revocation_share_public_grants_without_exposing_native_paths() {
    let (temp, app, runtime, input) = setup().await;
    let response = call(
        &app,
        "POST",
        "/native/folders",
        serde_json::to_value(&input).unwrap(),
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let registered: RegisteredFolder =
        serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
    let response = call(
        &app,
        "GET",
        &format!("/folders?project_id={}", input.project_id),
        serde_json::Value::Null,
        false,
    )
    .await;
    let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
    let grants: Vec<FolderGrant> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(grants, vec![registered.grant.clone()]);
    assert!(!String::from_utf8_lossy(&bytes).contains("canonical_path"));
    assert!(!String::from_utf8_lossy(&bytes).contains(&temp.path().to_string_lossy().to_string()));
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/native/folders?project_id={}", input.project_id),
            serde_json::Value::Null,
            false
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let mut wrong = input.clone();
    wrong.project_id = "unknown".into();
    assert_eq!(
        call(
            &app,
            "POST",
            "/native/folders",
            serde_json::to_value(wrong).unwrap(),
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    wrong = input.clone();
    wrong.idempotency_key = "different-choice".into();
    wrong.expected_resource_id = "0".repeat(64);
    wrong.path = temp.path().join("other");
    std::fs::create_dir(&wrong.path).unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/native/folders",
            serde_json::to_value(wrong).unwrap(),
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let revocation = RevokeFolder {
        project_id: input.project_id.clone(),
        folder_id: registered.grant.id.clone(),
        expected_revision: registered.grant.revision.clone(),
    };
    assert_eq!(
        call(
            &app,
            "POST",
            "/native/folder-revocations",
            serde_json::to_value(&revocation).unwrap(),
            false
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for _ in 0..2 {
        assert_eq!(
            call(
                &app,
                "POST",
                "/native/folder-revocations",
                serde_json::to_value(&revocation).unwrap(),
                true
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    assert!(
        runtime
            .files
            .as_ref()
            .unwrap()
            .grants(&input.project_id)
            .unwrap()
            .is_empty()
    );
    let response = call(
        &app,
        "GET",
        &format!(
            "/native/folder-requests?project_id={}&key={}",
            input.project_id, input.idempotency_key
        ),
        serde_json::Value::Null,
        true,
    )
    .await;
    let lookup: FolderRequestLookup =
        serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
    assert!(lookup.folder.unwrap().revoked);
}
