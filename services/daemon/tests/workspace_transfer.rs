use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_domain::{hash, workspace_transfer as transfer};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

async fn get(
    app: &axum::Router,
    path: &str,
    authenticated: bool,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut request = Request::builder().uri(path);
    if authenticated {
        request = request.header("Authorization", "Bearer private-transfer-owner");
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    (
        status,
        headers,
        to_bytes(response.into_body(), 5 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
}

#[tokio::test]
async fn completed_workspace_transfer_is_authenticated_source_scoped_and_repeatable() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Transfer API".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Retained output".into(),
        })
        .await
        .unwrap();
    let other = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "Other output".into(),
        })
        .await
        .unwrap();
    let profile:ProviderProfile=serde_json::from_value(serde_json::json!({"id":"fixture","name":"Fixture","kind":"test","model":"test","endpoint":"","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true})).unwrap();
    store.insert_provider(&profile).await.unwrap();
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        true,
    )
    .unwrap()
    .with_file_broker(cowork_broker::FileBroker::open(directory.path(), vec![]).unwrap());
    runtime.workspace(&order.id).await.unwrap();
    let original = vec![39u8; transfer::CHUNK_BYTES as usize + 3];
    let source = directory
        .path()
        .join("workspaces")
        .join(&order.id)
        .join("files/payload.bin");
    std::fs::write(&source, &original).unwrap();
    let input:CreateRun=serde_json::from_value(serde_json::json!({"order_id":order.id,"base_revision":order.revision,"idempotency_key":"transfer-once","prompt":"Create the deterministic result.","provider_id":"fixture","provider_revision":1,"access_mode":"autonomous"})).unwrap();
    let run = runtime.create_run(&input).await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(runtime.clone().serve(1, stop.clone()));
    let mut completed = false;
    for _ in 0..500 {
        let record = runtime.store.detail(&run.id).await.unwrap();
        if record.run.state == RunState::Completed {
            completed = true;
            break;
        }
        if matches!(
            record.run.state,
            RunState::Failed | RunState::Interrupted | RunState::Expired
        ) {
            panic!("Run failed: {:?}", record.run.error_code);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    stop.cancel();
    worker.await.unwrap();
    assert!(completed);
    std::fs::write(&source, b"live source changed after completion").unwrap();
    let transfer_control = runtime.clone();
    let app = router(
        AppState {
            runtime,
            token: Arc::new("private-transfer-owner".into()),
            native_token: None,
            origins: Arc::new(vec![]),
        },
        None,
    );
    let path = format!(
        "/api/v2/orders/{}/runs/{}/workspace/transfer",
        order.id, run.id
    );
    assert_eq!(get(&app, &path, false).await.0, StatusCode::UNAUTHORIZED);
    let first = transfer_control.begin_upload_transfer().unwrap();
    let second = transfer_control.begin_upload_transfer().unwrap();
    let busy = get(&app, &path, true).await;
    assert_eq!(busy.0, StatusCode::CONFLICT);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&busy.2).unwrap()["code"],
        "transfer_busy"
    );
    drop(first);
    drop(second);
    let response = get(&app, &path, true).await;
    assert_eq!(
        response.0,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&response.2)
    );
    let manifest: WorkspaceTransferManifest = serde_json::from_slice(&response.2).unwrap();
    transfer::validate(&manifest).unwrap();
    assert_eq!(manifest.files[0].sha256, hash(&original));
    let chunk = &manifest.files[0].chunks[1];
    let chunk_path = format!(
        "{path}/chunk?path=payload.bin&offset={}&expected_sha256={}",
        chunk.offset, chunk.sha256
    );
    assert_eq!(
        get(&app, &chunk_path, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    let data = get(&app, &chunk_path, true).await;
    assert_eq!(data.0, StatusCode::OK);
    assert_eq!(data.2, vec![39u8; 3]);
    assert_eq!(data.1["x-cowork-sha256"], chunk.sha256.as_str());
    assert_eq!(get(&app, &chunk_path, true).await.2, data.2);
    assert_eq!(
        get(&app, &path.replace(&order.id, &other.id), true).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(
            &app,
            &chunk_path.replace(&chunk.sha256, &"0".repeat(64)),
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"live source changed after completion"
    );
}
