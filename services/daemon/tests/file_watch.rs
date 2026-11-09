use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use cowork_broker::{FileBroker, NativeFolderSelection};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn setup() -> (
    tempfile::TempDir,
    axum::Router,
    Runtime,
    Project,
    WorkspaceTask,
    AutomationFileWatch,
) {
    let temp = tempfile::tempdir().unwrap();
    let inbox = temp.path().join("inbox");
    std::fs::create_dir(&inbox).unwrap();
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "File API".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Files".into(),
        })
        .await
        .unwrap();
    let broker = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
    let folder = broker
        .register_native_folder(&NativeFolderSelection {
            idempotency_key: "folder".into(),
            project_id: project.id.clone(),
            name: "Inbox".into(),
            path: inbox.clone(),
            expected_resource_id: cowork_broker::inspect_native_folder(&inbox).unwrap().1,
            writable: false,
        })
        .unwrap()
        .grant;
    let task=store.write_workspace_task(&project.id,None,&serde_json::from_value(json!({"expected_revision":0,"idempotency_key":"task","value":{"name":"Invoices","description":"","order_id":order.id,"prompt":"Process observed invoices","expected_output":"","execution":{"kind":"model","provider_id":"fixture","provider_revision":1},"access_mode":"read_only","folders":[folder],"budgets":Budgets::default()}})).unwrap()).await.unwrap();
    let watch = AutomationFileWatch {
        folder,
        path: ".".into(),
        recursive: false,
        changes: vec![FileWatchChange::Created],
        extensions: vec![".pdf".into()],
        exclude_paths: vec![],
        include_existing: false,
        debounce_ms: 1000,
        poll_interval_ms: 5000,
        max_files: 100,
        max_file_bytes: 1024,
        max_scan_bytes: 4096,
    };
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_file_broker(broker);
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new("owner".into()),
            native_token: None,
            origins: Arc::new(vec!["http://localhost:5175".into()]),
        },
        None,
    );
    (temp, app, runtime, project, task, watch)
}
async fn call(
    app: &axum::Router,
    path: &str,
    method: &str,
    owner: bool,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .uri(path)
        .method(method)
        .header(header::CONTENT_TYPE, "application/json");
    if owner {
        request = request.header(header::AUTHORIZATION, "Bearer owner");
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn file_watch_api_uses_real_folder_access_and_scopes_status_to_the_project() {
    let (_temp, app, runtime, project, task, watch) = setup().await;
    let path = format!("/api/v2/projects/{}/automation-triggers", project.id);
    let write = json!({"id":null,"expected_revision":0,"idempotency_key":"watch","name":"Watch invoices","task_id":task.id,"task_revision":task.revision,"source":"file","file_watch":watch,"predicates":[],"enabled":true});
    assert_eq!(
        call(&app, &path, "POST", false, write.clone()).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, trigger) = call(&app, &path, "POST", true, write).await;
    assert_eq!(status, StatusCode::OK);
    let id = trigger["id"].as_str().unwrap();
    let status_path = format!("{path}/{id}/file-watch");
    assert_eq!(
        call(&app, &status_path, "GET", false, json!(null)).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, value) = call(&app, &status_path, "GET", true, json!(null)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["initialized"], true);
    let other = runtime
        .store
        .create_project(CreateProject {
            name: "Other".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            &format!(
                "/api/v2/projects/{}/automation-triggers/{id}/file-watch",
                other.id
            ),
            "GET",
            true,
            json!(null)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // File source events can only come from the native observer.
    assert_eq!(
        call(
            &app,
            &format!("{path}/{id}/events"),
            "POST",
            true,
            json!({"event_id":"forged","data":{"path":"arbitrary.pdf"}})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn forged_replaced_and_cross_project_folder_bindings_are_rejected() {
    let (_temp, app, runtime, project, task, watch) = setup().await;
    let path = format!("/api/v2/projects/{}/automation-triggers", project.id);
    for (key, folder) in [
        ("fake-id", json!({"id":"fake"})),
        ("fake-resource", json!({"resource_id":"replacement"})),
        ("wrong-project", json!({"project_id":"other"})),
    ] {
        let mut forged = serde_json::to_value(&watch).unwrap();
        for (name, value) in folder.as_object().unwrap() {
            forged["folder"][name] = value.clone();
        }
        let write = json!({"id":null,"expected_revision":0,"idempotency_key":key,"name":"Invalid watch","task_id":task.id,"task_revision":task.revision,"source":"file","file_watch":forged,"predicates":[],"enabled":true});
        assert_eq!(
            call(&app, &path, "POST", true, write).await.0,
            StatusCode::FORBIDDEN
        );
    }
    assert!(
        runtime
            .store
            .automation_triggers(&project.id)
            .await
            .unwrap()
            .is_empty()
    );
}
