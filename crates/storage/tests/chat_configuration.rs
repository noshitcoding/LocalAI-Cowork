use cowork_contracts::*;
use cowork_domain::Error;
use cowork_storage::Store;
use serde_json::json;

fn value(project: &str) -> ChatRunConfiguration {
    serde_json::from_value(json!({"agent_config":{"instructions":"COPIED_LIBRARY_TEXT","generation":{"temperature":0.15},"agent_source":{"id":"agent-source","revision":3},"memory_ids":["memory-source"]},"provider_id":"saved-profile","provider_revision":7,"access_mode":"confirm_all","budgets":Budgets::default(),"native_tools":["browser"],"mcp_tools":[],"folders":[{"id":"folder","project_id":project,"name":"Original folder","writable":false,"revision":"exact-folder-revision","resource_id":"resource"}],"environment":null,"model_binding":null,"output":null,"workflow":null})).unwrap()
}

#[tokio::test]
async fn copied_chat_configuration_survives_reopen_and_has_independent_cas_and_original_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Original".into(),
        })
        .await
        .unwrap();
    let next_project = store
        .create_project(CreateProject {
            name: "Moved".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Copied chat".into(),
        })
        .await
        .unwrap();
    let empty = store.chat_configuration(&order.id).await.unwrap();
    assert_eq!(empty.revision, 0);
    assert!(empty.value.is_none());
    let original = WriteChatConfiguration {
        expected_revision: 0,
        idempotency_key: "original-save".into(),
        value: value(&project.id),
    };
    let first = store
        .write_chat_configuration(&order.id, &original)
        .await
        .unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(
        store.order(&order.id).await.unwrap().revision,
        order.revision
    );
    let mut update = original.clone();
    update.expected_revision = 1;
    update.idempotency_key = "edit-save".into();
    update.value.agent_config.instructions = "LOCAL_COPIED_EDIT".into();
    let second = store
        .write_chat_configuration(&order.id, &update)
        .await
        .unwrap();
    let replay = store
        .write_chat_configuration(&order.id, &original)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&replay).unwrap(),
        serde_json::to_value(&first).unwrap()
    );
    assert_eq!(
        store.chat_configuration(&order.id).await.unwrap().revision,
        2
    );
    let mut wrong = original.clone();
    wrong.value.agent_config.instructions = "different same key".into();
    assert!(matches!(
        store.write_chat_configuration(&order.id, &wrong).await,
        Err(Error::Conflict)
    ));
    wrong.idempotency_key = "stale-cas".into();
    assert!(matches!(
        store.write_chat_configuration(&order.id, &wrong).await,
        Err(Error::Conflict)
    ));
    let moved = store
        .manage_chat(
            &order.id,
            &ManageChat {
                expected_revision: order.revision,
                idempotency_key: "move".into(),
                title: "Renamed copy".into(),
                project_id: next_project.id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(moved.order.revision, order.revision + 1);
    let after_move = store.chat_configuration(&order.id).await.unwrap();
    assert_eq!(after_move.revision, second.revision);
    assert_eq!(
        after_move.value.as_ref().unwrap().folders[0].project_id,
        project.id
    );
    store
        .archive_chat(
            &order.id,
            &ArchiveChat {
                expected_revision: moved.order.revision,
                idempotency_key: "archive".into(),
                archived: true,
            },
        )
        .await
        .unwrap();
    let mut archived_write = update.clone();
    archived_write.expected_revision = 2;
    archived_write.idempotency_key = "after-archive".into();
    assert!(matches!(
        store
            .write_chat_configuration(&order.id, &archived_write)
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        store
            .write_chat_configuration(&order.id, &original)
            .await
            .unwrap()
            .revision,
        1
    );
    store.close().await;
    let reopened = Store::sqlite(&path).await.unwrap();
    let saved = reopened.chat_configuration(&order.id).await.unwrap();
    let saved = saved.value.unwrap();
    assert_eq!(saved.agent_config.instructions, "LOCAL_COPIED_EDIT");
    assert_eq!(saved.agent_config.agent_source.unwrap().revision, 3);
    assert_eq!(saved.provider_revision, Some(7));
    assert_eq!(saved.provider_id.as_deref(), Some("saved-profile"));
    assert_eq!(saved.folders[0].project_id, project.id);
    assert_eq!(saved.native_tools, vec!["browser"]);
    assert!(reopened.chat_management(&order.id).await.unwrap().archived);
    reopened.close().await;
}

#[tokio::test]
async fn configuration_rejects_invalid_selection_and_does_not_create_a_run() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Bounds".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "No execution".into(),
        })
        .await
        .unwrap();
    let mut input = WriteChatConfiguration {
        expected_revision: 0,
        idempotency_key: "invalid".into(),
        value: value(&project.id),
    };
    input.value.provider_revision = None;
    assert!(matches!(
        store.write_chat_configuration(&order.id, &input).await,
        Err(Error::Validation(_))
    ));
    input.value.provider_revision = Some(7);
    input.value.native_tools.push("invented_tool".into());
    assert!(matches!(
        store.write_chat_configuration(&order.id, &input).await,
        Err(Error::Validation(_))
    ));
    input.value.native_tools = vec!["browser".into()];
    store
        .write_chat_configuration(&order.id, &input)
        .await
        .unwrap();
    assert!(store.runs(&order.id).await.unwrap().is_empty());
}
