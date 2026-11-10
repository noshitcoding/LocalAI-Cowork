use cowork_broker::{FileBroker, NativeFolderSelection};
use cowork_contracts::*;
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use serde_json::json;
use std::{path::Path, time::Duration};

async fn due(path: &Path) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(path))
        .await
        .unwrap();
    sqlx::query("UPDATE automation_file_watches SET next_scan_at=0")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
async fn file_event_binds_observed_path_and_hash_to_an_accepted_typed_workflow() {
    let (temp, runtime, project, mut write, original, mut profile) = fixture(false, &[]).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    profile.id = "typed-file-fixture-profile".into();
    profile.kind = ProviderKind::OpenaiCompatible;
    profile.endpoint = format!("http://{}", listener.local_addr().unwrap());
    profile.model = "typed-file-fixture".into();
    let app = axum::Router::new().route(
        "/models",
        axum::routing::get(|| async { axum::Json(json!({"data":[{"id":"typed-file-fixture"}]})) }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    runtime.store.insert_provider(&profile).await.unwrap();
    runtime.providers.write().await.push(ConfiguredProvider {
        profile: profile.clone(),
        credential_env: None,
    });
    let mut definition: cowork_domain::workflows::WorkflowDefinition = serde_json::from_str(
        include_str!("../../../tests/fixtures/workflow-dataflow.json"),
    )
    .unwrap();
    definition.roles[0].provider_id = profile.id;
    definition.roles[0].provider_revision = profile.revision;
    let library = runtime
        .store
        .create_workflow(&project.id, "typed-file", &definition)
        .await
        .unwrap();
    let old = runtime.store.workspace_task(&write.task_id).await.unwrap();
    let mut value = old.value;
    value.execution=serde_json::from_value(json!({"kind":"workflow","workflow_id":library.id,"version_id":library.versions[0].id,"inputs":{"path":"","sha256":"","currency":"EUR","private_note":"fixed"}})).unwrap();
    value.budgets = definition.budgets;
    let task=runtime.store.write_workspace_task(&project.id,Some(&old.id),&serde_json::from_value(json!({"expected_revision":old.revision,"idempotency_key":"typed-task","value":value})).unwrap()).await.unwrap();
    write.id = Some(original.id.clone());
    write.expected_revision = original.revision;
    write.idempotency_key = "typed-mapping".into();
    write.task_revision = task.revision;
    write.workflow_inputs = Some(Box::new(
        serde_json::from_value(
            json!({"format_version":1,"bindings":{"/path":"/path","/sha256":"/sha256"}}),
        )
        .unwrap(),
    ));
    let trigger = runtime
        .write_automation_trigger(&project.id, &write)
        .await
        .unwrap();
    let database = temp.path().join("data.sqlite3");
    std::fs::write(
        temp.path().join("inbox/new.pdf"),
        b"observed metadata fixture",
    )
    .unwrap();
    due(&database).await;
    assert_eq!(runtime.poll_automation_file_watches().await.unwrap(), 0);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    due(&database).await;
    assert_eq!(runtime.poll_automation_file_watches().await.unwrap(), 1);
    let events = runtime
        .store
        .automation_events(&project.id, &trigger.id, None, 50)
        .await
        .unwrap();
    let input = events.items[0].workflow_input.as_ref().unwrap();
    assert_eq!(input.values["path"], "new.pdf");
    assert_eq!(
        input.values["sha256"],
        cowork_domain::hash(b"observed metadata fixture")
    );
    let dispatch = runtime
        .store
        .claim_automation_event()
        .await
        .unwrap()
        .unwrap();
    let result = runtime.create_run_request(&dispatch.request).await;
    assert!(result.is_ok(), "file workflow admission: {result:?}");
    runtime
        .store
        .reconcile_automation_event(&dispatch.event.id, false)
        .await
        .unwrap();
    let events = runtime
        .store
        .automation_events(&project.id, &trigger.id, None, 50)
        .await
        .unwrap();
    let run = runtime
        .store
        .stored_run(events.items[0].run_id.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        run.record.spec.workflow.as_ref().unwrap().inputs.as_deref(),
        Some(input.values.as_ref())
    );
    assert!(!run.record.spec.prompt.contains("previous_sha256"));
    runtime.store.close().await;
    server.abort();
}
async fn fixture(
    include_existing: bool,
    initial_files: &[(&str, &str)],
) -> (
    tempfile::TempDir,
    Runtime,
    Project,
    WriteAutomationTrigger,
    AutomationTrigger,
    ProviderProfile,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("inbox");
    std::fs::create_dir(&root).unwrap();
    for (path, content) in initial_files {
        std::fs::write(root.join(path), content).unwrap();
    }
    let store = Store::sqlite(&temp.path().join("data.sqlite3"))
        .await
        .unwrap();
    let project = store
        .create_project(CreateProject {
            name: "File events".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "File processing".into(),
        })
        .await
        .unwrap();
    let broker = FileBroker::open(&temp.path().join("broker"), vec![]).unwrap();
    let grant = broker
        .register_native_folder(&NativeFolderSelection {
            idempotency_key: "select-folder".into(),
            project_id: project.id.clone(),
            name: "Inbox".into(),
            path: root.clone(),
            expected_resource_id: cowork_broker::inspect_native_folder(&root).unwrap().1,
            writable: false,
        })
        .unwrap()
        .grant;
    let profile:ProviderProfile=serde_json::from_value(json!({"id":"fixture","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true})).unwrap();
    store.insert_provider(&profile).await.unwrap();
    let task=store.write_workspace_task(&project.id,None,&serde_json::from_value(json!({"expected_revision":0,"idempotency_key":"task","value":{"name":"Read invoices","description":"","order_id":order.id,"prompt":"Check the exact observed file hash before processing.","expected_output":"A checked result","execution":{"kind":"model","provider_id":"fixture","provider_revision":1},"access_mode":"read_only","folders":[grant],"budgets":Budgets::default()}})).unwrap()).await.unwrap();
    let config = AutomationFileWatch {
        folder: grant,
        path: ".".into(),
        recursive: true,
        changes: vec![
            FileWatchChange::Created,
            FileWatchChange::Modified,
            FileWatchChange::Removed,
        ],
        extensions: vec![".pdf".into()],
        exclude_paths: vec!["processed".into()],
        include_existing,
        debounce_ms: 1000,
        poll_interval_ms: 5000,
        max_files: 100,
        max_file_bytes: 1024,
        max_scan_bytes: 4096,
    };
    let write = WriteAutomationTrigger {
        workflow_inputs: None,
        id: None,
        expected_revision: 0,
        idempotency_key: "watch".into(),
        name: "Invoices".into(),
        task_id: task.id,
        task_revision: task.revision,
        source: AutomationEventSource::File,
        file_watch: Some(config),
        predicates: vec![],
        enabled: true,
    };
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile: profile.clone(),
            credential_env: None,
        }],
        true,
    )
    .unwrap()
    .with_file_broker(broker);
    let trigger = runtime
        .write_automation_trigger(&project.id, &write)
        .await
        .unwrap();
    (temp, runtime, project, write, trigger, profile)
}

#[tokio::test]
async fn actual_files_debounce_across_restart_and_admit_one_normal_run() {
    let (temp, runtime, project, _, trigger, profile) =
        fixture(false, &[("existing.pdf", "old invoice")]).await;
    let database = temp.path().join("data.sqlite3");
    assert_eq!(runtime.poll_automation_file_watches().await.unwrap(), 0);
    std::fs::write(temp.path().join("inbox/new.pdf"), "partial").unwrap();
    due(&database).await;
    assert_eq!(runtime.poll_automation_file_watches().await.unwrap(), 0);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    std::fs::write(temp.path().join("inbox/new.pdf"), "complete invoice").unwrap();
    due(&database).await;
    assert_eq!(runtime.poll_automation_file_watches().await.unwrap(), 0);
    assert_eq!(
        runtime
            .store
            .automation_file_watch_status(&project.id, &trigger.id)
            .await
            .unwrap()
            .pending_changes,
        1
    );
    drop(runtime);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let reopened = Runtime::new(
        Store::sqlite(&database).await.unwrap(),
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        true,
    )
    .unwrap()
    .with_file_broker(FileBroker::open(&temp.path().join("broker"), vec![]).unwrap());
    due(&database).await;
    assert_eq!(reopened.poll_automation_file_watches().await.unwrap(), 1);
    due(&database).await;
    assert_eq!(reopened.poll_automation_file_watches().await.unwrap(), 0);
    let page = reopened
        .store
        .automation_events(&project.id, &trigger.id, None, 50)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].source, AutomationEventSource::File);
    assert_eq!(page.items[0].data["path"], "new.pdf");
    assert_eq!(
        page.items[0].data["sha256"],
        cowork_domain::hash(b"complete invoice")
    );
    assert_eq!(reopened.automation_tick().await.unwrap(), 1);
    let page = reopened
        .store
        .automation_events(&project.id, &trigger.id, None, 50)
        .await
        .unwrap();
    let run = reopened
        .store
        .stored_run(page.items[0].run_id.as_ref().unwrap())
        .await
        .unwrap();
    assert!(matches!(run.record.spec.access_mode, AccessMode::ReadOnly));
    assert!(run.record.spec.prompt.contains("new.pdf"));
    assert!(
        run.record
            .spec
            .prompt
            .contains(&cowork_domain::hash(b"complete invoice"))
    );
}

#[tokio::test]
async fn failed_partial_scan_preserves_baseline_and_never_infers_a_removal() {
    let (temp, runtime, project, _, trigger, _) = fixture(false, &[("a.pdf", "original")]).await;
    let database = temp.path().join("data.sqlite3");
    runtime.poll_automation_file_watches().await.unwrap();
    let baseline = runtime
        .store
        .automation_file_watch_status(&project.id, &trigger.id)
        .await
        .unwrap();
    std::fs::remove_file(temp.path().join("inbox/a.pdf")).unwrap();
    std::fs::write(temp.path().join("inbox/too-large.pdf"), vec![0; 2048]).unwrap();
    due(&database).await;
    assert_eq!(runtime.poll_automation_file_watches().await.unwrap(), 0);
    let failed = runtime
        .store
        .automation_file_watch_status(&project.id, &trigger.id)
        .await
        .unwrap();
    assert_eq!(failed.last_scan_at, baseline.last_scan_at);
    assert_eq!(failed.tracked_files, 1);
    assert_eq!(
        failed.reason.as_deref(),
        Some("automation_watch_file_too_large")
    );
    assert!(
        runtime
            .store
            .automation_events(&project.id, &trigger.id, None, 50)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    std::fs::remove_file(temp.path().join("inbox/too-large.pdf")).unwrap();
    due(&database).await;
    runtime.poll_automation_file_watches().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    due(&database).await;
    assert_eq!(runtime.poll_automation_file_watches().await.unwrap(), 1);
    let page = runtime
        .store
        .automation_events(&project.id, &trigger.id, None, 50)
        .await
        .unwrap();
    assert_eq!(page.items[0].data["change"], "removed");
    assert!(page.items[0].data["sha256"].is_null());
    assert_eq!(
        page.items[0].data["previous_sha256"],
        cowork_domain::hash(b"original")
    );
}

#[tokio::test]
async fn revoked_folder_stops_observation_and_original_configuration_receipt_remains_readable() {
    let (temp, runtime, project, write, trigger, _) = fixture(true, &[]).await;
    let watch = write.file_watch.as_ref().unwrap();
    std::fs::write(temp.path().join("inbox/a.pdf"), "invoice").unwrap();
    runtime
        .files
        .as_ref()
        .unwrap()
        .revoke_native_folder(&project.id, &watch.folder.id, &watch.folder.revision)
        .unwrap();
    assert_eq!(runtime.poll_automation_file_watches().await.unwrap(), 0);
    let status = runtime
        .store
        .automation_file_watch_status(&project.id, &trigger.id)
        .await
        .unwrap();
    assert_eq!(
        status.reason.as_deref(),
        Some("automation_watch_folder_unavailable")
    );
    assert!(!status.initialized);
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
        runtime
            .write_automation_trigger(&project.id, &write)
            .await
            .unwrap()
            .id,
        trigger.id
    );
}

#[tokio::test]
async fn concurrent_and_expired_observers_cannot_commit_an_old_scan() {
    let (temp, runtime, project, _, trigger, _) = fixture(false, &[]).await;
    let (a, b) = tokio::join!(
        runtime.store.claim_automation_file_watch(),
        runtime.store.claim_automation_file_watch()
    );
    let claims = vec![a.unwrap(), b.unwrap()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(claims.len(), 1);
    let old = &claims[0];
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(temp.path().join("data.sqlite3")),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE automation_file_watches SET lease_until=0,next_scan_at=0")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let fresh = runtime
        .store
        .claim_automation_file_watch()
        .await
        .unwrap()
        .unwrap();
    assert!(
        runtime
            .store
            .complete_automation_file_watch(old, &[], 0, &[])
            .await
            .is_err()
    );
    runtime
        .store
        .complete_automation_file_watch(&fresh, &[], 0, &[])
        .await
        .unwrap();
    assert!(
        runtime
            .store
            .complete_automation_file_watch(&fresh, &[], 0, &[])
            .await
            .is_err()
    );
    assert!(
        runtime
            .store
            .automation_file_watch_status(&project.id, &trigger.id)
            .await
            .unwrap()
            .initialized
    );
}

#[tokio::test]
async fn changing_the_trigger_fences_a_scanned_but_uncommitted_result() {
    let (temp, runtime, project, mut write, trigger, _) = fixture(true, &[]).await;
    std::fs::write(temp.path().join("inbox/a.pdf"), "invoice").unwrap();
    let claim = runtime
        .store
        .claim_automation_file_watch()
        .await
        .unwrap()
        .unwrap();
    let prepared = runtime
        .files
        .as_ref()
        .unwrap()
        .prepare_file_watch_scan(&project.id, write.file_watch.as_ref().unwrap())
        .unwrap();
    write.id = Some(trigger.id.clone());
    write.expected_revision = trigger.revision;
    write.idempotency_key = "pause-watch".into();
    write.enabled = false;
    runtime
        .write_automation_trigger(&project.id, &write)
        .await
        .unwrap();
    assert!(
        runtime
            .store
            .complete_automation_file_watch(
                &claim,
                &prepared.files,
                prepared.ignored_entries,
                &prepared.unavailable_paths
            )
            .await
            .is_err()
    );
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
        runtime
            .store
            .automation_file_watch_status(&project.id, &trigger.id)
            .await
            .unwrap()
            .reason
            .as_deref(),
        Some("trigger_paused")
    );
}

#[tokio::test]
async fn temporarily_unobservable_hardlink_is_never_mistaken_for_a_removal() {
    let (temp, runtime, project, _, trigger, _) = fixture(false, &[("a.pdf", "original")]).await;
    let database = temp.path().join("data.sqlite3");
    let observed = temp.path().join("inbox/a.pdf");
    let extra_link = temp.path().join("extra-link.pdf");
    std::fs::hard_link(&observed, &extra_link).unwrap();
    for _ in 0..2 {
        due(&database).await;
        assert_eq!(runtime.poll_automation_file_watches().await.unwrap(), 0);
        tokio::time::sleep(Duration::from_millis(1100)).await;
    }
    let status = runtime
        .store
        .automation_file_watch_status(&project.id, &trigger.id)
        .await
        .unwrap();
    assert_eq!(status.tracked_files, 1);
    assert_eq!(status.pending_changes, 0);
    assert_eq!(
        status.reason.as_deref(),
        Some("automation_watch_entries_unavailable")
    );
    std::fs::remove_file(extra_link).unwrap();
    due(&database).await;
    assert_eq!(runtime.poll_automation_file_watches().await.unwrap(), 0);
    assert!(
        runtime
            .store
            .automation_file_watch_status(&project.id, &trigger.id)
            .await
            .unwrap()
            .reason
            .is_none()
    );
    std::fs::remove_file(observed).unwrap();
    due(&database).await;
    assert_eq!(runtime.poll_automation_file_watches().await.unwrap(), 0);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    due(&database).await;
    assert_eq!(runtime.poll_automation_file_watches().await.unwrap(), 1);
    let page = runtime
        .store
        .automation_events(&project.id, &trigger.id, None, 50)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].data["change"], "removed");
    assert_eq!(
        page.items[0].data["previous_sha256"],
        cowork_domain::hash(b"original")
    );
}
