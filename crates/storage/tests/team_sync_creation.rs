//! One real SQLite probe for offline creation, original receipt replay and scope collisions.
use cowork_contracts::CreateTeamProject;
use cowork_domain::Error;
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{
    Store,
    team_service::{LiteLlmExecutor, TeamService, TeamSyncMutation},
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;
use zeroize::Zeroizing;

struct Keys;
impl KeyStore for Keys {
    fn load(&self, _: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        Ok(Zeroizing::new([41; 32]))
    }
}

#[tokio::test]
async fn offline_order_creation_keeps_original_receipts_and_scope() {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&temporary.path().join("replica.sqlite"))
        .await
        .unwrap();
    let identity = store.identity();
    let bootstrap = "b".repeat(64);
    identity
        .initialize_credentials(&bootstrap, &"s".repeat(64))
        .await
        .unwrap();
    let (_, session) = identity
        .bootstrap(
            &bootstrap,
            "replica-owner",
            "Replica owner",
            "SyntheticPassword-2026!",
            "Replica team",
        )
        .await
        .unwrap();
    let actor = identity.authenticate_session(&session.token).await.unwrap();
    let service = TeamService::new(
        store,
        Arc::new(Keys),
        KeyId {
            instance: Uuid::new_v4(),
            key: Uuid::new_v4(),
        },
        Arc::new(LiteLlmExecutor::new(None).unwrap()),
    );
    let team = service.teams(&actor).await.unwrap()[0].id;
    let project = service
        .create_project(
            &actor,
            team,
            CreateTeamProject {
                name: "Original project".into(),
                idempotency_key: "original-project".into(),
            },
        )
        .await
        .unwrap()
        .id;
    let other = service
        .create_project(
            &actor,
            team,
            CreateTeamProject {
                name: "Other project".into(),
                idempotency_key: "other-project".into(),
            },
        )
        .await
        .unwrap()
        .id;
    let id = Uuid::new_v4();
    let original = TeamSyncMutation {
        idempotency_key: Uuid::new_v4(),
        kind: "order".into(),
        entity_id: id,
        expected_revision: 0,
        value: json!({"title":" Offline order ","prompt":"Original prompt","expected_revision":0,"archived":false}),
    };
    let receipt = service
        .apply_sync(
            &actor,
            team,
            project,
            serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(receipt["value"]["id"], id.to_string());
    assert_eq!(receipt["value"]["revision"], 1);
    assert_eq!(receipt["value"]["title"], "Offline order");
    let page = service
        .sync_project(&actor, team, project, 0, 100)
        .await
        .unwrap();
    assert_eq!(
        page.changes
            .iter()
            .filter(|change| change.kind == "order" && change.entity_id == id)
            .count(),
        1
    );
    let update = TeamSyncMutation {
        idempotency_key: Uuid::new_v4(),
        kind: "order".into(),
        entity_id: id,
        expected_revision: 1,
        value: json!({"title":"Changed order","prompt":"Changed prompt","expected_revision":1,"archived":false}),
    };
    service
        .apply_sync(&actor, team, project, update)
        .await
        .unwrap();
    let replay = service
        .apply_sync(
            &actor,
            team,
            project,
            serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay, receipt);
    let orders = service.orders(&actor, team, project).await.unwrap();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].revision, 2);
    assert_eq!(orders[0].title, "Changed order");
    let mut changed_original: TeamSyncMutation =
        serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap();
    changed_original.value["prompt"] = json!("Replacement request with the original key");
    assert!(matches!(
        service
            .apply_sync(&actor, team, project, changed_original)
            .await,
        Err(Error::Conflict)
    ));
    let collision = TeamSyncMutation {
        idempotency_key: Uuid::new_v4(),
        ..original
    };
    assert!(matches!(
        service.apply_sync(&actor, team, other, collision).await,
        Err(Error::Conflict)
    ));
    assert!(
        service
            .orders(&actor, team, other)
            .await
            .unwrap()
            .is_empty()
    );
}
