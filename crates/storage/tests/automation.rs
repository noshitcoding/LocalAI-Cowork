use cowork_contracts::*;
use cowork_domain::Error;
use cowork_storage::Store;
use serde_json::json;

async fn fixture(
    store: &Store,
    source: AutomationEventSource,
) -> (Project, WorkspaceTask, AutomationTrigger) {
    let project = store
        .create_project(CreateProject {
            name: "Automation".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Events".into(),
        })
        .await
        .unwrap();
    let write = serde_json::from_value(json!({"expected_revision":0,"idempotency_key":"task-create","value":{
        "name":"Review invoice","description":"","order_id":order.id,"prompt":"Review supplied invoice data.","expected_output":"A checked result",
        "execution":{"kind":"model","provider_id":"fixture","provider_revision":1},"access_mode":"read_only","budgets":Budgets::default()
    }})).unwrap();
    let task = store
        .write_workspace_task(&project.id, None, &write)
        .await
        .unwrap();
    let trigger = store
        .write_automation_trigger(&project.id, &write_trigger(&task, source))
        .await
        .unwrap();
    (project, task, trigger)
}
fn write_trigger(task: &WorkspaceTask, source: AutomationEventSource) -> WriteAutomationTrigger {
    WriteAutomationTrigger {
        workflow_inputs: None,
        id: None,
        expected_revision: 0,
        idempotency_key: "trigger-create".into(),
        name: "Invoices".into(),
        task_id: task.id.clone(),
        task_revision: task.revision,
        source,
        file_watch: None,
        predicates: vec![],
        enabled: true,
    }
}
fn event(key: &str) -> SubmitAutomationEvent {
    SubmitAutomationEvent {
        event_id: key.into(),
        data: json!({"amount":150,"vendor":"Acme"}),
    }
}

#[tokio::test]
async fn duplicate_delivery_is_stable_after_claim_and_changed_data_conflicts() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, _, trigger) = fixture(&store, AutomationEventSource::Manual).await;
    let submitted = store
        .submit_automation_event(
            &project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &event("invoice-1"),
        )
        .await
        .unwrap();
    assert!(store.claim_automation_event().await.unwrap().is_some());
    for _ in 0..100 {
        let replay = store
            .submit_automation_event(
                &project.id,
                &trigger.id,
                AutomationEventSource::Manual,
                &event("invoice-1"),
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&replay).unwrap(),
            serde_json::to_value(&submitted).unwrap()
        );
    }
    let mut altered = event("invoice-1");
    altered.data = json!({"amount":999});
    assert!(matches!(
        store
            .submit_automation_event(
                &project.id,
                &trigger.id,
                AutomationEventSource::Manual,
                &altered
            )
            .await,
        Err(Error::Conflict)
    ));
    let page = store
        .automation_events(&project.id, &trigger.id, None, 50)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].state, AutomationEventState::Dispatching);
    assert!(store.claim_automation_event().await.unwrap().is_none());
}

#[tokio::test]
async fn preview_is_read_only_and_filter_matches_actual_admission() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, task, trigger) = fixture(&store, AutomationEventSource::Manual).await;
    let mut write = write_trigger(&task, AutomationEventSource::Manual);
    write.id = Some(trigger.id.clone());
    write.expected_revision = trigger.revision;
    write.idempotency_key = "filter".into();
    write.predicates = vec![EventPredicate::GreaterThan {
        pointer: "/amount".into(),
        value: 200.0,
    }];
    store
        .write_automation_trigger(&project.id, &write)
        .await
        .unwrap();
    assert!(
        !store
            .preview_automation_event(&project.id, &trigger.id, &event("preview"))
            .await
            .unwrap()
            .matches
    );
    assert!(
        store
            .automation_events(&project.id, &trigger.id, None, 50)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let filtered = store
        .submit_automation_event(
            &project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &event("filtered"),
        )
        .await
        .unwrap();
    assert_eq!(filtered.state, AutomationEventState::Filtered);
    assert!(store.claim_automation_event().await.unwrap().is_none());
}

#[tokio::test]
async fn task_changes_block_pending_events_and_original_trigger_saves_replay() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, task, trigger) = fixture(&store, AutomationEventSource::Manual).await;
    store
        .submit_automation_event(
            &project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &event("pending"),
        )
        .await
        .unwrap();
    let mut edited = task.value.clone();
    edited.prompt = "Changed instructions".into();
    store
        .write_workspace_task(
            &project.id,
            Some(&task.id),
            &WriteWorkspaceTask {
                expected_revision: task.revision,
                idempotency_key: "task-edit".into(),
                value: edited,
            },
        )
        .await
        .unwrap();
    assert!(store.claim_automation_event().await.unwrap().is_none());
    let page = store
        .automation_events(&project.id, &trigger.id, None, 50)
        .await
        .unwrap();
    assert_eq!(page.items[0].reason.as_deref(), Some("task_changed"));
    let original = store
        .write_automation_trigger(
            &project.id,
            &write_trigger(&task, AutomationEventSource::Manual),
        )
        .await
        .unwrap();
    assert_eq!(original.revision, 1);
}

#[tokio::test]
async fn concurrent_workers_claim_once_and_retirement_fences_original_dispatch() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, _, trigger) = fixture(&store, AutomationEventSource::Manual).await;
    store
        .submit_automation_event(
            &project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &event("pending"),
        )
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        store.claim_automation_event(),
        store.claim_automation_event()
    );
    let mut claims = vec![a.unwrap(), b.unwrap()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(claims.len(), 1);
    let claimed = claims.remove(0);
    store
        .reconcile_automation_event(&claimed.event.id, true)
        .await
        .unwrap();
    let current = store
        .automation_events(&project.id, &trigger.id, None, 50)
        .await
        .unwrap()
        .items
        .remove(0);
    let request = RetireAutomationEvent {
        idempotency_key: "retire".into(),
        expected_updated_at: current.updated_at,
    };
    let retired = store
        .retire_automation_event(&project.id, &current.id, &request)
        .await
        .unwrap();
    assert_eq!(retired.state, AutomationEventState::Discarded);
    assert_eq!(
        store
            .retire_automation_event(&project.id, &current.id, &request)
            .await
            .unwrap()
            .updated_at,
        retired.updated_at
    );
    // A forged or delayed admission cannot consume the preserved original claim.
    let cowork_workflow_contracts::RunRequest::Single(input) = claimed.request else {
        panic!()
    };
    let spec:RunSpec=serde_json::from_value(json!({"project_id":project.id,"order_id":input.order_id,"prompt":input.prompt,
        "provider":{"id":"fixture","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true},
        "access_mode":"read_only","executor":"local-v2","policy_revision":1,"allowed_tools":[],"budgets":input.budgets,"folders":[],"mcp_tools":[],"environment":null,"file_action":null,"workflow":null,"output":null,"model_binding":null,"agent_config":null})).unwrap();
    assert!(matches!(
        store.create_run(&input, spec).await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn webhook_credentials_are_scoped_revocable_and_never_returned() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, _, trigger) = fixture(&store, AutomationEventSource::Webhook).await;
    let token = "a7".repeat(32);
    let configure = ConfigureAutomationWebhook {
        expected_revision: 0,
        idempotency_key: "grant".into(),
        token: Some(token.clone()),
        enabled: true,
        expires_at: cowork_storage::now() + 60_000,
    };
    let grant = store
        .configure_automation_webhook(&project.id, &trigger.id, &configure)
        .await
        .unwrap();
    assert!(!serde_json::to_string(&grant).unwrap().contains(&token));
    assert!(matches!(
        store
            .submit_automation_webhook(&trigger.id, &"b8".repeat(32), &event("wrong"))
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .submit_automation_webhook("unknown", &token, &event("wrong"))
            .await,
        Err(Error::Forbidden)
    ));
    let accepted = store
        .submit_automation_webhook(&trigger.id, &token, &event("valid"))
        .await
        .unwrap();
    assert_eq!(accepted.source, AutomationEventSource::Webhook);
    let revoke = ConfigureAutomationWebhook {
        expected_revision: grant.revision,
        idempotency_key: "revoke".into(),
        token: None,
        enabled: false,
        expires_at: grant.expires_at,
    };
    store
        .configure_automation_webhook(&project.id, &trigger.id, &revoke)
        .await
        .unwrap();
    assert!(matches!(
        store
            .submit_automation_webhook(&trigger.id, &token, &event("revoked"))
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        store
            .configure_automation_webhook(&project.id, &trigger.id, &configure)
            .await
            .unwrap()
            .revision,
        1
    );
    assert!(
        !store
            .automation_webhook_grant(&project.id, &trigger.id)
            .await
            .unwrap()
            .unwrap()
            .enabled
    );
}

#[tokio::test]
async fn restart_preserves_pending_events_and_cross_project_access_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let (project, _, trigger) = fixture(&store, AutomationEventSource::Manual).await;
    store
        .submit_automation_event(
            &project.id,
            &trigger.id,
            AutomationEventSource::Manual,
            &event("restart"),
        )
        .await
        .unwrap();
    let reopened = Store::sqlite(&path).await.unwrap();
    assert!(reopened.claim_automation_event().await.unwrap().is_some());
    assert!(store.claim_automation_event().await.unwrap().is_none());
    let other = store
        .create_project(CreateProject {
            name: "Other".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        store
            .automation_events(&other.id, &trigger.id, None, 50)
            .await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store
            .submit_automation_event(
                &other.id,
                &trigger.id,
                AutomationEventSource::Manual,
                &event("cross")
            )
            .await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store
            .submit_automation_event(
                &project.id,
                &trigger.id,
                AutomationEventSource::Webhook,
                &event("source")
            )
            .await,
        Err(Error::Forbidden)
    ));
}
