use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn fixture() -> (axum::Router, Runtime, Project, WorkspaceTask) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "HTTP automation".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Task".into(),
        })
        .await
        .unwrap();
    let write=serde_json::from_value(json!({"expected_revision":0,"idempotency_key":"task","value":{"name":"Task","description":"","order_id":order.id,"prompt":"Review data","expected_output":"","execution":{"kind":"model","provider_id":"missing","provider_revision":1},"access_mode":"read_only","budgets":Budgets::default()}})).unwrap();
    let task = store
        .write_workspace_task(&project.id, None, &write)
        .await
        .unwrap();
    let runtime = Runtime::new(store, vec![], false).unwrap();
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new("owner-test-token".into()),
            native_token: None,
            origins: Arc::new(vec!["http://localhost:5175".into()]),
        },
        None,
    );
    (app, runtime, project, task)
}
async fn call(
    app: &axum::Router,
    path: &str,
    method: &str,
    token: Option<&str>,
    origin: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .uri(path)
        .method(method)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
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
async fn mapped_event_preview_and_ingress_freeze_values_without_admitting_a_run() {
    let (app, runtime, project, task) = typed_fixture().await;
    let path = format!("/api/v2/projects/{}/automation-triggers", project.id);
    let mut write = json!({"id":null,"expected_revision":0,"idempotency_key":"mapped-trigger","name":"Typed event","task_id":task.id,"task_revision":task.revision,"source":"manual","predicates":[],"enabled":false,
        "workflow_inputs":{"format_version":1,"bindings":{"/path":"/filename","/sha256":"/hash"}}});
    let (status, trigger) = call(
        &app,
        &path,
        "POST",
        Some("owner-test-token"),
        None,
        write.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{trigger}");
    let base = format!("{path}/{}", trigger["id"].as_str().unwrap());
    let input = json!({"event_id":"sample","data":{"filename":"event.pdf","hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","currency":"USD","unbound":"ONLY_IN_ORIGINAL_EVENT_DATA"}});
    let (status, preview) = call(
        &app,
        &format!("{base}/preview"),
        "POST",
        Some("owner-test-token"),
        None,
        input.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["would_queue"], false);
    assert_eq!(preview["reason"], "trigger_paused");
    assert_eq!(preview["workflow_input"]["values"]["path"], "event.pdf");
    assert_eq!(preview["workflow_input"]["values"]["currency"], "EUR");
    assert_eq!(preview["task_revision"], task.revision);
    write["id"] = trigger["id"].clone();
    write["expected_revision"] = trigger["revision"].clone();
    write["idempotency_key"] = json!("activate");
    write["enabled"] = json!(true);
    assert_eq!(
        call(&app, &path, "POST", Some("owner-test-token"), None, write)
            .await
            .0,
        StatusCode::OK
    );
    let mut invalid = input.clone();
    invalid["data"]["filename"] = json!(42);
    let (status, preview) = call(
        &app,
        &format!("{base}/preview"),
        "POST",
        Some("owner-test-token"),
        None,
        invalid.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["would_queue"], false);
    assert_eq!(preview["input_error"], "automation_inputs_type_mismatch");
    invalid["event_id"] = json!("invalid");
    let (_, blocked) = call(
        &app,
        &format!("{base}/events"),
        "POST",
        Some("owner-test-token"),
        None,
        invalid,
    )
    .await;
    assert_eq!(blocked["state"], "blocked");
    assert_eq!(blocked["reason"], "automation_inputs_type_mismatch");
    let mut forged = input.clone();
    forged["workflow_input"] = json!({"values":{"path":"forged.pdf"}});
    assert_eq!(
        call(
            &app,
            &format!("{base}/events"),
            "POST",
            Some("owner-test-token"),
            None,
            forged
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, original) = call(
        &app,
        &format!("{base}/events"),
        "POST",
        Some("owner-test-token"),
        None,
        input.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(original["state"], "queued");
    assert_eq!(original["workflow_input"]["values"]["path"], "event.pdf");
    assert!(
        !original["workflow_input"]
            .to_string()
            .contains("ONLY_IN_ORIGINAL_EVENT_DATA")
    );
    assert_eq!(
        call(
            &app,
            &format!("{base}/events"),
            "POST",
            Some("owner-test-token"),
            None,
            input
        )
        .await,
        (StatusCode::OK, original)
    );
    assert!(
        runtime
            .store
            .runs(&task.value.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    runtime.store.close().await;
}

async fn typed_fixture() -> (axum::Router, Runtime, Project, WorkspaceTask) {
    let (app, runtime, project, task) = fixture().await;
    let definition: cowork_domain::workflows::WorkflowDefinition = serde_json::from_str(
        include_str!("../../../tests/fixtures/workflow-dataflow.json"),
    )
    .unwrap();
    let profile:ProviderProfile=serde_json::from_value(json!({"id":definition.roles[0].provider_id,"name":"Fixture","kind":"test","endpoint":"","model":"fixture","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true})).unwrap();
    runtime.store.insert_provider(&profile).await.unwrap();
    let workflow = runtime
        .store
        .create_workflow(&project.id, "definition", &definition)
        .await
        .unwrap();
    let mut value = task.value;
    value.execution=serde_json::from_value(json!({"kind":"workflow","workflow_id":workflow.id,"version_id":workflow.versions[0].id,"inputs":{"path":"saved.pdf","sha256":"hash","currency":"EUR","private_note":"fixed"}})).unwrap();
    value.budgets = definition.budgets;
    let task=runtime.store.write_workspace_task(&project.id,Some(&task.id),&serde_json::from_value(json!({"expected_revision":task.revision,"idempotency_key":"typed-task","value":value})).unwrap()).await.unwrap();
    (app, runtime, project, task)
}

#[tokio::test]
async fn invalid_mapping_or_untyped_task_cannot_create_an_automation_binding() {
    for typed in [false, true] {
        let (app, runtime, project, task) = if typed {
            typed_fixture().await
        } else {
            fixture().await
        };
        let path = format!("/api/v2/projects/{}/automation-triggers", project.id);
        let body = json!({"id":null,"expected_revision":0,"idempotency_key":"invalid-mapping","name":"Invalid mapping","task_id":task.id,"task_revision":task.revision,"source":"manual","predicates":[],"enabled":true,"workflow_inputs":{"format_version":1,"bindings":if typed{json!({"/missing":"/source"})}else{json!({})}}});
        let (status, error) = call(&app, &path, "POST", Some("owner-test-token"), None, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(
            error["code"],
            if typed {
                "automation_inputs_invalid"
            } else {
                "automation_inputs_workflow_required"
            }
        );
        assert!(
            runtime
                .store
                .automation_triggers(&project.id)
                .await
                .unwrap()
                .is_empty()
        );
        runtime.store.close().await;
    }
}

#[tokio::test]
async fn webhook_has_its_own_scoped_auth_and_owner_management_stays_protected() {
    let (app, runtime, project, task) = fixture().await;
    let path = format!("/api/v2/projects/{}/automation-triggers", project.id);
    let write = json!({"id":null,"expected_revision":0,"idempotency_key":"trigger","name":"Webhook","task_id":task.id,"task_revision":task.revision,"source":"webhook","predicates":[],"enabled":true});
    assert_eq!(
        call(&app, &path, "POST", None, None, write.clone()).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            &path,
            "POST",
            Some("owner-test-token"),
            Some("https://hostile.invalid"),
            write.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, trigger) = call(&app, &path, "POST", Some("owner-test-token"), None, write).await;
    assert_eq!(status, StatusCode::OK);
    let trigger_id = trigger["id"].as_str().unwrap();
    let token = "ac".repeat(32);
    let grant_path = format!("{path}/{trigger_id}/webhook");
    let grant = json!({"expected_revision":0,"idempotency_key":"grant","token":token,"enabled":true,"expires_at":cowork_storage::now()+60_000});
    let (status, metadata) = call(
        &app,
        &grant_path,
        "PUT",
        Some("owner-test-token"),
        None,
        grant,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!metadata.to_string().contains(&token));
    let endpoint = format!("/api/automation-webhooks/{trigger_id}");
    let input = json!({"event_id":"delivery-1","data":{"amount":42}});
    for bearer in [None, Some("owner-test-token"), Some("wrong")] {
        assert_eq!(
            call(&app, &endpoint, "POST", bearer, None, input.clone())
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        call(
            &app,
            &endpoint,
            "POST",
            Some(&token),
            Some("http://localhost:5175"),
            input.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            &format!("{endpoint}?token={token}"),
            "POST",
            None,
            None,
            input.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, accepted) = call(&app, &endpoint, "POST", Some(&token), None, input.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(&app, &endpoint, "POST", Some(&token), None, input)
            .await
            .1,
        accepted
    );
    assert_eq!(runtime.automation_tick().await.unwrap(), 0); // Missing provider blocks visibly.
    let page = runtime
        .store
        .automation_events(&project.id, trigger_id, None, 50)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].state, AutomationEventState::Blocked);
}

#[tokio::test]
async fn preview_never_enqueues_and_event_body_limits_apply() {
    let (app, runtime, project, task) = fixture().await;
    let trigger = runtime
        .store
        .write_automation_trigger(
            &project.id,
            &WriteAutomationTrigger {
                workflow_inputs: None,
                id: None,
                expected_revision: 0,
                idempotency_key: "manual".into(),
                name: "Manual".into(),
                task_id: task.id,
                task_revision: task.revision,
                source: AutomationEventSource::Manual,
                file_watch: None,
                predicates: vec![],
                enabled: true,
            },
        )
        .await
        .unwrap();
    let base = format!(
        "/api/v2/projects/{}/automation-triggers/{}",
        project.id, trigger.id
    );
    let (status, preview) = call(
        &app,
        &format!("{base}/preview"),
        "POST",
        Some("owner-test-token"),
        None,
        json!({"event_id":"preview","data":{"amount":1}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["would_queue"], true);
    assert!(
        runtime
            .store
            .automation_events(&project.id, &trigger.id, None, 50)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        call(
            &app,
            &format!("{base}/events"),
            "POST",
            Some("owner-test-token"),
            None,
            json!({"event_id":"large","data":{"text":"x".repeat(60_000)}})
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}
