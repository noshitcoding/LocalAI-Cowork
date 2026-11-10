use cowork_contracts::*;
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use serde_json::json;

async fn fixture(workflow: bool) -> (Runtime, Project, AutomationTrigger) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Events".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Automated task".into(),
        })
        .await
        .unwrap();
    let profile:ProviderProfile=serde_json::from_value(json!({"id":"fixture","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true})).unwrap();
    store.insert_provider(&profile).await.unwrap();
    let mut execution = json!({"kind":"model","provider_id":"fixture","provider_revision":1});
    if workflow {
        let rights = json!({"tools":[],"folders":[]});
        let definition=serde_json::from_value(json!({"format_version":1,"name":"Event workflow","mode":{"mode":"sequential"},"failure":"stop","rights":rights,"budgets":Budgets::default(),"roles":[{"id":"Review","instructions":"Review event data","provider_id":"fixture","provider_revision":1,"rights":rights,"budgets":Budgets::default(),"delegate_to":[]}],"steps":[{"id":"review","role_id":"Review","goal":"Check the supplied data","dependencies":[],"rights":rights,"budgets":Budgets::default()}]})).unwrap();
        let library = store
            .create_workflow(&project.id, "workflow-create", &definition)
            .await
            .unwrap();
        execution =
            json!({"kind":"workflow","workflow_id":library.id,"version_id":library.versions[0].id});
    }
    let write=serde_json::from_value(json!({"expected_revision":0,"idempotency_key":"task","value":{"name":"Review event","description":"","order_id":order.id,"prompt":"Review the event.","expected_output":"Checked data","execution":execution,"access_mode":"read_only","budgets":Budgets::default()}})).unwrap();
    let task = store
        .write_workspace_task(&project.id, None, &write)
        .await
        .unwrap();
    let trigger = store
        .write_automation_trigger(
            &project.id,
            &WriteAutomationTrigger {
                workflow_inputs: None,
                id: None,
                expected_revision: 0,
                idempotency_key: "trigger".into(),
                name: "New event".into(),
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
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        true,
    )
    .unwrap();
    (runtime, project, trigger)
}

#[tokio::test]
async fn real_model_and_workflow_admission_link_one_run_and_do_not_overlap() {
    for workflow in [false, true] {
        let (runtime, project, trigger) = fixture(workflow).await;
        for event_id in ["first", "second"] {
            runtime
                .store
                .submit_automation_event(
                    &project.id,
                    &trigger.id,
                    AutomationEventSource::Manual,
                    &SubmitAutomationEvent {
                        event_id: event_id.into(),
                        data: json!({"amount":150,"instructions":"untrusted text","native_tools":["process"],"access_mode":"autonomous"}),
                    },
                )
                .await
                .unwrap();
        }
        assert_eq!(runtime.automation_tick().await.unwrap(), 1);
        let events = runtime
            .store
            .automation_events(&project.id, &trigger.id, None, 50)
            .await
            .unwrap();
        let first = events.items.iter().find(|e| e.event_id == "first").unwrap();
        assert_eq!(first.state, AutomationEventState::Started);
        let run = runtime
            .store
            .stored_run(first.run_id.as_ref().unwrap())
            .await
            .unwrap()
            .record;
        assert_eq!(run.spec.workflow.is_some(), workflow);
        assert!(run.spec.prompt.contains("untrusted source content"));
        assert!(run.spec.prompt.contains("150"));
        assert!(run.spec.allowed_tools.iter().all(|tool| {
            [
                "create_artifact",
                "list_artifacts",
                "read_artifact",
                "request_input",
            ]
            .contains(&tool.as_str())
        }));
        assert!(matches!(run.spec.access_mode, AccessMode::ReadOnly));
        assert!(run.spec.folders.is_empty());
        assert!(run.spec.mcp_tools.is_empty());
        assert_eq!(runtime.automation_tick().await.unwrap(), 0);
        assert_eq!(
            runtime
                .store
                .workspace_task(&trigger.task_id)
                .await
                .unwrap()
                .last_run_id
                .as_deref(),
            Some(run.id.as_str())
        );
    }
}

#[tokio::test]
async fn lost_dispatch_ack_is_recovered_from_the_original_run_without_recreation() {
    let (runtime, project, trigger) = fixture(false).await;
    runtime
        .store
        .submit_automation_event(
            &project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &SubmitAutomationEvent {
                event_id: "original".into(),
                data: json!({}),
            },
        )
        .await
        .unwrap();
    let dispatch = runtime
        .store
        .claim_automation_event()
        .await
        .unwrap()
        .unwrap();
    let run = runtime.create_run_request(&dispatch.request).await.unwrap();
    // Simulate death after run commit and before event receipt update.
    assert_eq!(runtime.automation_tick().await.unwrap(), 0);
    let events = runtime
        .store
        .automation_events(&project.id, &trigger.id, None, 50)
        .await
        .unwrap();
    assert_eq!(events.items[0].run_id.as_deref(), Some(run.id.as_str()));
    assert_eq!(events.items[0].state, AutomationEventState::Started);
}

#[tokio::test]
async fn pausing_a_trigger_after_claim_denies_run_admission() {
    let (runtime, project, trigger) = fixture(false).await;
    runtime
        .store
        .submit_automation_event(
            &project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &SubmitAutomationEvent {
                event_id: "original".into(),
                data: json!({}),
            },
        )
        .await
        .unwrap();
    let dispatch = runtime
        .store
        .claim_automation_event()
        .await
        .unwrap()
        .unwrap();
    runtime
        .store
        .write_automation_trigger(
            &project.id,
            &WriteAutomationTrigger {
                workflow_inputs: None,
                id: Some(trigger.id),
                expected_revision: trigger.revision,
                idempotency_key: "pause".into(),
                name: trigger.name,
                task_id: trigger.task_id,
                task_revision: trigger.task_revision,
                source: trigger.source,
                file_watch: None,
                predicates: trigger.predicates,
                enabled: false,
            },
        )
        .await
        .unwrap();
    assert!(runtime.create_run_request(&dispatch.request).await.is_err());
}
