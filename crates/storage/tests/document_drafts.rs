use cowork_contracts::*;
use cowork_storage::Store;
use serde_json::json;

#[tokio::test]
async fn document_drafts_keep_conflicts_and_replay_the_original_save() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("drafts.db")).await.unwrap();
    document_draft_contract(store).await;
}

#[tokio::test]
#[ignore = "Requires a dedicated PostgreSQL database in COWORK_TEST_POSTGRES"]
async fn postgres_document_draft_contract() {
    document_draft_contract(
        Store::connect(&std::env::var("COWORK_TEST_POSTGRES").unwrap())
            .await
            .unwrap(),
    )
    .await;
}

async fn document_draft_contract(store: Store) {
    let project = store
        .create_project(CreateProject {
            name: "Docs".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "Drafts".into(),
        })
        .await
        .unwrap();
    let request = SaveDocumentDraft {
        id: "draft".into(),
        base_revision: 0,
        idempotency_key: "save-1".into(),
        content: json!({"format":"tex","body":"First"}),
    };
    let first = store
        .save_document_draft(&order.id, &request)
        .await
        .unwrap();
    assert!(first.saved);
    assert_eq!(first.draft.revision, 1);
    let replay = store
        .save_document_draft(&order.id, &request)
        .await
        .unwrap();
    assert_eq!(replay.snapshot.id, first.snapshot.id);
    let mut changed = request.clone();
    changed.content = json!({"body":"Other"});
    assert!(
        store
            .save_document_draft(&order.id, &changed)
            .await
            .is_err()
    );
    changed.idempotency_key = "save-2".into();
    let conflict = store
        .save_document_draft(&order.id, &changed)
        .await
        .unwrap();
    assert!(!conflict.saved);
    assert!(conflict.snapshot.conflict);
    assert_eq!(conflict.draft.content, request.content);
    assert_eq!(conflict.snapshot.content, changed.content);
    changed.idempotency_key = "save-3".into();
    changed.base_revision = 1;
    assert_eq!(
        store
            .save_document_draft(&order.id, &changed)
            .await
            .unwrap()
            .draft
            .revision,
        2
    );
    let snapshots = store
        .document_draft_snapshots(&order.id, "draft")
        .await
        .unwrap();
    assert_eq!(snapshots.len(), 3);
    assert_eq!(
        store
            .save_document_draft(&order.id, &request)
            .await
            .unwrap()
            .draft
            .revision,
        1
    );
    assert_eq!(
        store
            .document_draft(&order.id, "draft")
            .await
            .unwrap()
            .revision,
        2
    );
    assert!(
        store
            .document_draft("foreign-order", "draft")
            .await
            .is_err()
    );
    let left = SaveDocumentDraft {
        id: "race".into(),
        base_revision: 0,
        idempotency_key: "race-left".into(),
        content: json!({"body":"Left"}),
    };
    let right = SaveDocumentDraft {
        id: "race".into(),
        base_revision: 0,
        idempotency_key: "race-right".into(),
        content: json!({"body":"Right"}),
    };
    let (left, right) = tokio::join!(
        store.save_document_draft(&order.id, &left),
        store.save_document_draft(&order.id, &right)
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_ne!(left.saved, right.saved);
    assert_eq!(left.draft.content, right.draft.content);
    assert_eq!(
        store
            .document_draft_snapshots(&order.id, "race")
            .await
            .unwrap()
            .len(),
        2
    );
}
