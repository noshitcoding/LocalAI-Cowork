use cowork_contracts::*;
use cowork_domain::Error;
use cowork_runtime::Runtime;
use cowork_storage::Store;
use std::collections::BTreeMap;
#[tokio::test]
async fn native_chat_deletion_holds_workspace_custody_and_replays_original_without_reopening_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("chat.sqlite"))
        .await
        .unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Native custody".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "Chat".into(),
        })
        .await
        .unwrap();
    let toolbox = cowork_toolbox::Toolbox::open(cowork_toolbox::Config {
        root: dir.path().join("toolbox"),
        programs: BTreeMap::new(),
        environment: BTreeMap::new(),
        github_token: None,
    })
    .unwrap();
    let runtime = Runtime::new(store, vec![], true)
        .unwrap()
        .with_toolbox(toolbox.clone());
    let input = DeleteLibraryEntry {
        expected_revision: order.revision,
        idempotency_key: "delete-exact".into(),
    };
    let active = toolbox.workspace_lease(&order.id).unwrap();
    assert!(matches!(
        runtime.delete_native_chat(&order.id, &input).await,
        Err(Error::WorkspaceBusy)
    ));
    runtime
        .store
        .native_chat_available(&order.id)
        .await
        .unwrap();
    drop(active);
    let saved = runtime.delete_native_chat(&order.id, &input).await.unwrap();
    assert!(saved.deleted);
    let _unrelated_after_retirement = toolbox.workspace_lease(&order.id).unwrap();
    let replay = runtime.delete_native_chat(&order.id, &input).await.unwrap();
    assert_eq!(replay.revision, saved.revision);
    assert!(
        runtime
            .store
            .native_chat_available(&order.id)
            .await
            .is_err()
    );
}
