use cowork_contracts::*;
use cowork_domain::Error;
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::{Store, library_config::LibraryOwner};

async fn fixture() -> (tempfile::TempDir, Runtime, CreateRun) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("settings.sqlite"))
        .await
        .unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Settings proof".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "New Chat".into(),
        })
        .await
        .unwrap();
    let profile = ProviderProfile {
        id: "fixture".into(),
        name: "Fixture".into(),
        kind: ProviderKind::Test,
        endpoint: "".into(),
        model: "fixture".into(),
        revision: 1,
        context_tokens: 32768,
        credential_required: false,
        credential_ready: true,
        generation: Default::default(),
        capabilities: Default::default(),
    };
    store.insert_provider(&profile).await.unwrap();
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        true,
    )
    .unwrap();
    let request = CreateRun {
        order_id: order.id,
        base_revision: order.revision,
        idempotency_key: "original-run".into(),
        prompt: "Hello".into(),
        provider_id: "fixture".into(),
        provider_revision: 1,
        access_mode: AccessMode::ConfirmMutations,
        budgets: Budgets::default(),
        folders: vec![],
        native_tools: vec![],
        mcp_tools: vec![],
        environment: None,
        output: None,
        model_binding: None,
        agent_config: Some(EffectiveAgentConfig {
            instructions: "Copied role instructions".into(),
            generation: GenerationSettings {
                temperature: Some(0.0),
                timeout_ms: Some(3000),
            },
            ..Default::default()
        }),
    };
    (dir, runtime, request)
}

#[tokio::test]
async fn accepted_run_freezes_global_project_and_agent_values() {
    let (_dir, runtime, request) = fixture().await;
    let prefs = runtime
        .store
        .library_preferences(LibraryOwner::Local)
        .await
        .unwrap();
    let mut value = prefs.value;
    value.system_prompt = "Workspace system instructions".into();
    runtime
        .store
        .library_write_preferences(
            LibraryOwner::Local,
            &WritePreferences {
                idempotency_key: "preferences".into(),
                expected_revision: prefs.revision,
                value,
            },
        )
        .await
        .unwrap();
    let order = runtime.store.order(&request.order_id).await.unwrap();
    let project = runtime
        .store
        .project_settings(&order.project_id)
        .await
        .unwrap();
    runtime
        .store
        .write_project_settings(
            &order.project_id,
            &WriteProjectSettings {
                idempotency_key: "project".into(),
                expected_revision: project.revision,
                name: project.name,
                instructions: "Project rules".into(),
                sources: vec![],
                working_folder: None,
            },
        )
        .await
        .unwrap();
    let run = runtime.create_run(&request).await.unwrap();
    let config = run.spec.agent_config.as_ref().unwrap();
    for expected in [
        "Workspace system instructions",
        "Project rules",
        "Copied role instructions",
    ] {
        assert!(config.instructions.contains(expected));
    }
    assert_eq!(config.generation.temperature, Some(0.0));
    assert_eq!(config.generation.timeout_ms, Some(3000));
    let accepted = serde_json::to_value(&run.spec).unwrap();
    let prefs = runtime
        .store
        .library_preferences(LibraryOwner::Local)
        .await
        .unwrap();
    let mut changed = prefs.value;
    changed.system_prompt = "Changed later".into();
    runtime
        .store
        .library_write_preferences(
            LibraryOwner::Local,
            &WritePreferences {
                idempotency_key: "changed".into(),
                expected_revision: prefs.revision,
                value: changed,
            },
        )
        .await
        .unwrap();
    let replay = runtime.create_run(&request).await.unwrap();
    assert_eq!(replay.id, run.id);
    assert_eq!(serde_json::to_value(&replay.spec).unwrap(), accepted);
}

#[tokio::test]
async fn agent_filter_cannot_grant_tools_and_readonly_removes_mutations() {
    let (_dir, runtime, mut request) = fixture().await;
    request.agent_config.as_mut().unwrap().tool_filter = Some(vec!["execute_program".into()]);
    assert!(matches!(
        runtime.preflight(&request).await,
        Err(Error::Validation(_))
    ));
    request.agent_config.as_mut().unwrap().tool_filter = Some(vec!["read_artifact".into()]);
    let flight = runtime.preflight(&request).await.unwrap();
    assert_eq!(flight.allowed_tools, vec!["read_artifact"]);
    request.agent_config.as_mut().unwrap().tool_filter = None;
    request.access_mode = AccessMode::ReadOnly;
    let flight = runtime.preflight(&request).await.unwrap();
    assert!(!flight.allowed_tools.contains(&"create_artifact".into()));
    assert!(flight.allowed_tools.contains(&"request_input".into()));
}

#[tokio::test]
async fn provider_edit_replay_and_retirement_survive_reopen() {
    let (dir, runtime, _request) = fixture().await;
    let edit = UpdateProviderSettings {
        idempotency_key: "edit-model".into(),
        expected_revision: 1,
        profile: CreateProvider {
            name: "Renamed".into(),
            kind: ProviderKind::Test,
            endpoint: "".into(),
            model: "fixture".into(),
            context_tokens: 32768,
            capabilities: Default::default(),
        },
        generation: GenerationSettings {
            temperature: Some(0.0),
            timeout_ms: Some(5000),
        },
        api_key: None,
        remove_key: false,
    };
    let changed = runtime
        .update_provider_settings("fixture", &edit)
        .await
        .unwrap();
    assert_eq!(changed.profile.revision, 2);
    assert_eq!(changed.profile.generation.temperature, Some(0.0));
    assert_eq!(
        runtime
            .update_provider_settings("fixture", &edit)
            .await
            .unwrap()
            .profile
            .revision,
        2
    );
    let mut conflict = edit.clone();
    conflict.idempotency_key = "conflicting-edit".into();
    assert!(matches!(
        runtime.update_provider_settings("fixture", &conflict).await,
        Err(Error::Conflict)
    ));
    let delete = DeleteProviderSettings {
        idempotency_key: "delete".into(),
        expected_revision: 2,
    };
    runtime
        .delete_provider_settings("fixture", &delete)
        .await
        .unwrap();
    runtime
        .delete_provider_settings("fixture", &delete)
        .await
        .unwrap();
    assert!(runtime.profiles().await.is_empty());
    runtime.store.close().await;
    let reopened = Store::sqlite(&dir.path().join("settings.sqlite"))
        .await
        .unwrap();
    assert!(reopened.providers().await.unwrap().is_empty());
    assert!(reopened.provider_retired("fixture").await.unwrap());
}

#[tokio::test]
async fn archived_chat_cannot_receive_a_new_direct_run() {
    let (_dir, runtime, mut request) = fixture().await;
    let archived = runtime
        .store
        .archive_chat(
            &request.order_id,
            &ArchiveChat {
                expected_revision: request.base_revision,
                idempotency_key: "archive-before-run".into(),
                archived: true,
            },
        )
        .await
        .unwrap();
    request.base_revision = archived.order.revision;
    assert!(matches!(
        runtime.create_run(&request).await,
        Err(Error::Conflict)
    ));
    assert!(
        runtime
            .store
            .runs(&request.order_id)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn finish(runtime: &Runtime, request: &CreateRun) -> RunRecord {
    let run = runtime.create_run(request).await.unwrap();
    let stop = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(runtime.clone().serve(1, stop.clone()));
    let completed = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let current = runtime.store.stored_run(&run.id).await.unwrap().record;
            if cowork_domain::terminal(&current.state) {
                break current;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    stop.cancel();
    task.await.unwrap();
    completed.unwrap()
}

#[tokio::test]
async fn skill_modes_actually_plan_execute_and_learn_without_rewriting_history() {
    for (mode, expected_attempts) in [
        (AgentExecutionMode::Plan, 1),
        (AgentExecutionMode::Hybrid, 3),
        (AgentExecutionMode::Execute, 2),
    ] {
        let (_dir, runtime, mut request) = fixture().await;
        let order = runtime.store.order(&request.order_id).await.unwrap();
        let skill = runtime
            .store
            .productivity_write_skill(
                &order.project_id,
                WriteSkill {
                    run_mode: mode,
                    auto_learn: true,
                    id: None,
                    expected_revision: 0,
                    name: "Mode proof".into(),
                    description: String::new(),
                    instructions: "Follow the selected mode".into(),
                    trigger_pattern: None,
                    prompt_template: None,
                    enabled: true,
                },
            )
            .await
            .unwrap();
        request.access_mode = AccessMode::Autonomous;
        let run = finish(&runtime, &request).await;
        assert_eq!(
            run.state,
            RunState::Completed,
            "{mode:?}: {:?}",
            run.error_code
        );
        assert_eq!(
            run.spec.agent_config.as_ref().unwrap().execution_mode,
            Some(mode)
        );
        assert_eq!(run.model_attempts.len(), expected_attempts, "{mode:?}");
        let learning = runtime
            .store
            .skill_learning_history(&order.project_id, &skill.id)
            .await
            .unwrap();
        assert_eq!(learning.items.len(), 1);
        assert_eq!(learning.items[0].run_id, run.id);
        assert_eq!(
            runtime
                .store
                .productivity_skills(&order.project_id)
                .await
                .unwrap()[0]
                .revision,
            1
        );
        if mode == AgentExecutionMode::Hybrid {
            let stored = runtime.store.stored_run(&run.id).await.unwrap();
            assert!(
                stored
                    .checkpoint
                    .messages
                    .iter()
                    .any(|m| m["cowork_phase"] == cowork_domain::HYBRID_EXECUTION_MARKER)
            );
        }
    }
}

#[tokio::test]
async fn regeneration_excludes_replaced_answer_and_replays_the_original_fork() {
    let (_dir, runtime, mut request) = fixture().await;
    request.access_mode = AccessMode::ReadOnly;
    let run = finish(&runtime, &request).await;
    assert_eq!(run.state, RunState::Completed);
    let messages = runtime.store.messages(&request.order_id).await.unwrap();
    let assistant = messages.iter().find(|m| m.role == "assistant").unwrap();
    let fork = BranchOrder {
        title: "New answer".into(),
        through_message_id: assistant.id.clone(),
        idempotency_key: "regeneration".into(),
    };
    let chat = runtime
        .store
        .regenerate_order(&request.order_id, fork.clone())
        .await
        .unwrap();
    assert_eq!(chat.draft, request.prompt);
    assert!(
        runtime
            .store
            .history(&chat.id)
            .await
            .unwrap()
            .inherited
            .unwrap()
            .messages
            .is_empty()
    );
    assert_eq!(
        runtime
            .store
            .regenerate_order(&request.order_id, fork)
            .await
            .unwrap()
            .id,
        chat.id
    );
    request.order_id = chat.id.clone();
    request.base_revision = 1;
    let regenerated = finish(&runtime, &request).await;
    assert_eq!(regenerated.state, RunState::Completed);
    assert_ne!(regenerated.id, run.id);
    assert_eq!(runtime.store.messages(&chat.id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn folder_instruction_levels_and_file_sources_are_frozen_only_from_selected_grants() {
    let (dir, runtime, mut request) = fixture().await;
    let order = runtime.store.order(&request.order_id).await.unwrap();
    let root = dir.path().join("source");
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(root.join("AGENTS.md"), "ROOT_FOLDER_RULE").unwrap();
    std::fs::write(root.join("nested/AGENTS.md"), "NESTED_FOLDER_RULE").unwrap();
    std::fs::write(root.join("nested/source.txt"), "SOURCE_CONTENT_FROZEN").unwrap();
    let broker = cowork_broker::FileBroker::open(
        &dir.path().join("broker"),
        vec![cowork_broker::FolderConfig {
            id: "source".into(),
            project_id: order.project_id.clone(),
            name: "Source folder".into(),
            path: root.clone(),
            writable: true,
        }],
    )
    .unwrap();
    request.folders = broker.grants(&order.project_id).unwrap();
    request.access_mode = AccessMode::ReadOnly;
    let runtime = runtime.with_file_broker(broker);
    let p = runtime
        .store
        .project_settings(&order.project_id)
        .await
        .unwrap();
    runtime
        .store
        .write_project_settings(
            &order.project_id,
            &WriteProjectSettings {
                expected_revision: p.revision,
                idempotency_key: "sources".into(),
                name: p.name,
                instructions: String::new(),
                sources: vec![ProjectSource {
                    id: "file".into(),
                    label: "Source".into(),
                    kind: "file".into(),
                    reference: "source::nested/source.txt".into(),
                    enabled: true,
                    access: "read_only".into(),
                }],
                working_folder: None,
            },
        )
        .await
        .unwrap();
    let run = runtime.create_run(&request).await.unwrap();
    let instructions = &run.spec.agent_config.as_ref().unwrap().instructions;
    for expected in [
        "ROOT_FOLDER_RULE",
        "NESTED_FOLDER_RULE",
        "SOURCE_CONTENT_FROZEN",
    ] {
        assert!(instructions.contains(expected), "{instructions}");
    }
    std::fs::write(root.join("AGENTS.md"), "CHANGED_RULE").unwrap();
    assert!(
        !runtime
            .store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .spec
            .agent_config
            .unwrap()
            .instructions
            .contains("CHANGED_RULE")
    );
}
