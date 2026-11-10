use cowork_contracts::{CreateProject, KnowledgeOrigin, KnowledgeProvenance};
use cowork_storage::{KnowledgeImportRequest, KnowledgeKind, Store, WriteKnowledge};
use uuid::Uuid;

fn chunk(title: &str, content: &str) -> WriteKnowledge {
    WriteKnowledge {
        location: None,
        memory: None,
        id: None,
        expected_revision: 0,
        kind: KnowledgeKind::Knowledge,
        title: title.into(),
        content: content.into(),
        tags: vec!["imported".into()],
        provenance: Some(KnowledgeProvenance {
            format: 1,
            origin: KnowledgeOrigin::UserAuthored,
            sources: vec![],
        }),
    }
}
#[tokio::test]
async fn batch_is_project_scoped_atomic_and_replays_original_receipt() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "knowledge".into(),
        })
        .await
        .unwrap();
    let other = store
        .create_project(CreateProject {
            name: "other".into(),
        })
        .await
        .unwrap();
    let input = KnowledgeImportRequest {
        idempotency_key: Uuid::new_v4(),
        documents: vec![
            chunk("First", "first material"),
            chunk("Second", "second material"),
        ],
    };
    let result = store
        .import_knowledge(&project.id, input.clone())
        .await
        .unwrap();
    assert_eq!(result.documents.len(), 2);
    let replay = store
        .import_knowledge(&project.id, input.clone())
        .await
        .unwrap();
    assert_eq!(
        replay.documents.iter().map(|d| &d.id).collect::<Vec<_>>(),
        result.documents.iter().map(|d| &d.id).collect::<Vec<_>>()
    );
    assert_eq!(
        store
            .productivity_knowledge(&project.id, "", None, 200)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        store
            .productivity_knowledge(&other.id, "", None, 200)
            .await
            .unwrap()
            .is_empty()
    );
    let mut changed = input.clone();
    changed.documents[0].content = "different".into();
    assert!(store.import_knowledge(&project.id, changed).await.is_err());
    // The same client key in a different project cannot replay the first project's objects.
    let scoped = store.import_knowledge(&other.id, input).await.unwrap();
    assert_ne!(scoped.documents[0].id, result.documents[0].id);
    assert_eq!(scoped.project_id, other.id);
    let invalid = KnowledgeImportRequest {
        idempotency_key: Uuid::new_v4(),
        documents: vec![chunk("Valid", "not committed"), chunk("Invalid", "")],
    };
    assert!(store.import_knowledge(&project.id, invalid).await.is_err());
    assert_eq!(
        store
            .productivity_knowledge(&project.id, "", None, 200)
            .await
            .unwrap()
            .len(),
        2
    );
    let mut foreign_source = chunk("Invalid scoped snapshot", "must not persist");
    foreign_source.provenance = Some(cowork_contracts::KnowledgeProvenance {
        format: 1,
        origin: KnowledgeOrigin::Imported,
        sources: vec![cowork_contracts::KnowledgeSource {
            format: 1,
            file_id: Some(Uuid::new_v4().to_string()),
            artifact_id: None,
            source_uri: None,
            revision: "1".into(),
            sha256: "a".repeat(64),
            locator: "line 1".into(),
            verification: cowork_contracts::KnowledgeSourceVerification::Declared,
        }],
    });
    // The second chunk passes validation but fails scoped snapshot lookup after the first write.
    let late_failure = KnowledgeImportRequest {
        idempotency_key: Uuid::new_v4(),
        documents: vec![chunk("Rolled back", "must not persist"), foreign_source],
    };
    assert!(
        store
            .import_knowledge(&project.id, late_failure)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .productivity_knowledge(&project.id, "", None, 200)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn oversized_chunks_and_updates_cannot_enter_the_import_path() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "limits".into(),
        })
        .await
        .unwrap();
    for mut document in [
        chunk("Too long", &"x".repeat(4001)),
        chunk("Existing id", "content"),
    ] {
        if document.title == "Existing id" {
            document.id = Some(Uuid::new_v4().to_string());
        }
        assert!(
            store
                .import_knowledge(
                    &project.id,
                    KnowledgeImportRequest {
                        idempotency_key: Uuid::new_v4(),
                        documents: vec![document]
                    }
                )
                .await
                .is_err()
        );
    }
    assert!(
        store
            .productivity_knowledge(&project.id, "", None, 200)
            .await
            .unwrap()
            .is_empty()
    );
}
