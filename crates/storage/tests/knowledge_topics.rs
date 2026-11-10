use cowork_contracts::*;
use cowork_storage::{
    ProductivityImportPolicy, ProductivityImportRequest, Store, WriteProjectKnowledgeAccess,
};
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

fn folder(name: &str, parent: Option<&str>) -> KnowledgeFolder {
    KnowledgeFolder {
        id: Uuid::new_v4().to_string(),
        name: name.into(),
        parent_id: parent.map(str::to_string),
    }
}
fn note(title: &str, content: &str, folder: Option<&str>, tags: &[&str]) -> WriteKnowledge {
    WriteKnowledge {
        location: Some(KnowledgeLocation {
            folder_id: folder.map(str::to_string),
        }),
        memory: None,
        provenance: None,
        id: None,
        expected_revision: 0,
        kind: KnowledgeKind::Knowledge,
        title: title.into(),
        content: content.into(),
        tags: tags.iter().map(|s| (*s).into()).collect(),
    }
}
async fn setup(store: &Store) -> (Project, KnowledgeFolder, KnowledgeFolder, KnowledgeFolder) {
    let p = store
        .create_project(CreateProject {
            name: "Research".into(),
        })
        .await
        .unwrap();
    let root = folder("AI", None);
    let child = folder("Quantization", Some(&root.id));
    let other = folder("Finance", None);
    store
        .write_knowledge_folders(
            &p.id,
            false,
            WriteKnowledgeFolders {
                expected_revision: 0,
                folders: vec![root.clone(), child.clone(), other.clone()],
            },
        )
        .await
        .unwrap();
    (p, root, child, other)
}
fn search(folder: &KnowledgeFolder, descendants: bool, query: &str) -> SearchKnowledgeGraph {
    SearchKnowledgeGraph {
        folder_id: folder.id.clone(),
        include_descendants: descendants,
        query: query.into(),
    }
}
async fn build(
    store: &Store,
    p: &Project,
    folder: &KnowledgeFolder,
    descendants: bool,
) -> KnowledgeTopicGraph {
    store
        .build_knowledge_graph(
            &p.id,
            false,
            BuildKnowledgeGraph {
                folder_id: folder.id.clone(),
                include_descendants: descendants,
            },
        )
        .await
        .unwrap()
}
fn run(p: &Project, order: &Order, prompt: &str) -> RunSpec {
    serde_json::from_value(json!({"project_id":p.id,"order_id":order.id,"prompt":prompt,"provider":{"id":"fixture","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true},"access_mode":"read_only","executor":"local-v2","policy_revision":1,"allowed_tools":[],"budgets":Budgets::default(),"folders":[],"mcp_tools":[],"environment":null,"file_action":null,"workflow":null,"output":null,"model_binding":null,"agent_config":null})).unwrap()
}

#[tokio::test]
async fn hierarchy_rejects_cycles_duplicate_siblings_and_foreign_locations() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (p, root, child, _) = setup(&store).await;
    let foreign = store
        .create_project(CreateProject {
            name: "Other".into(),
        })
        .await
        .unwrap();
    assert!(
        store
            .productivity_write_knowledge(&foreign.id, note("Foreign", "text", Some(&root.id), &[]))
            .await
            .is_err()
    );
    let current = store.knowledge_folders(&p.id, false).await.unwrap();
    for folders in [
        vec![
            KnowledgeFolder {
                parent_id: Some(child.id.clone()),
                ..root.clone()
            },
            child.clone(),
        ],
        vec![
            root.clone(),
            KnowledgeFolder {
                id: Uuid::new_v4().to_string(),
                name: "ai".into(),
                parent_id: None,
            },
        ],
        vec![child.clone()],
    ] {
        assert!(
            store
                .write_knowledge_folders(
                    &p.id,
                    false,
                    WriteKnowledgeFolders {
                        expected_revision: 1,
                        folders
                    }
                )
                .await
                .is_err()
        );
    }
    assert_eq!(
        store.knowledge_folders(&p.id, false).await.unwrap().folders,
        current.folders
    );
    let request = WriteKnowledgeFolders {
        expected_revision: 1,
        folders: vec![root.clone(), child.clone()],
    };
    let saved = store
        .write_knowledge_folders(&p.id, false, request.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .write_knowledge_folders(&p.id, false, request)
            .await
            .unwrap()
            .revision,
        saved.revision
    );
    let mut personal = note("Personal", "content", Some(&root.id), &[]);
    personal.kind = KnowledgeKind::Memory;
    personal.memory = Some(MemoryMetadata {
        scope: MemoryScope::User,
        ..Default::default()
    });
    assert!(
        store
            .productivity_write_knowledge(&p.id, personal)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn imports_and_moves_are_atomic_and_nonempty_folders_cannot_be_removed() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (p, root, child, _) = setup(&store).await;
    let imported = KnowledgeImportRequest {
        idempotency_key: Uuid::new_v4(),
        documents: vec![
            note("Part1", "material", Some(&child.id), &[]),
            note("Part2", "more", Some(&child.id), &[]),
        ],
    };
    let saved = store
        .import_knowledge(&p.id, imported.clone())
        .await
        .unwrap();
    assert!(
        saved
            .documents
            .iter()
            .all(|d| d.folder_id.as_deref() == Some(&child.id))
    );
    assert_eq!(
        store
            .import_knowledge(&p.id, imported)
            .await
            .unwrap()
            .documents[0]
            .id,
        saved.documents[0].id
    );
    let bad = KnowledgeImportRequest {
        idempotency_key: Uuid::new_v4(),
        documents: vec![
            note("Rollback", "material", Some(&root.id), &[]),
            note("Invalid", "more", Some("missing"), &[]),
        ],
    };
    assert!(store.import_knowledge(&p.id, bad).await.is_err());
    assert_eq!(
        store
            .productivity_knowledge(&p.id, "", None, 2000)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        store
            .write_knowledge_folders(
                &p.id,
                false,
                WriteKnowledgeFolders {
                    expected_revision: 1,
                    folders: vec![root]
                }
            )
            .await
            .is_err()
    );
    let d = &saved.documents[0];
    let mut edit = note("Edited", "changed", None, &[]);
    edit.id = Some(d.id.clone());
    edit.expected_revision = d.revision;
    edit.location = None;
    let retained = store
        .productivity_write_knowledge(&p.id, edit.clone())
        .await
        .unwrap();
    assert_eq!(retained.folder_id, d.folder_id);
    edit.expected_revision = retained.revision;
    edit.location = Some(KnowledgeLocation { folder_id: None });
    assert!(
        store
            .productivity_write_knowledge(&p.id, edit)
            .await
            .unwrap()
            .folder_id
            .is_none()
    );
}

#[tokio::test]
async fn graphs_expand_real_tag_links_only_inside_the_chosen_subtree_and_reject_stale_content() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (p, root, child, other) = setup(&store).await;
    let seed = store
        .productivity_write_knowledge(
            &p.id,
            note(
                "NVFP4",
                "Blackwell measurement",
                Some(&root.id),
                &["precision"],
            ),
        )
        .await
        .unwrap();
    let related = store
        .productivity_write_knowledge(
            &p.id,
            note(
                "Neighbor",
                "Additional evidence",
                Some(&child.id),
                &["Precision"],
            ),
        )
        .await
        .unwrap();
    let excluded = store
        .productivity_write_knowledge(
            &p.id,
            note(
                "Outside",
                "External material",
                Some(&other.id),
                &["precision"],
            ),
        )
        .await
        .unwrap();
    assert_eq!(build(&store, &p, &root, false).await.documents, 1);
    assert_eq!(
        store
            .search_knowledge_graph(&p.id, false, search(&root, false, "NVFP4"))
            .await
            .unwrap()
            .hits
            .len(),
        1
    );
    assert_eq!(build(&store, &p, &root, true).await.documents, 2);
    let hits = store
        .search_knowledge_graph(&p.id, false, search(&root, true, "NVFP4"))
        .await
        .unwrap()
        .hits;
    assert!(
        hits.iter()
            .any(|h| h.document.id == seed.id && h.via == "direct")
    );
    assert!(
        hits.iter()
            .any(|h| h.document.id == related.id && h.via == "tag:precision")
    );
    assert!(!hits.iter().any(|h| h.document.id == excluded.id));
    assert!(
        store
            .search_knowledge_graph(&p.id, false, search(&root, true, "unmatchedword"))
            .await
            .unwrap()
            .hits
            .is_empty()
    );
    let mut edit = note(
        "Neighbor",
        "revised evidence",
        Some(&child.id),
        &["precision"],
    );
    edit.id = Some(related.id);
    edit.expected_revision = 1;
    store
        .productivity_write_knowledge(&p.id, edit)
        .await
        .unwrap();
    assert!(
        store
            .knowledge_graphs(&p.id, false)
            .await
            .unwrap()
            .iter()
            .find(|g| g.include_descendants)
            .unwrap()
            .stale
    );
    assert!(
        store
            .search_knowledge_graph(&p.id, false, search(&root, true, "NVFP4"))
            .await
            .is_err()
    );
    build(&store, &p, &root, true).await;
    assert_eq!(
        store
            .search_knowledge_graph(&p.id, false, search(&root, true, "NVFP4"))
            .await
            .unwrap()
            .hits
            .len(),
        2
    );
}

#[tokio::test]
async fn graph_neighbors_enter_new_run_context_only_under_existing_access_and_memory_scopes() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (p, root, _, _) = setup(&store).await;
    let order = store
        .create_order(CreateOrder {
            project_id: p.id.clone(),
            title: "Ask".into(),
        })
        .await
        .unwrap();
    store
        .productivity_write_knowledge(
            &p.id,
            note("NVFP4", "Seed document", Some(&root.id), &["link"]),
        )
        .await
        .unwrap();
    store
        .productivity_write_knowledge(
            &p.id,
            note("CONNECTED", "Other evidence", Some(&root.id), &["link"]),
        )
        .await
        .unwrap();
    let mut restricted = note("HIDDEN", "private chat material", Some(&root.id), &["link"]);
    restricted.kind = KnowledgeKind::Memory;
    restricted.memory = Some(MemoryMetadata {
        scope: MemoryScope::Chat {
            order_id: store
                .create_order(CreateOrder {
                    project_id: p.id.clone(),
                    title: "Other".into(),
                })
                .await
                .unwrap()
                .id,
        },
        ..Default::default()
    });
    store
        .productivity_write_knowledge(&p.id, restricted)
        .await
        .unwrap();
    let spec = run(&p, &order, "NVFP4");
    let before = serde_json::to_string(&store.preview_model_context(&spec).await.unwrap()).unwrap();
    assert!(!before.contains("CONNECTED"));
    build(&store, &p, &root, true).await;
    let text = serde_json::to_string(&store.preview_model_context(&spec).await.unwrap()).unwrap();
    assert!(text.contains("CONNECTED") && text.contains("fingerprint"));
    assert!(!text.contains("HIDDEN"));
    store
        .write_project_knowledge_access(
            &p.id,
            &WriteProjectKnowledgeAccess {
                expected_revision: 0,
                project_knowledge: false,
                personal_knowledge: true,
            },
        )
        .await
        .unwrap();
    let text = serde_json::to_string(&store.preview_model_context(&spec).await.unwrap()).unwrap();
    assert!(!text.contains("CONNECTED") && !text.contains("Seed document"));
}

#[tokio::test]
async fn folders_and_graphs_survive_reopen_and_archive_copy_remaps_locations() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("knowledge.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let (p, root, child, _) = setup(&store).await;
    store
        .productivity_write_knowledge(
            &p.id,
            note("Persisted", "NVFP4 material", Some(&child.id), &["test"]),
        )
        .await
        .unwrap();
    let graph = build(&store, &p, &root, true).await;
    drop(store);
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        store
            .knowledge_folders(&p.id, false)
            .await
            .unwrap()
            .folders
            .len(),
        3
    );
    assert_eq!(
        store
            .search_knowledge_graph(&p.id, false, search(&root, true, "NVFP4"))
            .await
            .unwrap()
            .fingerprint,
        graph.fingerprint
    );
    let archive = store.productivity_export(Some(&p.id)).await.unwrap();
    let imported = store
        .productivity_import(ProductivityImportRequest {
            idempotency_key: Uuid::new_v4().to_string(),
            policy: ProductivityImportPolicy::Copy,
            expected_project_revisions: BTreeMap::new(),
            expected_order_revisions: BTreeMap::new(),
            archive,
        })
        .await
        .unwrap();
    let copy = &imported.projects[0].project;
    let folders = store.knowledge_folders(&copy.id, false).await.unwrap();
    let docs = store
        .productivity_knowledge(&copy.id, "", None, 2000)
        .await
        .unwrap();
    assert_eq!(docs.len(), 1);
    assert_ne!(docs[0].document.folder_id.as_ref(), Some(&child.id));
    assert!(folders.folders.iter().any(|f| Some(&f.id)==docs[0].document.folder_id.as_ref()&&f.name=="Quantization"));
    assert!(
        store
            .knowledge_graphs(&copy.id, false)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn source_links_expand_without_tags_and_removing_graph_disables_expansion() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (p, root, _, _) = setup(&store).await;
    let source = KnowledgeSource {
        format: 1,
        source_uri: Some("urn:cowork:graph-source-test".into()),
        file_id: None,
        artifact_id: None,
        revision: "1".into(),
        sha256: "a".repeat(64),
        locator: "page 1".into(),
        verification: KnowledgeSourceVerification::Declared,
    };
    for (title, content) in [("NVFP4", "seed"), ("Source neighbor", "related material")] {
        let mut request = note(title, content, Some(&root.id), &[]);
        request.provenance = Some(KnowledgeProvenance {
            format: 1,
            origin: KnowledgeOrigin::Imported,
            sources: vec![source.clone()],
        });
        store
            .productivity_write_knowledge(&p.id, request)
            .await
            .unwrap();
    }
    build(&store, &p, &root, false).await;
    let result = store
        .search_knowledge_graph(&p.id, false, search(&root, false, "NVFP4"))
        .await
        .unwrap();
    assert_eq!(result.hits.len(), 2);
    assert!(result.hits.iter().any(|hit| hit.via.starts_with("source:")));
    store
        .delete_knowledge_graph(
            &p.id,
            false,
            BuildKnowledgeGraph {
                folder_id: root.id.clone(),
                include_descendants: false,
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .search_knowledge_graph(&p.id, false, search(&root, false, "NVFP4"))
            .await
            .is_err()
    );
    assert!(
        store
            .knowledge_graphs(&p.id, false)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn personal_topics_work_across_source_projects_and_respect_the_owner_switch() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (p, _, _, _) = setup(&store).await;
    let target = store
        .create_project(CreateProject {
            name: "Target".into(),
        })
        .await
        .unwrap();
    let topic = folder("Personal topic", None);
    store
        .write_knowledge_folders(
            &p.id,
            true,
            WriteKnowledgeFolders {
                expected_revision: 0,
                folders: vec![topic.clone()],
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .knowledge_folders(&target.id, true)
            .await
            .unwrap()
            .folders,
        vec![topic.clone()]
    );
    for title in ["NVFP4", "PERSONAL_NEIGHBOR"] {
        let mut request = note(
            title,
            "Personal research",
            Some(&topic.id),
            &["personal-link"],
        );
        request.kind = KnowledgeKind::Memory;
        request.memory = Some(MemoryMetadata {
            scope: MemoryScope::User,
            ..Default::default()
        });
        store
            .productivity_write_knowledge(&p.id, request)
            .await
            .unwrap();
    }
    store
        .build_knowledge_graph(
            &target.id,
            true,
            BuildKnowledgeGraph {
                folder_id: topic.id.clone(),
                include_descendants: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .search_knowledge_graph(&target.id, true, search(&topic, true, "NVFP4"))
            .await
            .unwrap()
            .hits
            .len(),
        2
    );
    let order = store
        .create_order(CreateOrder {
            project_id: target.id.clone(),
            title: "Ask personal".into(),
        })
        .await
        .unwrap();
    let spec = run(&target, &order, "NVFP4");
    assert!(
        serde_json::to_string(&store.preview_model_context(&spec).await.unwrap())
            .unwrap()
            .contains("PERSONAL_NEIGHBOR")
    );
    store
        .write_owner_knowledge_access(&cowork_storage::WriteOwnerKnowledgeAccess {
            expected_revision: 0,
            personal_knowledge: false,
        })
        .await
        .unwrap();
    assert!(
        !serde_json::to_string(&store.preview_model_context(&spec).await.unwrap())
            .unwrap()
            .contains("PERSONAL_NEIGHBOR")
    );
}

#[tokio::test]
async fn disabling_personal_entries_does_not_release_their_folder() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (p, _, _, _) = setup(&store).await;
    let topic = folder("Retained personal folder", None);
    store
        .write_knowledge_folders(
            &p.id,
            true,
            WriteKnowledgeFolders {
                expected_revision: 0,
                folders: vec![topic.clone()],
            },
        )
        .await
        .unwrap();
    let mut request = note("Disabled entry", "retained material", Some(&topic.id), &[]);
    request.kind = KnowledgeKind::Memory;
    request.memory = Some(MemoryMetadata {
        scope: MemoryScope::User,
        enabled: false,
        ..Default::default()
    });
    store
        .productivity_write_knowledge(&p.id, request)
        .await
        .unwrap();
    assert!(
        store
            .write_knowledge_folders(
                &p.id,
                true,
                WriteKnowledgeFolders {
                    expected_revision: 1,
                    folders: vec![]
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        store.knowledge_folders(&p.id, true).await.unwrap().folders,
        vec![topic]
    );
}
