//! One explicit local store smoke check, no provider/tool/network/service effects.
use cowork_contracts::*;
use cowork_domain::{Error, hash};
use cowork_storage::*;
use std::collections::BTreeMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scratch = tempfile::tempdir()?;
    let store = Store::sqlite(&scratch.path().join("provenance.db")).await?;
    let project = store
        .create_project(CreateProject {
            name: "Source scope".into(),
        })
        .await?;
    let foreign = store
        .create_project(CreateProject {
            name: "Other scope".into(),
        })
        .await?;
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Source".into(),
        })
        .await?;
    let upload = store
        .reserve_upload(
            &order.id,
            &ReserveUpload {
                idempotency_key: "source".into(),
                file: DownloadMetadata {
                    file_name: "source.txt".into(),
                    size: 14,
                    sha256: hash(b"source excerpt"),
                },
            },
        )
        .await?;
    let upload = store.confirm_upload(&order.id, &upload.id).await?;
    let source = KnowledgeProvenance {
        format: 1,
        origin: KnowledgeOrigin::Retrieved,
        sources: vec![KnowledgeSource {
            format: 1,
            source_uri: None,
            file_id: Some(upload.id.clone()),
            artifact_id: None,
            revision: upload.revision.to_string(),
            sha256: upload.file.sha256.clone(),
            locator: "lines 1-1".into(),
            verification: KnowledgeSourceVerification::Declared,
        }],
    };
    let request = WriteKnowledge {
        location: None,
        memory: None,
        provenance: Some(source),
        id: None,
        expected_revision: 0,
        kind: KnowledgeKind::Memory,
        title: "Bound memory".into(),
        content: "A sourced conclusion".into(),
        tags: vec!["source".into()],
    };
    assert!(matches!(
        store
            .productivity_write_knowledge(&foreign.id, request.clone())
            .await,
        Err(Error::NotFound)
    ));
    let document = store
        .productivity_write_knowledge(&project.id, request)
        .await?;
    assert_eq!(
        document.provenance.sources[0].verification,
        KnowledgeSourceVerification::SnapshotVerified
    );
    let changed = store
        .productivity_write_knowledge(
            &project.id,
            WriteKnowledge {
                location: None,
                memory: document.memory.clone(),
                provenance: None,
                id: Some(document.id.clone()),
                expected_revision: document.revision,
                kind: document.kind.clone(),
                title: document.title.clone(),
                content: "Updated conclusion".into(),
                tags: document.tags.clone(),
            },
        )
        .await?;
    let history = store
        .knowledge_revisions(&project.id, &document.id, 0, 100)
        .await?;
    assert_eq!(history.len(), 2);
    assert_ne!(history[0].content_sha256, history[1].content_sha256);
    assert_eq!(history[0].provenance, history[1].provenance);
    let archive = store.productivity_export(Some(&project.id)).await?;
    assert_eq!(archive.projects[0].knowledge_revisions.len(), 2);
    let copied = store
        .productivity_import(ProductivityImportRequest {
            idempotency_key: "copy".into(),
            policy: ProductivityImportPolicy::Copy,
            expected_project_revisions: BTreeMap::new(),
            expected_order_revisions: BTreeMap::new(),
            archive: ProductivityArchive::from_zip(&archive.to_zip()?)?,
        })
        .await?;
    let docs = store
        .productivity_knowledge(&copied.projects[0].project.id, "", None, 100)
        .await?;
    assert_eq!(docs.len(), 1);
    let imported = &docs[0].document;
    assert_eq!(imported.content, changed.content);
    assert_eq!(imported.provenance.origin, KnowledgeOrigin::Imported);
    assert!(
        imported
            .provenance
            .sources
            .iter()
            .all(|source| source.file_id.is_none()
                && source.artifact_id.is_none()
                && source.verification == KnowledgeSourceVerification::Declared)
    );
    let copied_history = store
        .knowledge_revisions(&imported.project_id, &imported.id, 0, 100)
        .await?;
    assert_eq!(copied_history.len(), 3);
    assert_eq!(copied_history[0].document["content"], document.content);
    assert_eq!(
        copied_history.last().unwrap().content_sha256,
        hash(changed.content.as_bytes())
    );
    store.close().await;
    println!(
        "PASS: actual SQLite migration, scoped source binding, immutable revisions and inert archive copy"
    );
    Ok(())
}
