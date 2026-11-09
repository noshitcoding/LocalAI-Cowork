use cowork_contracts::*;
use cowork_domain::Error;
use cowork_storage::Store;

async fn project_order(store: &Store, name: &str) -> (Project, Order) {
    let p = store
        .create_project(CreateProject { name: name.into() })
        .await
        .unwrap();
    let o = store
        .create_order(CreateOrder {
            project_id: p.id.clone(),
            title: "Original chat".into(),
        })
        .await
        .unwrap();
    (p, o)
}
fn settings(project: &Project, key: &str) -> WriteProjectSettings {
    WriteProjectSettings {
        expected_revision: project.revision,
        idempotency_key: key.into(),
        name: "Renamed project".into(),
        instructions: "Ground all conclusions in evidence.".into(),
        sources: vec![ProjectSource {
            id: "source".into(),
            label: "Reference".into(),
            kind: "link".into(),
            reference: "https://example.com/reference".into(),
            enabled: true,
            access: "read_only".into(),
        }],
        working_folder: None,
    }
}
fn definition(order: &Order) -> WorkspaceTaskDefinition {
    WorkspaceTaskDefinition {
        name: "Research task".into(),
        description: "A reusable request".into(),
        order_id: order.id.clone(),
        prompt: "Summarize the architecture".into(),
        expected_output: "Cited findings".into(),
        execution: WorkspaceTaskExecution::Model {
            provider_id: "fixture".into(),
            provider_revision: 1,
        },
        access_mode: AccessMode::ConfirmMutations,
        budgets: Budgets::default(),
        folders: vec![],
        native_tools: vec![],
        mcp_tools: vec![],
        agent_config: None,
        environment: None,
    }
}

#[tokio::test]
async fn project_settings_and_name_are_atomic_cas_and_replay_preserves_receipt() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, _) = project_order(&store, "Initial").await;
    let initial = store.project_settings(&project.id).await.unwrap();
    assert_eq!(initial.revision, 1);
    assert!(initial.sources.is_empty());
    let input = settings(&project, "original");
    let saved = store
        .write_project_settings(&project.id, &input)
        .await
        .unwrap();
    assert_eq!(saved.revision, 2);
    assert_eq!(store.projects().await.unwrap()[0].name, "Renamed project");
    assert_eq!(
        store
            .write_project_settings(&project.id, &input)
            .await
            .unwrap()
            .revision,
        2
    );
    let mut changed = input.clone();
    changed.instructions = "Changed original".into();
    assert!(matches!(
        store.write_project_settings(&project.id, &changed).await,
        Err(Error::Conflict)
    ));
    changed.idempotency_key = "new".into();
    assert!(matches!(
        store.write_project_settings(&project.id, &changed).await,
        Err(Error::Conflict)
    ));
    changed.expected_revision = 2;
    changed.working_folder = Some("source".into());
    assert!(matches!(
        store.write_project_settings(&project.id, &changed).await,
        Err(Error::Validation(_))
    ));
    assert_eq!(
        store
            .project_settings(&project.id)
            .await
            .unwrap()
            .instructions,
        input.instructions
    );
}
#[tokio::test]
async fn chat_rename_move_archive_restore_keep_identity_and_history() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (first, order) = project_order(&store, "First").await;
    let (second, _) = project_order(&store, "Second").await;
    let input = ManageChat {
        expected_revision: order.revision,
        idempotency_key: "move".into(),
        title: "Moved chat".into(),
        project_id: second.id.clone(),
    };
    let moved = store.manage_chat(&order.id, &input).await.unwrap();
    assert_eq!(moved.order.id, order.id);
    assert_eq!(moved.order.project_id, second.id);
    assert_eq!(moved.order.revision, 2);
    assert_eq!(
        store
            .manage_chat(&order.id, &input)
            .await
            .unwrap()
            .order
            .revision,
        2
    );
    assert!(store.orders(&first.id).await.unwrap().is_empty());
    let archived = store
        .archive_chat(
            &order.id,
            &ArchiveChat {
                expected_revision: 2,
                idempotency_key: "archive".into(),
                archived: true,
            },
        )
        .await
        .unwrap();
    assert!(archived.archived);
    assert!(
        !store
            .orders(&second.id)
            .await
            .unwrap()
            .iter()
            .any(|item| item.id == order.id)
    );
    assert_eq!(store.order(&order.id).await.unwrap().title, "Moved chat");
    assert_eq!(
        store.archived_chats(&second.id).await.unwrap()[0].order.id,
        order.id
    );
    assert!(store.archived_chats(&first.id).await.unwrap().is_empty());
    assert!(store.history(&order.id).await.is_ok());
    let restored = store
        .archive_chat(
            &order.id,
            &ArchiveChat {
                expected_revision: 3,
                idempotency_key: "restore".into(),
                archived: false,
            },
        )
        .await
        .unwrap();
    assert!(store.archived_chats(&second.id).await.unwrap().is_empty());
    assert!(!restored.archived);
    assert!(
        store
            .orders(&second.id)
            .await
            .unwrap()
            .iter()
            .any(|item| item.id == order.id)
    );
}
#[tokio::test]
async fn active_run_blocks_move_and_archive_but_allows_rename() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, order) = project_order(&store, "First").await;
    let (target, _) = project_order(&store, "Second").await;
    let provider = ProviderProfile {
        generation: Default::default(),
        id: "fixture".into(),
        name: "Fixture".into(),
        kind: ProviderKind::Test,
        endpoint: String::new(),
        model: "test".into(),
        revision: 1,
        context_tokens: 32768,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    let input:CreateRun=serde_json::from_value(serde_json::json!({"order_id":order.id,"base_revision":order.revision,"idempotency_key":"run-original","prompt":"Inspect","provider_id":"fixture","provider_revision":1,"access_mode":"confirm_mutations"})).unwrap();
    let spec = RunSpec {
        input_workspace: None,
        agent_config: None,
        model_binding: None,
        output: None,
        workflow: None,
        mcp_tools: vec![],
        environment: None,
        folders: vec![],
        order_id: order.id.clone(),
        project_id: project.id.clone(),
        prompt: input.prompt.clone(),
        provider: Some(provider),
        file_action: None,
        access_mode: AccessMode::ConfirmMutations,
        executor: "local-v2".into(),
        policy_revision: 1,
        allowed_tools: vec!["create_artifact".into()],
        budgets: Budgets::default(),
    };
    store.create_run(&input, spec).await.unwrap();
    let current = store.order(&order.id).await.unwrap();
    assert!(matches!(
        store
            .manage_chat(
                &order.id,
                &ManageChat {
                    expected_revision: current.revision,
                    idempotency_key: "move-active".into(),
                    title: "Moved".into(),
                    project_id: target.id
                }
            )
            .await,
        Err(Error::WorkspaceBusy)
    ));
    assert!(matches!(
        store
            .archive_chat(
                &order.id,
                &ArchiveChat {
                    expected_revision: current.revision,
                    idempotency_key: "archive-active".into(),
                    archived: true
                }
            )
            .await,
        Err(Error::WorkspaceBusy)
    ));
    assert_eq!(
        store
            .manage_chat(
                &order.id,
                &ManageChat {
                    expected_revision: current.revision,
                    idempotency_key: "rename".into(),
                    title: "Renamed active chat".into(),
                    project_id: project.id
                }
            )
            .await
            .unwrap()
            .order
            .title,
        "Renamed active chat"
    );
}
#[tokio::test]
async fn task_start_freezes_exact_version_across_retries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite");
    let store = Store::sqlite(&path).await.unwrap();
    let (project, order) = project_order(&store, "Tasks").await;
    let project_input = settings(&project, "project-settings");
    store
        .write_project_settings(&project.id, &project_input)
        .await
        .unwrap();
    let input = WriteWorkspaceTask {
        expected_revision: 0,
        idempotency_key: "create-task".into(),
        value: definition(&order),
    };
    let task = store
        .write_workspace_task(&project.id, None, &input)
        .await
        .unwrap();
    assert_eq!(
        store
            .write_workspace_task(&project.id, None, &input)
            .await
            .unwrap()
            .id,
        task.id
    );
    let start = StartWorkspaceTask {
        expected_revision: task.revision,
        order_revision: order.revision,
        idempotency_key: "start-original".into(),
    };
    let snapshot = store
        .begin_workspace_task_run(&task.id, &start)
        .await
        .unwrap();
    assert_eq!(snapshot.value.prompt, input.value.prompt);
    let mut update = input.clone();
    update.expected_revision = 1;
    update.idempotency_key = "edit-task".into();
    update.value.prompt = "A different future task".into();
    store
        .write_workspace_task(&project.id, Some(&task.id), &update)
        .await
        .unwrap();
    let repeated = store
        .begin_workspace_task_run(&task.id, &start)
        .await
        .unwrap();
    assert_eq!(repeated.value.prompt, snapshot.value.prompt);
    assert_eq!(repeated.revision, 1);
    let mut stale = start.clone();
    stale.idempotency_key = "new-run".into();
    assert!(matches!(
        store.begin_workspace_task_run(&task.id, &stale).await,
        Err(Error::Conflict)
    ));
    let mut changed = start.clone();
    changed.order_revision += 1;
    assert!(matches!(
        store.begin_workspace_task_run(&task.id, &changed).await,
        Err(Error::Conflict)
    ));
    drop(store);
    let reopened = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        reopened.workspace_tasks(&project.id).await.unwrap()[0]
            .value
            .prompt,
        "A different future task"
    );
    let archived = reopened
        .archive_workspace_task(
            &task.id,
            &ArchiveWorkspaceTask {
                expected_revision: 2,
                idempotency_key: "archive-task".into(),
            },
        )
        .await
        .unwrap();
    assert!(archived.archived);
    assert!(
        reopened
            .workspace_tasks(&project.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        reopened.begin_workspace_task_run(&task.id, &start).await,
        Err(Error::Conflict)
    ));
}

#[tokio::test]
async fn a_prepared_task_cannot_start_after_its_chat_is_archived() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, order) = project_order(&store, "Archived task chat").await;
    let task = store
        .write_workspace_task(
            &project.id,
            None,
            &WriteWorkspaceTask {
                expected_revision: 0,
                idempotency_key: "create".into(),
                value: definition(&order),
            },
        )
        .await
        .unwrap();
    let request = StartWorkspaceTask {
        expected_revision: task.revision,
        order_revision: order.revision,
        idempotency_key: "prepared-original".into(),
    };
    store
        .begin_workspace_task_run(&task.id, &request)
        .await
        .unwrap();
    store
        .archive_chat(
            &order.id,
            &ArchiveChat {
                expected_revision: order.revision,
                idempotency_key: "archive-chat".into(),
                archived: true,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        store.begin_workspace_task_run(&task.id, &request).await,
        Err(Error::Forbidden)
    ));
    assert!(
        store
            .workspace_task(&task.id)
            .await
            .unwrap()
            .last_run_id
            .is_none()
    );
}

#[tokio::test]
async fn project_deletion_detaches_chats_disables_tasks_and_has_a_durable_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("delete.sqlite");
    let store = Store::sqlite(&path).await.unwrap();
    let (project, order) = project_order(&store, "Remove me").await;
    let task = store
        .write_workspace_task(
            &project.id,
            None,
            &WriteWorkspaceTask {
                expected_revision: 0,
                idempotency_key: "task".into(),
                value: definition(&order),
            },
        )
        .await
        .unwrap();
    let current = store.project_settings(&project.id).await.unwrap();
    let input = DeleteLibraryEntry {
        expected_revision: current.revision,
        idempotency_key: "delete".into(),
    };
    let deleted = store
        .delete_native_project(&project.id, &input)
        .await
        .unwrap();
    assert!(deleted.deleted);
    assert!(store.projects().await.unwrap().is_empty());
    assert!(store.workspace_task(&task.id).await.unwrap().archived);
    let detached = store.order(&order.id).await.unwrap();
    assert_eq!(detached.project_id, "__cowork_independent_chats__");
    assert_eq!(store.orders(&detached.project_id).await.unwrap().len(), 1);
    assert!(
        store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Forbidden".into()
            })
            .await
            .is_err()
    );
    store.close().await;
    let reopened = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        reopened
            .delete_native_project(&project.id, &input)
            .await
            .unwrap()
            .revision,
        deleted.revision
    );
    assert!(reopened.export_order(&order.id).await.is_ok());
}
