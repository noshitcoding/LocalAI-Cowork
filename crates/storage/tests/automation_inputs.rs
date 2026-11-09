use cowork_contracts::*;
use cowork_domain::{Error, workflows::WorkflowDefinition};
use cowork_storage::{Store, WorkflowLibraryEntry};
use cowork_workflow_contracts::RunRequest;
use serde_json::{Value, json};

struct Fixture {
    store: Store,
    project: Project,
    order: Order,
    task: WorkspaceTask,
    workflow: WorkflowLibraryEntry,
}
async fn fixture(store: Store) -> Fixture {
    let project = store
        .create_project(CreateProject {
            name: "Mapped events".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Typed event".into(),
        })
        .await
        .unwrap();
    let definition: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-dataflow.json"
    ))
    .unwrap();
    let profile:ProviderProfile=serde_json::from_value(json!({"id":definition.roles[0].provider_id,"name":"Fixture","kind":"test","endpoint":"","model":"fixture","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true})).unwrap();
    store.insert_provider(&profile).await.unwrap();
    let workflow = store
        .create_workflow(&project.id, "definition", &definition)
        .await
        .unwrap();
    let write:WriteWorkspaceTask=serde_json::from_value(json!({"expected_revision":0,"idempotency_key":"task","value":{
        "name":"Review file","description":"","order_id":order.id,"prompt":"Use declared inputs.","expected_output":"Checked result",
        "execution":{"kind":"workflow","workflow_id":workflow.id,"version_id":workflow.versions[0].id,
            "inputs":{"path":"saved.pdf","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","currency":"EUR","private_note":"fixed"}},
        "access_mode":"read_only","budgets":definition.budgets}})).unwrap();
    let task = store
        .write_workspace_task(&project.id, None, &write)
        .await
        .unwrap();
    Fixture {
        store,
        project,
        order,
        task,
        workflow,
    }
}
fn write(f: &Fixture, enabled: bool) -> WriteAutomationTrigger {
    serde_json::from_value(json!({"id":null,"expected_revision":0,"idempotency_key":"trigger","name":"File fields","task_id":f.task.id,"task_revision":f.task.revision,
        "source":"manual","predicates":[],"enabled":enabled,"workflow_inputs":{"format_version":1,"bindings":{"/path":"/file/name","/sha256":"/hash"}}})).unwrap()
}
fn data() -> Value {
    json!({"file":{"name":"event.pdf"},"hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","currency":"USD","unbound":"UNBOUND_EVENT_MARKER","native_tools":["execute_program"]})
}
fn event(id: &str, data: Value) -> SubmitAutomationEvent {
    SubmitAutomationEvent {
        event_id: id.into(),
        data,
    }
}

#[tokio::test]
async fn preview_is_read_only_works_while_paused_and_reports_mapping_errors() {
    let f = fixture(Store::connect("sqlite::memory:").await.unwrap()).await;
    let trigger = f
        .store
        .write_automation_trigger(&f.project.id, &write(&f, false))
        .await
        .unwrap();
    let good = f
        .store
        .preview_automation_event(&f.project.id, &trigger.id, &event("preview", data()))
        .await
        .unwrap();
    assert!(!good.would_queue);
    assert_eq!(good.reason.as_deref(), Some("trigger_paused"));
    assert_eq!(good.trigger_revision, Some(trigger.revision));
    assert_eq!(good.task_revision, Some(f.task.revision));
    let input = good.workflow_input.unwrap();
    assert_eq!(input.values["path"], "event.pdf");
    assert_eq!(input.values["currency"], "EUR");
    assert_eq!(input.version_id, f.workflow.versions[0].id);
    assert_eq!(input.definition_sha256, f.workflow.versions[0].sha256);
    let bad = f
        .store
        .preview_automation_event(
            &f.project.id,
            &trigger.id,
            &event("preview", json!({"file":{"name":true}})),
        )
        .await
        .unwrap();
    assert!(bad.workflow_input.is_none());
    assert!(bad.input_error.is_some());
    assert!(!bad.would_queue);
    assert!(
        f.store
            .automation_events(&f.project.id, &trigger.id, None, 100)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert!(f.store.runs(&f.order.id).await.unwrap().is_empty());
    f.store.close().await;
}
#[tokio::test]
async fn accepted_values_survive_reopen_claim_and_exact_original_ingress_replay() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let f = fixture(Store::sqlite(&path).await.unwrap()).await;
    let trigger = f
        .store
        .write_automation_trigger(&f.project.id, &write(&f, true))
        .await
        .unwrap();
    let input = event("original", data());
    let original = f
        .store
        .submit_automation_event(
            &f.project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &input,
        )
        .await
        .unwrap();
    assert_eq!(original.state, AutomationEventState::Queued);
    assert_eq!(
        original.workflow_input.as_ref().unwrap().values["path"],
        "event.pdf"
    );
    f.store.close().await;
    let reopened = Store::sqlite(&path).await.unwrap();
    let dispatch = reopened.claim_automation_event().await.unwrap().unwrap();
    let RunRequest::Workflow(request) = dispatch.request else {
        panic!("workflow")
    };
    assert_eq!(request.inputs.as_ref().unwrap()["path"], "event.pdf");
    assert_eq!(request.inputs.as_ref().unwrap()["currency"], "EUR");
    assert_eq!(request.budgets, f.task.value.budgets);
    assert_eq!(request.access_mode, AccessMode::ReadOnly);
    assert!(!request.prompt.contains("UNBOUND_EVENT_MARKER"));
    assert!(!request.prompt.contains("execute_program"));
    assert_eq!(
        serde_json::to_value(
            reopened
                .submit_automation_event(
                    &f.project.id,
                    &trigger.id,
                    AutomationEventSource::Manual,
                    &input
                )
                .await
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(original).unwrap()
    );
    assert!(reopened.claim_automation_event().await.unwrap().is_none());
    reopened.close().await;
}
#[tokio::test]
async fn missing_or_wrong_event_fields_are_durably_blocked_before_a_dispatch_request() {
    let f = fixture(Store::connect("sqlite::memory:").await.unwrap()).await;
    let trigger = f
        .store
        .write_automation_trigger(&f.project.id, &write(&f, true))
        .await
        .unwrap();
    for (id, data, code) in [
        ("missing", json!({}), "automation_inputs_value_missing"),
        (
            "type",
            json!({"file":{"name":42},"hash":"h"}),
            "automation_inputs_type_mismatch",
        ),
    ] {
        let input = event(id, data);
        let receipt = f
            .store
            .submit_automation_event(
                &f.project.id,
                &trigger.id,
                AutomationEventSource::Manual,
                &input,
            )
            .await
            .unwrap();
        assert_eq!(receipt.state, AutomationEventState::Blocked);
        assert_eq!(receipt.reason.as_deref(), Some(code));
        assert!(receipt.workflow_input.is_none());
        assert_eq!(
            serde_json::to_value(
                f.store
                    .submit_automation_event(
                        &f.project.id,
                        &trigger.id,
                        AutomationEventSource::Manual,
                        &input
                    )
                    .await
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(receipt).unwrap()
        );
    }
    assert!(f.store.claim_automation_event().await.unwrap().is_none());
    assert!(f.store.runs(&f.order.id).await.unwrap().is_empty());
    f.store.close().await;
}
#[tokio::test]
async fn changed_trigger_and_revoked_definition_cannot_reinterpret_queued_values() {
    for revoke in [false, true] {
        let f = fixture(Store::connect("sqlite::memory:").await.unwrap()).await;
        let mut write = write(&f, true);
        let trigger = f
            .store
            .write_automation_trigger(&f.project.id, &write)
            .await
            .unwrap();
        let original = f
            .store
            .submit_automation_event(
                &f.project.id,
                &trigger.id,
                AutomationEventSource::Manual,
                &event("queued", data()),
            )
            .await
            .unwrap();
        if revoke {
            f.store
                .revoke_workflow(&f.project.id, &f.workflow.id, 1, "revoke")
                .await
                .unwrap();
        } else {
            write.id = Some(trigger.id.clone());
            write.expected_revision = trigger.revision;
            write.idempotency_key = "changed".into();
            write
                .workflow_inputs
                .as_mut()
                .unwrap()
                .bindings
                .insert("/path".into(), "/other".into());
            f.store
                .write_automation_trigger(&f.project.id, &write)
                .await
                .unwrap();
        }
        assert!(f.store.claim_automation_event().await.unwrap().is_none());
        let row = f
            .store
            .automation_events(&f.project.id, &trigger.id, None, 100)
            .await
            .unwrap()
            .items
            .remove(0);
        assert_eq!(row.id, original.id);
        assert_eq!(row.state, AutomationEventState::Blocked);
        assert_eq!(
            row.reason.as_deref(),
            Some(if revoke {
                "automation_inputs_workflow_unavailable"
            } else {
                "trigger_changed"
            })
        );
        assert_eq!(
            serde_json::to_value(row.workflow_input).unwrap(),
            serde_json::to_value(original.workflow_input).unwrap()
        );
        f.store.close().await;
    }
}
#[tokio::test]
async fn changed_projection_cannot_replace_the_original_ingress_values() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let f = fixture(Store::sqlite(&path).await.unwrap()).await;
    let trigger = f
        .store
        .write_automation_trigger(&f.project.id, &write(&f, true))
        .await
        .unwrap();
    let original = f
        .store
        .submit_automation_event(
            &f.project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &event("queued", data()),
        )
        .await
        .unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    sqlx::query("UPDATE automation_events SET payload=json_set(payload,'$.workflow_input.values.path','changed.pdf') WHERE id=?").bind(&original.id).execute(&pool).await.unwrap();
    assert!(matches!(
        f.store.claim_automation_event().await,
        Err(Error::Forbidden)
    ));
    assert!(f.store.runs(&f.order.id).await.unwrap().is_empty());
    pool.close().await;
    f.store.close().await;
}
#[tokio::test]
async fn old_unmapped_trigger_keeps_its_original_request_shape_and_context() {
    let f = fixture(Store::connect("sqlite::memory:").await.unwrap()).await;
    let mut write = write(&f, true);
    write.workflow_inputs = None;
    let trigger = f
        .store
        .write_automation_trigger(&f.project.id, &write)
        .await
        .unwrap();
    let preview = f
        .store
        .preview_automation_event(&f.project.id, &trigger.id, &event("preview", data()))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(preview).unwrap(),
        json!({"matches":true,"would_queue":true,"reason":null})
    );
    let receipt = f
        .store
        .submit_automation_event(
            &f.project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &event("legacy", data()),
        )
        .await
        .unwrap();
    assert!(receipt.workflow_input.is_none());
    let dispatch = f.store.claim_automation_event().await.unwrap().unwrap();
    let RunRequest::Workflow(request) = dispatch.request else {
        panic!("workflow")
    };
    assert_eq!(request.inputs.unwrap()["path"], "saved.pdf");
    assert!(request.prompt.contains("UNBOUND_EVENT_MARKER"));
    f.store.close().await;
}
