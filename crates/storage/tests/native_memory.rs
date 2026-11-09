use cowork_contracts::*;
use cowork_domain::Error;
use cowork_storage::{Store, library_config::LibraryOwner};
use serde_json::json;
async fn fixture(store: &Store) -> (Project, Order, Order) {
    let project = store
        .create_project(CreateProject {
            name: "Memory".into(),
        })
        .await
        .unwrap();
    let a = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "First chat".into(),
        })
        .await
        .unwrap();
    let b = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Second chat".into(),
        })
        .await
        .unwrap();
    (project, a, b)
}
fn memory(title: &str, scope: MemoryScope, key: Option<&str>) -> WriteKnowledge {
    WriteKnowledge {
        location: None,
        memory: Some(MemoryMetadata {
            scope,
            profile_key: key.map(str::to_owned),
            ..Default::default()
        }),
        id: None,
        expected_revision: 0,
        kind: KnowledgeKind::Memory,
        title: title.into(),
        content: format!("Evidence {title}"),
        tags: vec![],
        provenance: None,
    }
}
fn spec(project: &Project, order: &Order, agent: Option<&AgentTemplate>) -> RunSpec {
    serde_json::from_value(json!({"project_id":project.id,"order_id":order.id,"prompt":"Inspect evidence","provider":{"id":"fixture","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true},"access_mode":"read_only","executor":"local-v2","policy_revision":1,"allowed_tools":[],"budgets":Budgets::default(),"folders":[],"mcp_tools":[],"environment":null,"file_action":null,"workflow":null,"output":null,"model_binding":null,"agent_config":agent.map(|a|EffectiveAgentConfig{agent_source:Some(LibrarySource{id:a.id.clone(),revision:a.revision}),..Default::default()})})).unwrap()
}
async fn preview(store: &Store, spec: &RunSpec) -> String {
    serde_json::to_string(&store.preview_model_context(spec).await.unwrap()).unwrap()
}
async fn agent(store: &Store) -> AgentTemplate {
    store
        .library_write_agent_template(
            LibraryOwner::Local,
            None,
            &WriteAgentTemplate {
                idempotency_key: "agent-create".into(),
                expected_revision: 0,
                value: AgentTemplateDefinition {
                    name: "Researcher".into(),
                    configuration: AgentConfiguration {
                        instructions: "Use evidence.".into(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap()
}
#[tokio::test]
async fn user_memory_crosses_projects_only_when_explicitly_scoped_and_current() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (source, _, _) = fixture(&store).await;
    let (target, chat, _) = fixture(&store).await;
    let global = store
        .productivity_write_knowledge(
            &source.id,
            memory("GLOBAL_PROFILE", MemoryScope::User, Some("language")),
        )
        .await
        .unwrap();
    store
        .productivity_write_knowledge(
            &source.id,
            memory("PRIVATE_PROJECT", MemoryScope::Project, Some("source-only")),
        )
        .await
        .unwrap();
    let target_spec = spec(&target, &chat, None);
    let text = preview(&store, &target_spec).await;
    assert!(text.contains("GLOBAL_PROFILE"));
    assert!(!text.contains("PRIVATE_PROJECT"));
    let hits = store
        .productivity_knowledge(&target.id, "", None, 200)
        .await
        .unwrap();
    assert!(hits.iter().any(|hit| hit.document.id == global.id));
    let request:CreateRun=serde_json::from_value(json!({"order_id":chat.id,"base_revision":chat.revision,"idempotency_key":"global-memory-run","prompt":target_spec.prompt,"provider_id":"fixture","provider_revision":1,"access_mode":"read_only"})).unwrap();
    let accepted = store
        .create_run(&request, target_spec.clone())
        .await
        .unwrap();
    let frozen = store
        .stored_run(&accepted.id)
        .await
        .unwrap()
        .checkpoint
        .messages;
    let mut disabled = memory("GLOBAL_PROFILE", MemoryScope::User, Some("language"));
    disabled.id = Some(global.id.clone());
    disabled.expected_revision = global.revision;
    disabled.memory.as_mut().unwrap().enabled = false;
    store
        .productivity_write_knowledge(&source.id, disabled)
        .await
        .unwrap();
    assert!(
        !preview(&store, &target_spec)
            .await
            .contains("GLOBAL_PROFILE")
    );
    assert_eq!(
        store
            .stored_run(&accepted.id)
            .await
            .unwrap()
            .checkpoint
            .messages,
        frozen
    );
    let mut explicit = target_spec;
    explicit.agent_config = Some(EffectiveAgentConfig {
        memory_ids: Some(vec![global.id]),
        ..Default::default()
    });
    assert!(matches!(
        store.preview_model_context(&explicit).await,
        Err(Error::Validation(_))
    ));
    store
        .productivity_write_knowledge(
            &source.id,
            memory("RETIRED_SOURCE_MEMORY", MemoryScope::User, None),
        )
        .await
        .unwrap();
    let mut current = explicit.clone();
    current.agent_config = None;
    assert!(
        preview(&store, &current)
            .await
            .contains("RETIRED_SOURCE_MEMORY")
    );
    let settings = store.project_settings(&source.id).await.unwrap();
    store
        .delete_native_project(
            &source.id,
            &DeleteLibraryEntry {
                expected_revision: settings.revision,
                idempotency_key: "retire-memory-source".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        !preview(&store, &current)
            .await
            .contains("RETIRED_SOURCE_MEMORY")
    );
    assert!(
        !store
            .productivity_knowledge(&target.id, "", None, 200)
            .await
            .unwrap()
            .iter()
            .any(|hit| hit.document.project_id == source.id)
    );
}
#[tokio::test]
async fn memories_apply_only_to_the_actual_project_chat_and_selected_agent() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, a, b) = fixture(&store).await;
    let agent = agent(&store).await;
    store
        .productivity_write_knowledge(
            &project.id,
            memory("PROJECT_ONLY", MemoryScope::Project, None),
        )
        .await
        .unwrap();
    store
        .productivity_write_knowledge(
            &project.id,
            memory(
                "CHAT_ONLY",
                MemoryScope::Chat {
                    order_id: a.id.clone(),
                },
                None,
            ),
        )
        .await
        .unwrap();
    store
        .productivity_write_knowledge(
            &project.id,
            memory(
                "AGENT_ONLY",
                MemoryScope::Agent {
                    agent_id: agent.id.clone(),
                },
                None,
            ),
        )
        .await
        .unwrap();
    let mut disabled = memory("DISABLED", MemoryScope::Project, None);
    disabled.memory.as_mut().unwrap().enabled = false;
    store
        .productivity_write_knowledge(&project.id, disabled)
        .await
        .unwrap();
    let (other, foreign, _) = fixture(&store).await;
    store
        .productivity_write_knowledge(
            &other.id,
            memory("OTHER_PROJECT", MemoryScope::Project, None),
        )
        .await
        .unwrap();
    let first = preview(&store, &spec(&project, &a, None)).await;
    assert!(first.contains("PROJECT_ONLY"));
    assert!(first.contains("CHAT_ONLY"));
    assert!(!first.contains("AGENT_ONLY"));
    assert!(!first.contains("DISABLED"));
    assert!(!first.contains("OTHER_PROJECT"));
    let second = preview(&store, &spec(&project, &b, Some(&agent))).await;
    assert!(second.contains("PROJECT_ONLY"));
    assert!(second.contains("AGENT_ONLY"));
    assert!(!second.contains("CHAT_ONLY"));
    assert!(matches!(
        store
            .productivity_write_knowledge(
                &project.id,
                memory(
                    "Bad chat",
                    MemoryScope::Chat {
                        order_id: foreign.id
                    },
                    None
                )
            )
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .productivity_write_knowledge(
                &project.id,
                memory(
                    "Bad agent",
                    MemoryScope::Agent {
                        agent_id: "foreign-agent".into()
                    },
                    None
                )
            )
            .await,
        Err(Error::Forbidden)
    ));
}
#[tokio::test]
async fn profile_keys_have_deterministic_scope_precedence_without_keyword_matches() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, a, b) = fixture(&store).await;
    let agent = agent(&store).await;
    for (title, scope) in [
        ("PROJECT_PROFILE", MemoryScope::Project),
        (
            "AGENT_PROFILE",
            MemoryScope::Agent {
                agent_id: agent.id.clone(),
            },
        ),
        (
            "CHAT_PROFILE",
            MemoryScope::Chat {
                order_id: a.id.clone(),
            },
        ),
    ] {
        store
            .productivity_write_knowledge(
                &project.id,
                memory(title, scope, Some("response.language")),
            )
            .await
            .unwrap();
    }
    let mut current = spec(&project, &a, Some(&agent));
    current.prompt = "Hello".into();
    let text = preview(&store, &current).await;
    assert!(text.contains("CHAT_PROFILE"));
    assert!(!text.contains("AGENT_PROFILE"));
    assert!(!text.contains("PROJECT_PROFILE"));
    current.order_id = b.id;
    let text = preview(&store, &current).await;
    assert!(text.contains("AGENT_PROFILE"));
    assert!(!text.contains("PROJECT_PROFILE"));
    current.agent_config = None;
    assert!(preview(&store, &current).await.contains("PROJECT_PROFILE"));
}
#[tokio::test]
async fn memory_edits_preserve_scoping_for_legacy_clients_and_freeze_accepted_context() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, a, _) = fixture(&store).await;
    let mut initial = memory(
        "ORIGINAL_MEMORY",
        MemoryScope::Chat {
            order_id: a.id.clone(),
        },
        None,
    );
    initial.provenance=Some(serde_json::from_value(json!({"format":1,"origin":"retrieved","sources":[{"format":1,"source_uri":"https://example.com/evidence","revision":"1","sha256":"a".repeat(64),"locator":"section 1","verification":"declared"}]})).unwrap());
    let saved = store
        .productivity_write_knowledge(&project.id, initial.clone())
        .await
        .unwrap();
    let spec = spec(&project, &a, None);
    let request:CreateRun=serde_json::from_value(json!({"order_id":a.id,"base_revision":a.revision,"idempotency_key":"memory-run","prompt":spec.prompt,"provider_id":"fixture","provider_revision":1,"access_mode":"read_only"})).unwrap();
    let run = store.create_run(&request, spec.clone()).await.unwrap();
    let frozen = store.stored_run(&run.id).await.unwrap().checkpoint.messages;
    let mut updated = initial;
    updated.id = Some(saved.id.clone());
    updated.expected_revision = 1;
    updated.memory = None;
    updated.content = "Evidence UPDATED_MEMORY".into();
    let second = store
        .productivity_write_knowledge(&project.id, updated.clone())
        .await
        .unwrap();
    assert!(matches!(
        second.memory.as_ref().unwrap().scope,
        MemoryScope::Chat { .. }
    ));
    updated.expected_revision = 2;
    updated.memory = second.memory.clone();
    updated.memory.as_mut().unwrap().enabled = false;
    updated.provenance = Some(KnowledgeProvenance::user_authored());
    store
        .productivity_write_knowledge(&project.id, updated)
        .await
        .unwrap();
    assert!(!preview(&store, &spec).await.contains("UPDATED_MEMORY"));
    assert_eq!(
        store.stored_run(&run.id).await.unwrap().checkpoint.messages,
        frozen
    );
    let revisions = store
        .knowledge_revisions(&project.id, &saved.id, 0, 100)
        .await
        .unwrap();
    assert_eq!(revisions.len(), 3);
    assert_eq!(revisions[0].document["memory"]["scope"]["kind"], "chat");
    assert_eq!(revisions[2].document["memory"]["enabled"], false);
    assert_eq!(revisions[0].provenance.sources.len(), 1);
    assert!(revisions[2].provenance.sources.is_empty());
}

#[tokio::test]
async fn explicit_context_ids_include_each_selected_document_without_keywords_and_refuse_stale_refs()
 {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, a, _) = fixture(&store).await;
    let mut ids = Vec::new();
    let mut last = None;
    for index in 0..7 {
        let doc = store
            .productivity_write_knowledge(
                &project.id,
                memory(&format!("SELECTED_{index}"), MemoryScope::Project, None),
            )
            .await
            .unwrap();
        ids.push(doc.id.clone());
        last = Some(doc);
    }
    let mut current = spec(&project, &a, None);
    current.prompt = "Hello".into();
    current.agent_config = Some(EffectiveAgentConfig {
        memory_ids: Some(ids),
        ..Default::default()
    });
    let text = preview(&store, &current).await;
    for index in 0..7 {
        assert!(text.contains(&format!("SELECTED_{index}")));
    }
    let mut missing = current.clone();
    missing.agent_config.as_mut().unwrap().memory_ids = Some(vec!["missing".into()]);
    assert!(matches!(
        store.preview_model_context(&missing).await,
        Err(Error::Validation(_))
    ));
    let last = last.unwrap();
    let mut metadata = last.memory.clone().unwrap();
    metadata.enabled = false;
    store
        .productivity_write_knowledge(
            &project.id,
            WriteKnowledge {
                location: None,
                memory: Some(metadata),
                provenance: None,
                id: Some(last.id),
                expected_revision: last.revision,
                kind: KnowledgeKind::Memory,
                title: last.title,
                content: last.content,
                tags: vec![],
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        store.preview_model_context(&current).await,
        Err(Error::Validation(_))
    ));
    current.agent_config.as_mut().unwrap().memory_ids = None;
    current.agent_config.as_mut().unwrap().skill_ids =
        Some(vec!["foreign-or-deleted-skill".into()]);
    assert!(matches!(
        store.preview_model_context(&current).await,
        Err(Error::Validation(_))
    ));
}
