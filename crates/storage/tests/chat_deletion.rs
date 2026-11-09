use cowork_contracts::*;
use cowork_domain::Error;
use cowork_storage::{Store, WriteSchedule};
use serde_json::json;
async fn fixture(store: &Store) -> (Project, Order) {
    let project = store
        .create_project(CreateProject {
            name: "Deletion proof".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Unique retirement title".into(),
        })
        .await
        .unwrap();
    (project, order)
}
fn deletion(order: &Order, key: &str) -> DeleteLibraryEntry {
    DeleteLibraryEntry {
        expected_revision: order.revision,
        idempotency_key: key.into(),
    }
}
fn task(order: &Order) -> WorkspaceTaskDefinition {
    serde_json::from_value(json!({"name":"Linked task","description":"","order_id":order.id,"prompt":"Inspect","expected_output":"","execution":{"kind":"model","provider_id":"fixture","provider_revision":1},"access_mode":"confirm_mutations","budgets":Budgets::default(),"folders":[],"native_tools":[],"mcp_tools":[]})).unwrap()
}
fn schedule(order: &Order) -> WriteSchedule {
    serde_json::from_value(json!({"id":null,"expected_revision":0,"name":"Linked schedule","timezone":"UTC","recurrence":{"kind":"daily","hour":9,"minute":0},"template":{"order_id":order.id,"prompt":"Inspect","provider_id":"fixture","provider_revision":1},"paused":false})).unwrap()
}
fn run(order: &Order) -> (CreateRun, RunSpec) {
    let input:CreateRun=serde_json::from_value(json!({"order_id":order.id,"base_revision":order.revision,"idempotency_key":"run-before-delete","prompt":"Inspect","provider_id":"fixture","provider_revision":1,"access_mode":"confirm_mutations"})).unwrap();
    let spec:RunSpec=serde_json::from_value(json!({"order_id":order.id,"project_id":order.project_id,"prompt":input.prompt,"provider":{"generation":{},"id":"fixture","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true},"access_mode":"confirm_mutations","executor":"local-v2","policy_revision":1,"allowed_tools":["create_artifact"],"budgets":Budgets::default()})).unwrap();
    (input, spec)
}
#[tokio::test]
async fn deletion_is_durable_distinct_from_archive_and_keeps_history_and_exact_ack() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retirement.sqlite");
    let store = Store::sqlite(&path).await.unwrap();
    let (project, order) = fixture(&store).await;
    let task = store
        .write_workspace_task(
            &project.id,
            None,
            &WriteWorkspaceTask {
                expected_revision: 0,
                idempotency_key: "task".into(),
                value: task(&order),
            },
        )
        .await
        .unwrap();
    let scheduled = store
        .productivity_write_schedule(&project.id, schedule(&order))
        .await
        .unwrap();
    let (request, spec) = run(&order);
    let run = store.create_run(&request, spec).await.unwrap();
    store.cancel(&run.id).await.unwrap();
    let archived = store
        .archive_chat(
            &order.id,
            &ArchiveChat {
                expected_revision: store.order(&order.id).await.unwrap().revision,
                idempotency_key: "archive".into(),
                archived: true,
            },
        )
        .await
        .unwrap();
    let events = serde_json::to_value(store.events(&run.id, 0).await.unwrap()).unwrap();
    let history = serde_json::to_value(store.messages(&order.id).await.unwrap()).unwrap();
    let input = deletion(&archived.order, "delete");
    let saved = store.delete_native_chat(&order.id, &input).await.unwrap();
    assert!(saved.deleted);
    assert_eq!(saved.revision, input.expected_revision + 1);
    assert!(store.orders(&project.id).await.unwrap().is_empty());
    assert!(store.archived_chats(&project.id).await.unwrap().is_empty());
    assert!(
        store
            .search("Unique retirement", None, 0, 20)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert!(store.workspace_tasks(&project.id).await.unwrap().is_empty());
    assert!(store.workspace_task(&task.id).await.unwrap().archived);
    let schedules = store.productivity_schedules(&project.id).await.unwrap();
    assert_eq!(schedules[0].id, scheduled.id);
    assert!(schedules[0].paused);
    assert_eq!(schedules[0].revision, scheduled.revision + 1);
    assert!(matches!(
        store.native_chat_available(&order.id).await,
        Err(Error::NotFound)
    ));
    assert!(store.export_order(&order.id).await.is_ok());
    assert!(store.history(&order.id).await.is_ok());
    assert_eq!(
        serde_json::to_value(store.events(&run.id, 0).await.unwrap()).unwrap(),
        events
    );
    assert_eq!(
        serde_json::to_value(store.messages(&order.id).await.unwrap()).unwrap(),
        history
    );
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        serde_json::to_value(store.delete_native_chat(&order.id, &input).await.unwrap()).unwrap(),
        serde_json::to_value(&saved).unwrap()
    );
    let mut changed = input.clone();
    changed.expected_revision += 1;
    assert!(matches!(
        store.delete_native_chat(&order.id, &changed).await,
        Err(Error::Conflict)
    ));
    changed.idempotency_key = "new-delete".into();
    assert!(matches!(
        store.delete_native_chat(&order.id, &changed).await,
        Err(Error::NotFound)
    ));
}
#[tokio::test]
async fn retired_chat_cannot_be_restored_moved_edited_or_receive_new_work() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, order) = fixture(&store).await;
    let saved = store
        .delete_native_chat(&order.id, &deletion(&order, "retire"))
        .await
        .unwrap();
    assert!(
        store
            .archive_chat(
                &order.id,
                &ArchiveChat {
                    expected_revision: saved.revision,
                    idempotency_key: "restore".into(),
                    archived: false
                }
            )
            .await
            .is_err()
    );
    assert!(
        store
            .manage_chat(
                &order.id,
                &ManageChat {
                    expected_revision: saved.revision,
                    idempotency_key: "move".into(),
                    title: "Resurrected".into(),
                    project_id: project.id.clone()
                }
            )
            .await
            .is_err()
    );
    assert!(
        store
            .save_draft(
                &order.id,
                SaveDraft {
                    base_revision: saved.revision,
                    draft: "No new draft".into()
                }
            )
            .await
            .is_err()
    );
    assert!(
        store
            .write_workspace_task(
                &project.id,
                None,
                &WriteWorkspaceTask {
                    expected_revision: 0,
                    idempotency_key: "new-task".into(),
                    value: task(&order)
                }
            )
            .await
            .is_err()
    );
    assert!(
        store
            .productivity_write_schedule(&project.id, schedule(&order))
            .await
            .is_err()
    );
    let mut live = order.clone();
    live.revision = saved.revision;
    let (request, spec) = run(&live);
    assert!(store.create_run(&request, spec).await.is_err());
    assert!(store.runs(&order.id).await.unwrap().is_empty());
    assert!(
        store
            .branch_order(
                &order.id,
                BranchOrder {
                    title: "Fork".into(),
                    through_message_id: "unneeded".into(),
                    idempotency_key: "fork".into()
                }
            )
            .await
            .is_err()
    );
    assert!(
        store
            .regenerate_order(
                &order.id,
                BranchOrder {
                    title: "Fork".into(),
                    through_message_id: "unneeded".into(),
                    idempotency_key: "regenerate".into()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        store
            .archive_retained_chats(1, 4_000_000_000_000)
            .await
            .unwrap(),
        0,
        "retention must not mutate a deleted chat"
    );
    assert_eq!(
        store.order(&order.id).await.unwrap().revision,
        saved.revision
    );
    let p = store.project_settings(&project.id).await.unwrap();
    store
        .delete_native_project(
            &project.id,
            &DeleteLibraryEntry {
                expected_revision: p.revision,
                idempotency_key: "project".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.order(&order.id).await.unwrap().project_id,
        project.id,
        "project retirement must not move a tombstoned chat"
    );
}
#[tokio::test]
async fn active_run_and_claimed_occurrence_block_deletion_without_partial_changes() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, order) = fixture(&store).await;
    let (request, spec) = run(&order);
    let active = store.create_run(&request, spec).await.unwrap();
    let current = store.order(&order.id).await.unwrap();
    assert!(matches!(
        store
            .delete_native_chat(&order.id, &deletion(&current, "blocked-run"))
            .await,
        Err(Error::WorkspaceBusy)
    ));
    store.native_chat_available(&order.id).await.unwrap();
    store.cancel(&active.id).await.unwrap();
    let scheduled = store
        .productivity_write_schedule(&project.id, schedule(&current))
        .await
        .unwrap();
    store
        .productivity_claim_schedule(
            Some(&project.id),
            Some(&scheduled.id),
            true,
            Some(scheduled.revision),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        store
            .delete_native_chat(&order.id, &deletion(&current, "blocked-claim"))
            .await,
        Err(Error::WorkspaceBusy)
    ));
    store.native_chat_available(&order.id).await.unwrap();
    assert_eq!(
        store.order(&order.id).await.unwrap().revision,
        current.revision
    );
}
#[tokio::test]
async fn stale_cas_cannot_remove_chat_and_parallel_new_work_cannot_survive_tombstone() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, order) = fixture(&store).await;
    let current = store
        .save_draft(
            &order.id,
            SaveDraft {
                base_revision: 1,
                draft: "Keep this draft".into(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .delete_native_chat(&order.id, &deletion(&order, "stale"))
            .await,
        Err(Error::Conflict)
    ));
    store.native_chat_available(&order.id).await.unwrap();
    let input = deletion(&current, "racing-delete");
    let task_input = WriteWorkspaceTask {
        expected_revision: 0,
        idempotency_key: "racing-task".into(),
        value: task(&current),
    };
    let (deleted, _new_task) = tokio::join!(
        store.delete_native_chat(&order.id, &input),
        store.write_workspace_task(&project.id, None, &task_input)
    );
    deleted.unwrap();
    assert!(store.workspace_tasks(&project.id).await.unwrap().is_empty());
    assert!(store.native_chat_available(&order.id).await.is_err());
}
