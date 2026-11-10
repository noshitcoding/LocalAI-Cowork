use cowork_contracts::*;
use cowork_domain::Error;
use cowork_storage::{
    Store,
    library_config::{LibraryOwner, validate_agent_configuration, validate_effective_agent_config},
};
use serde_json::json;

fn personality(key: &str, name: &str) -> WritePersonality {
    WritePersonality {
        idempotency_key: key.into(),
        expected_revision: 0,
        value: PersonalityDefinition {
            name: name.into(),
            system_prompt: "Be precise. Cite your evidence.".into(),
            ..Default::default()
        },
    }
}
async fn actors(store: &Store) -> (cowork_identity::Principal, cowork_identity::Principal) {
    let identity = store.identity();
    let bootstrap = "b".repeat(64);
    identity
        .initialize_credentials(&bootstrap, &"s".repeat(64))
        .await
        .unwrap();
    let (_, first) = identity
        .bootstrap(
            &bootstrap,
            "library-owner",
            "Library owner",
            "SyntheticPassword-2026!",
            "Library fixture",
        )
        .await
        .unwrap();
    identity
        .register_local_identity(
            "library-other",
            "Other actor",
            "OtherSyntheticPassword-2026!",
        )
        .await
        .unwrap();
    let other = identity
        .login("library-other", "OtherSyntheticPassword-2026!", 3_600_000)
        .await
        .unwrap();
    (
        identity.authenticate_session(&first.token).await.unwrap(),
        identity.authenticate_session(&other.token).await.unwrap(),
    )
}

#[tokio::test]
async fn durable_copies_survive_source_edits_deletion_and_database_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("library.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let source = store
        .library_write_personality(
            LibraryOwner::Local,
            None,
            &personality("source-create", "Researcher"),
        )
        .await
        .unwrap();
    let write = WriteAgentTemplate {
        idempotency_key: "template-create".into(),
        expected_revision: 0,
        value: AgentTemplateDefinition {
            name: "Research agent".into(),
            personality: Some(source.clone()),
            configuration: AgentConfiguration {
                instructions: source.value.system_prompt.clone(),
                personality_source: Some(LibrarySource {
                    id: source.id.clone(),
                    revision: 1,
                }),
                generation: GenerationSettings {
                    temperature: Some(0.2),
                    timeout_ms: Some(90_000),
                },
                ..Default::default()
            },
            ..Default::default()
        },
    };
    let saved = store
        .library_write_agent_template(LibraryOwner::Local, None, &write)
        .await
        .unwrap();
    let mut edit = personality("source-edit", "Researcher v2");
    edit.expected_revision = 1;
    edit.value.system_prompt = "A changed source.".into();
    store
        .library_write_personality(LibraryOwner::Local, Some(&source.id), &edit)
        .await
        .unwrap();
    let removed = store
        .library_delete_personality(
            LibraryOwner::Local,
            &source.id,
            &DeleteLibraryEntry {
                idempotency_key: "source-delete".into(),
                expected_revision: 2,
            },
        )
        .await
        .unwrap();
    assert_eq!(removed.revision, 3);
    assert!(
        store
            .library_personalities(LibraryOwner::Local)
            .await
            .unwrap()
            .is_empty()
    );
    // A template retains and can reuse an authentic old source copy after deletion.
    let mut copied = write.clone();
    copied.idempotency_key = "template-duplicate".into();
    copied.value.name = "Research copy".into();
    store
        .library_write_agent_template(LibraryOwner::Local, None, &copied)
        .await
        .unwrap();
    let prefs = ApplicationPreferences {
        default_agent_id: Some(saved.id.clone()),
        system_prompt: "Installation default".into(),
        ..Default::default()
    };
    store
        .library_write_preferences(
            LibraryOwner::Local,
            &WritePreferences {
                idempotency_key: "prefs-create".into(),
                expected_revision: 0,
                value: prefs,
            },
        )
        .await
        .unwrap();
    store.close().await;
    let reopened = Store::sqlite(&path).await.unwrap();
    let actual = reopened
        .library_agent_template(LibraryOwner::Local, &saved.id)
        .await
        .unwrap();
    assert_eq!(
        actual.value.personality.unwrap().value.system_prompt,
        "Be precise. Cite your evidence."
    );
    assert_eq!(actual.value.configuration.generation.temperature, Some(0.2));
    assert_eq!(
        reopened
            .library_preferences(LibraryOwner::Local)
            .await
            .unwrap()
            .value
            .default_agent_id,
        Some(saved.id)
    );
    reopened.close().await;
}

#[tokio::test]
async fn original_acknowledgements_cas_and_unique_defaults_are_atomic() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let mut first = personality("one", "First");
    first.value.is_default = true;
    let a = store
        .library_write_personality(LibraryOwner::Local, None, &first)
        .await
        .unwrap();
    let mut second = personality("two", "Second");
    second.value.is_default = true;
    let b = store
        .library_write_personality(LibraryOwner::Local, None, &second)
        .await
        .unwrap();
    let entries = store
        .library_personalities(LibraryOwner::Local)
        .await
        .unwrap();
    assert_eq!(entries.iter().filter(|p| p.value.is_default).count(), 1);
    let previous = entries.iter().find(|p| p.id == a.id).unwrap();
    assert!(!previous.value.is_default);
    assert_eq!(previous.revision, 2);
    let repeated = store
        .library_write_personality(LibraryOwner::Local, None, &first)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&repeated).unwrap(),
        serde_json::to_value(&a).unwrap()
    );
    assert_eq!(
        store
            .library_personalities(LibraryOwner::Local)
            .await
            .unwrap()
            .len(),
        2
    );
    first.value.name = "Different original body".into();
    assert!(matches!(
        store
            .library_write_personality(LibraryOwner::Local, None, &first)
            .await,
        Err(Error::Conflict)
    ));
    let mut stale = personality("stale-update", "Stale");
    stale.expected_revision = 1;
    assert!(matches!(
        store
            .library_write_personality(LibraryOwner::Local, Some(&a.id), &stale)
            .await,
        Err(Error::Conflict)
    ));
    let delete = DeleteLibraryEntry {
        idempotency_key: "delete-b".into(),
        expected_revision: 1,
    };
    let removed = store
        .library_delete_personality(LibraryOwner::Local, &b.id, &delete)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(
            store
                .library_delete_personality(LibraryOwner::Local, &b.id, &delete)
                .await
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(removed).unwrap()
    );
    assert!(matches!(
        store.library_personality(LibraryOwner::Local, &b.id).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn private_actors_never_adopt_owner_or_other_actor_content_and_revoked_replay_is_denied() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (actor, other) = actors(&store).await;
    let local = store
        .library_write_personality(
            LibraryOwner::Local,
            None,
            &personality("same-key", "Owner secret prompt"),
        )
        .await
        .unwrap();
    let request = personality("same-key", "Actor profile");
    let private = store
        .library_write_personality(LibraryOwner::Actor(&actor), None, &request)
        .await
        .unwrap();
    assert!(matches!(
        store
            .library_write_preferences(
                LibraryOwner::Actor(&actor),
                &WritePreferences {
                    idempotency_key: "foreign-personality-default".into(),
                    expected_revision: 0,
                    value: ApplicationPreferences {
                        default_personality_id: Some(local.id.clone()),
                        ..Default::default()
                    },
                }
            )
            .await,
        Err(Error::NotFound)
    ));
    assert_eq!(
        store
            .library_preferences(LibraryOwner::Actor(&actor))
            .await
            .unwrap()
            .revision,
        0
    );
    assert!(
        !store
            .library_personality(LibraryOwner::Local, &local.id)
            .await
            .unwrap()
            .value
            .is_default
    );
    assert!(matches!(
        store
            .library_personality(LibraryOwner::Actor(&actor), &local.id)
            .await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store
            .library_personality(LibraryOwner::Actor(&other), &private.id)
            .await,
        Err(Error::NotFound)
    ));
    assert!(
        store
            .library_personalities(LibraryOwner::Actor(&other))
            .await
            .unwrap()
            .is_empty()
    );
    let counterfeit = WriteAgentTemplate {
        idempotency_key: "foreign-source".into(),
        expected_revision: 0,
        value: AgentTemplateDefinition {
            name: "Actor template".into(),
            personality: Some(local),
            ..Default::default()
        },
    };
    assert!(matches!(
        store
            .library_write_agent_template(LibraryOwner::Actor(&actor), None, &counterfeit)
            .await,
        Err(Error::Conflict)
    ));
    let prefs = WritePreferences {
        idempotency_key: "prefs".into(),
        expected_revision: 0,
        value: ApplicationPreferences {
            system_prompt: "Private default".into(),
            ..Default::default()
        },
    };
    store
        .library_write_preferences(LibraryOwner::Actor(&actor), &prefs)
        .await
        .unwrap();
    assert!(
        store
            .library_preferences(LibraryOwner::Actor(&other))
            .await
            .unwrap()
            .value
            .system_prompt
            .is_empty()
    );
    store.identity().logout(&actor).await.unwrap();
    assert!(matches!(
        store
            .library_write_personality(LibraryOwner::Actor(&actor), None, &request)
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store.library_preferences(LibraryOwner::Actor(&actor)).await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn preferences_replay_returns_original_and_rejects_stale_or_changed_requests() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let defaults = store
        .library_preferences(LibraryOwner::Local)
        .await
        .unwrap();
    assert_eq!(defaults.revision, 0);
    let first = WritePreferences {
        idempotency_key: "preferences-first".into(),
        expected_revision: 0,
        value: ApplicationPreferences {
            font_scale: 125,
            ..defaults.value
        },
    };
    let saved = store
        .library_write_preferences(LibraryOwner::Local, &first)
        .await
        .unwrap();
    let second = WritePreferences {
        idempotency_key: "preferences-second".into(),
        expected_revision: 1,
        value: ApplicationPreferences {
            font_scale: 150,
            ..saved.value.clone()
        },
    };
    store
        .library_write_preferences(LibraryOwner::Local, &second)
        .await
        .unwrap();
    let original = store
        .library_write_preferences(LibraryOwner::Local, &first)
        .await
        .unwrap();
    assert_eq!(original.revision, 1);
    assert_eq!(original.value.font_scale, 125);
    let mut changed = first.clone();
    changed.value.font_scale = 175;
    assert!(matches!(
        store
            .library_write_preferences(LibraryOwner::Local, &changed)
            .await,
        Err(Error::Conflict)
    ));
    changed.idempotency_key = "preferences-stale".into();
    assert!(matches!(
        store
            .library_write_preferences(LibraryOwner::Local, &changed)
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        store
            .library_preferences(LibraryOwner::Local)
            .await
            .unwrap()
            .value
            .font_scale,
        150
    );
}

#[test]
fn invalid_generation_duplicate_tools_and_unknown_wire_fields_are_rejected() {
    let invalid = EffectiveAgentConfig {
        generation: GenerationSettings {
            temperature: Some(f64::NAN),
            timeout_ms: None,
        },
        ..Default::default()
    };
    assert!(validate_effective_agent_config(&invalid).is_err());
    let duplicate = AgentConfiguration {
        native_tools: vec!["read_text_file".into(), "read_text_file".into()],
        ..Default::default()
    };
    assert!(validate_agent_configuration(&duplicate).is_err());
    assert!(serde_json::from_value::<WritePersonality>(json!({"idempotency_key":"bad","expected_revision":0,"value":{"name":"a","api_key":"secret"}})).is_err());
}

#[tokio::test]
async fn deleting_selected_defaults_clears_only_matching_preferences_atomically() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let source = store
        .library_write_personality(
            LibraryOwner::Local,
            None,
            &personality("default-source", "Default source"),
        )
        .await
        .unwrap();
    let template = store
        .library_write_agent_template(
            LibraryOwner::Local,
            None,
            &WriteAgentTemplate {
                idempotency_key: "default-template".into(),
                expected_revision: 0,
                value: AgentTemplateDefinition {
                    name: "Default agent".into(),
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
    store
        .library_write_preferences(
            LibraryOwner::Local,
            &WritePreferences {
                idempotency_key: "set-defaults".into(),
                expected_revision: 0,
                value: ApplicationPreferences {
                    default_agent_id: Some(template.id.clone()),
                    default_personality_id: Some(source.id.clone()),
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
    let deletion = DeleteLibraryEntry {
        idempotency_key: "delete-default-agent".into(),
        expected_revision: 1,
    };
    store
        .library_delete_agent_template(LibraryOwner::Local, &template.id, &deletion)
        .await
        .unwrap();
    let preferences = store
        .library_preferences(LibraryOwner::Local)
        .await
        .unwrap();
    assert_eq!(preferences.revision, 2);
    assert!(preferences.value.default_agent_id.is_none());
    assert_eq!(
        preferences.value.default_personality_id,
        Some(source.id.clone())
    );
    store
        .library_delete_agent_template(LibraryOwner::Local, &template.id, &deletion)
        .await
        .unwrap();
    assert_eq!(
        store
            .library_preferences(LibraryOwner::Local)
            .await
            .unwrap()
            .revision,
        2
    );
    store
        .library_delete_personality(
            LibraryOwner::Local,
            &source.id,
            &DeleteLibraryEntry {
                idempotency_key: "delete-default-personality".into(),
                expected_revision: 2,
            },
        )
        .await
        .unwrap();
    let preferences = store
        .library_preferences(LibraryOwner::Local)
        .await
        .unwrap();
    assert_eq!(preferences.revision, 3);
    assert!(preferences.value.default_personality_id.is_none());
}

#[tokio::test]
async fn personality_markers_and_preference_defaults_converge_without_replaying_effects() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let mut create = personality("converge-first", "First default");
    create.value.is_default = true;
    let first = store
        .library_write_personality(LibraryOwner::Local, None, &create)
        .await
        .unwrap();
    let second = store
        .library_write_personality(
            LibraryOwner::Local,
            None,
            &personality("converge-second", "Second"),
        )
        .await
        .unwrap();
    let preferences = store
        .library_preferences(LibraryOwner::Local)
        .await
        .unwrap();
    assert_eq!(preferences.revision, 1);
    assert_eq!(
        preferences.value.default_personality_id.as_deref(),
        Some(first.id.as_str())
    );
    let agent = store
        .library_write_agent_template(
            LibraryOwner::Local,
            None,
            &WriteAgentTemplate {
                idempotency_key: "converge-agent".into(),
                expected_revision: 0,
                value: AgentTemplateDefinition {
                    name: "Frozen first default".into(),
                    personality: Some(first.clone()),
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
    let choose_second = WritePreferences {
        idempotency_key: "converge-preferences".into(),
        expected_revision: 1,
        value: ApplicationPreferences {
            default_personality_id: Some(second.id.clone()),
            default_agent_id: Some(agent.id.clone()),
            ..preferences.value.clone()
        },
    };
    let selected = store
        .library_write_preferences(LibraryOwner::Local, &choose_second)
        .await
        .unwrap();
    assert_eq!(selected.revision, 2);
    let first_current = store
        .library_personality(LibraryOwner::Local, &first.id)
        .await
        .unwrap();
    let second_current = store
        .library_personality(LibraryOwner::Local, &second.id)
        .await
        .unwrap();
    assert_eq!(first_current.revision, 2);
    assert!(!first_current.value.is_default);
    assert_eq!(second_current.revision, 2);
    assert!(second_current.value.is_default);
    assert_eq!(
        store
            .library_agent_template(LibraryOwner::Local, &agent.id)
            .await
            .unwrap()
            .value
            .personality
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        store
            .library_write_personality(LibraryOwner::Local, None, &create)
            .await
            .unwrap()
            .revision,
        1
    );
    let stale = WritePreferences {
        idempotency_key: "converge-stale-preferences".into(),
        expected_revision: 1,
        value: preferences.value,
    };
    assert!(matches!(
        store
            .library_write_preferences(LibraryOwner::Local, &stale)
            .await,
        Err(Error::Conflict)
    ));
    let stale_edit = WritePersonality {
        idempotency_key: "converge-stale-personality".into(),
        expected_revision: 1,
        value: first.value.clone(),
    };
    assert!(matches!(
        store
            .library_write_personality(LibraryOwner::Local, Some(&first.id), &stale_edit)
            .await,
        Err(Error::Conflict)
    ));
    store
        .library_write_personality(
            LibraryOwner::Local,
            Some(&first.id),
            &WritePersonality {
                idempotency_key: "converge-unrelated-edit".into(),
                expected_revision: 2,
                value: first_current.value,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .library_preferences(LibraryOwner::Local)
            .await
            .unwrap()
            .revision,
        2
    );
    let mut clear = WritePersonality {
        idempotency_key: "converge-clear".into(),
        expected_revision: 2,
        value: second_current.value,
    };
    clear.value.is_default = false;
    let cleared = store
        .library_write_personality(LibraryOwner::Local, Some(&second.id), &clear)
        .await
        .unwrap();
    assert_eq!(cleared.revision, 3);
    store
        .library_write_personality(LibraryOwner::Local, Some(&second.id), &clear)
        .await
        .unwrap();
    assert_eq!(
        store
            .library_write_preferences(LibraryOwner::Local, &choose_second)
            .await
            .unwrap()
            .revision,
        2
    );
    let current = store
        .library_preferences(LibraryOwner::Local)
        .await
        .unwrap();
    assert_eq!(current.revision, 3);
    assert!(current.value.default_personality_id.is_none());
    assert_eq!(current.value.default_agent_id, Some(agent.id));
    assert!(
        !store
            .library_personality(LibraryOwner::Local, &second.id)
            .await
            .unwrap()
            .value
            .is_default
    );
    let missing = WritePreferences {
        idempotency_key: "converge-missing".into(),
        expected_revision: 3,
        value: ApplicationPreferences {
            default_personality_id: Some(uuid::Uuid::new_v4().to_string()),
            ..current.value
        },
    };
    assert!(matches!(
        store
            .library_write_preferences(LibraryOwner::Local, &missing)
            .await,
        Err(Error::NotFound)
    ));
    assert_eq!(
        store
            .library_preferences(LibraryOwner::Local)
            .await
            .unwrap()
            .revision,
        3
    );
}

#[tokio::test]
async fn codex_account_actions_are_private_and_uncertain_login_intents_never_replay() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (actor, other) = actors(&store).await;
    let profile = store
        .write_codex_account(
            LibraryOwner::Actor(&actor),
            None,
            &WriteCodexAccount {
                idempotency_key: "account-create".into(),
                expected_revision: 0,
                value: CodexAccountDefinition {
                    name: "Private Codex".into(),
                    priority: 1,
                },
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .codex_accounts(LibraryOwner::Local)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        store
            .codex_account(LibraryOwner::Actor(&other), &profile.id)
            .await,
        Err(Error::NotFound)
    ));
    assert!(
        store
            .prepare_codex_account_call(
                LibraryOwner::Actor(&actor),
                &profile.id,
                "login-original",
                1,
                "login_device"
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        store
            .prepare_codex_account_call(
                LibraryOwner::Actor(&actor),
                &profile.id,
                "login-original",
                1,
                "login_device"
            )
            .await,
        Err(Error::UncertainEffect)
    ));
    assert!(matches!(
        store
            .prepare_codex_account_call(
                LibraryOwner::Actor(&other),
                &profile.id,
                "login-original",
                1,
                "login_device"
            )
            .await,
        Err(Error::NotFound)
    ));
    let response = json!({"login_id":"opaque-login","flow":"device","authorization_url":null,"verification_url":"https://auth.openai.com/codex/device","user_code":"1234"});
    store
        .complete_codex_account_call(
            LibraryOwner::Actor(&actor),
            &profile.id,
            "login-original",
            &response,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .prepare_codex_account_call(
                LibraryOwner::Actor(&actor),
                &profile.id,
                "login-original",
                1,
                "login_device"
            )
            .await
            .unwrap(),
        Some(response)
    );
    assert!(matches!(
        store
            .prepare_codex_account_call(
                LibraryOwner::Actor(&actor),
                &profile.id,
                "login-original",
                1,
                "login_browser"
            )
            .await,
        Err(Error::Conflict)
    ));
    store.identity().logout(&actor).await.unwrap();
    assert!(matches!(
        store
            .prepare_codex_account_call(
                LibraryOwner::Actor(&actor),
                &profile.id,
                "login-original",
                1,
                "login_device"
            )
            .await,
        Err(Error::Forbidden)
    ));
}
