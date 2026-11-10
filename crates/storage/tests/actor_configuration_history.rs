//! Durable actor configuration copies and historical credentials; no remote services.
use cowork_contracts::*;
use cowork_domain::{Error, canonical_hash};
use cowork_identity::Principal;
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{Store, android_personal_provider};
use serde_json::{Value, json};
use uuid::Uuid;
use zeroize::Zeroizing;
struct Keys;
impl KeyStore for Keys {
    fn load(&self, _: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        Ok(Zeroizing::new([37; 32]))
    }
}
fn uuid(v: &Value, key: &str) -> Uuid {
    Uuid::parse_str(v[key].as_str().unwrap()).unwrap()
}
async fn setup(path: &std::path::Path) -> (Store, Principal, String, Uuid, Uuid) {
    let s = Store::sqlite(path).await.unwrap();
    let i = s.identity();
    let b = "b".repeat(64);
    i.initialize_credentials(&b, &"s".repeat(64)).await.unwrap();
    let (_, issued) = i
        .bootstrap(
            &b,
            "configuration-owner",
            "Owner",
            "SyntheticPassword-2026!",
            "Fixture",
        )
        .await
        .unwrap();
    let a = i.authenticate_session(&issued.token).await.unwrap();
    let p=s.android_personal_create_project(&a,json!({"privacy":"private_local","team_id":null,"name":"Private","description":"","policy":{}})).await.unwrap();
    let project = uuid(&p, "id");
    let t=s.android_personal_create_thread(&a,json!({"project_id":project,"title":"Copied chat","forked_from_thread_id":null,"forked_from_message_id":null}),None).await.unwrap();
    (s, a, issued.token, project, uuid(&t, "id"))
}
fn key() -> KeyId {
    KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    }
}
fn create() -> Value {
    json!({"team_id":null,"name":"Original","provider_kind":"openai_compatible","api_key":"synthetic-original-key","model_defaults":{"base_url":"https://original.example.test/v1","model":"original-model","auth_mode":"bearer","context_tokens":65536,"temperature":0.2}})
}
#[tokio::test]
async fn codex_model_copies_are_private_immutable_and_explicit_deletion_revokes() {
    use cowork_storage::library_config::LibraryOwner;
    let dir = tempfile::tempdir().unwrap();
    let (s, a, _, _, _) = setup(&dir.path().join("codex-profiles.sqlite")).await;
    let account = s
        .write_codex_account(
            LibraryOwner::Actor(&a),
            None,
            &WriteCodexAccount {
                expected_revision: 0,
                idempotency_key: "actor-account".into(),
                value: CodexAccountDefinition {
                    name: "Own Codex".into(),
                    priority: 0,
                },
            },
        )
        .await
        .unwrap();
    let request = MaterializeCodexModelProfile {
        expected_account_revision: 1,
        idempotency_key: "freeze-codex".into(),
        model: "fixture-model".into(),
        reasoning_effort: Some("low".into()),
        context_tokens: 32768,
    };
    let profile = s
        .materialize_codex_model_profile(LibraryOwner::Actor(&a), &account.id, &request)
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(&profile),
        canonical_hash(
            &s.materialize_codex_model_profile(LibraryOwner::Actor(&a), &account.id, &request)
                .await
                .unwrap()
        )
    );
    assert!(matches!(&profile.kind, ProviderKind::Codex));
    assert!(profile.capabilities.image_input);
    let raw = s
        .android_personal_provider(&a, Uuid::parse_str(&profile.id).unwrap())
        .await
        .unwrap();
    let projected = cowork_storage::actor_model_registry::provider_profile(&raw).unwrap();
    assert!(matches!(&projected.kind, ProviderKind::Codex));
    assert!(projected.capabilities.image_input);
    assert_eq!(projected.capabilities, profile.capabilities);
    assert_eq!(canonical_hash(&profile), canonical_hash(&projected));
    let namespace = format!("actor:{}", a.identity_id());
    let mut missing_image_capability = projected.clone();
    missing_image_capability.capabilities.image_input = false;
    assert!(
        s.codex_model_profile_account(&namespace, &missing_image_capability)
            .await
            .is_err()
    );
    assert!(
        s.codex_model_profile_account(&namespace, &profile)
            .await
            .is_ok()
    );
    assert!(
        s.codex_model_profile_account("installation-owner", &profile)
            .await
            .is_err()
    );
    let mut forged = profile.clone();
    forged.model = "other-model".into();
    assert!(
        s.codex_model_profile_account(&namespace, &forged)
            .await
            .is_err()
    );
    s.write_codex_account(
        LibraryOwner::Actor(&a),
        Some(&account.id),
        &WriteCodexAccount {
            expected_revision: 1,
            idempotency_key: "rename-account".into(),
            value: CodexAccountDefinition {
                name: "Later label".into(),
                priority: 1,
            },
        },
    )
    .await
    .unwrap();
    assert!(
        s.codex_model_profile_account(&namespace, &profile)
            .await
            .is_ok()
    );
    s.android_personal_provider_mutate(
        &a,
        "delete",
        Some(Uuid::parse_str(&profile.id).unwrap()),
        json!({"expected_revision":1}),
        Some("delete-codex-model-copy"),
        &Keys,
        key(),
    )
    .await
    .unwrap();
    assert!(
        s.codex_model_profile_account(&namespace, &profile)
            .await
            .is_err()
    );
    assert_eq!(
        canonical_hash(&profile),
        canonical_hash(
            &s.original_codex_model_profile(LibraryOwner::Actor(&a), &account.id, &request)
                .await
                .unwrap()
                .unwrap()
        )
    );
}
#[tokio::test]
async fn actor_context_has_user_scope_real_skill_modes_and_exact_retry_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let (s, a, _, project, _) = setup(&dir.path().join("context-api.sqlite")).await;
    let other=s.android_personal_create_project(&a,json!({"privacy":"private_local","team_id":null,"name":"Other project","description":"","policy":{}})).await.unwrap();
    let other = uuid(&other, "id");
    let memory = Uuid::new_v4();
    let original = json!({"request_id":Uuid::new_v4(),"expected_revision":0,"value":{"project_id":null,"key":"language","content":"Original global memory","category":"profile","confidence":0.8,"enabled":true}});
    let first = s
        .actor_context_mutate(&a, "memories", memory, &original, false)
        .await
        .unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(first.value["scope"], "user");
    let update = json!({"request_id":Uuid::new_v4(),"expected_revision":1,"value":{"project_id":null,"key":"language","content":"Changed global memory","category":"profile","confidence":0.8,"enabled":true}});
    s.actor_context_mutate(&a, "memories", memory, &update, false)
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(
            &s.actor_context_mutate(&a, "memories", memory, &original, false)
                .await
                .unwrap()
        ),
        canonical_hash(&first)
    );
    for project in [project, other] {
        let listed = s
            .actor_context(&a, "memories", Some(project))
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].revision, 2);
        assert_eq!(listed[0].value["content"], "Changed global memory");
    }
    let skill = Uuid::new_v4();
    s.actor_context_mutate(&a,"skills",skill,&json!({"request_id":Uuid::new_v4(),"expected_revision":0,"value":{"project_id":project,"name":"Research plan","description":"Scoped guidance","prompt_template":"Inspect: {{input}}","trigger_pattern":"*research*","run_mode":"plan","enabled":true}}),false).await.unwrap();
    assert!(
        s.actor_context(&a, "skills", Some(other))
            .await
            .unwrap()
            .is_empty()
    );
    let skills = s
        .android_wire_runtime_metadata(&a, project, "skill", &[skill])
        .await
        .unwrap();
    let context = json!({"skill":skills});
    assert!(
        cowork_storage::android_private_workflows::render_actor_skills(
            &context,
            "Research evidence"
        )
        .unwrap()
        .contains("Inspect: Research evidence")
    );
    assert!(
        cowork_storage::android_private_workflows::render_actor_skills(&context, "Unrelated")
            .unwrap()
            .is_empty()
    );
    let mut input = json!({"prompt":"Research evidence","frozen_runtime_context":context});
    s.freeze_actor_run_defaults(&a, &mut input).await.unwrap();
    assert_eq!(
        input["run_configuration"]["agent_config"]["execution_mode"],
        "plan"
    );
    let mut unrelated = json!({"prompt":"Unrelated","frozen_runtime_context":context});
    s.freeze_actor_run_defaults(&a, &mut unrelated)
        .await
        .unwrap();
    assert!(
        unrelated["run_configuration"]["agent_config"]
            .get("execution_mode")
            .is_none()
    );
    let mut role_only = json!({"prompt":"Research evidence","private_workflow":{"context_selection":{"parent_skill_ids":[]}},"frozen_runtime_context":context});
    s.freeze_actor_run_defaults(&a, &mut role_only)
        .await
        .unwrap();
    assert!(
        role_only["run_configuration"]["agent_config"]
            .get("execution_mode")
            .is_none()
    );
    s.actor_context_mutate(
        &a,
        "memories",
        memory,
        &json!({"request_id":Uuid::new_v4(),"expected_revision":2}),
        true,
    )
    .await
    .unwrap();
    assert!(
        s.actor_context(&a, "memories", Some(other))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn actor_history_survives_profile_edit_and_restart_then_explicit_deletion_revokes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let (s, a, token, _, _) = setup(&path).await;
    let key = key();
    let original = s
        .android_personal_provider_mutate(&a, "create", None, create(), Some("create"), &Keys, key)
        .await
        .unwrap();
    let projected = cowork_storage::actor_model_registry::provider_profile(&original).unwrap();
    assert!(matches!(&projected.kind, ProviderKind::OpenaiCompatible));
    assert_eq!(projected.capabilities, ModelCapabilities::default());
    assert!(!projected.capabilities.image_input);
    let id = uuid(&original, "id");
    let binding = android_personal_provider::binding(&original).unwrap();
    let updated=s.android_personal_provider_mutate(&a,"update",Some(id),json!({"expected_revision":1,"name":"Changed","provider_kind":"openai_compatible","api_key":"synthetic-replacement-key","model_defaults":{"base_url":"https://changed.example.test/v1","model":"changed-model","auth_mode":"bearer","context_tokens":32768,"temperature":0.8}}),Some("update"),&Keys,key).await.unwrap();
    let (frozen, secret) = s
        .android_personal_provider_dispatch(&a, &binding, &Keys)
        .await
        .unwrap();
    assert_eq!(frozen, original);
    assert_eq!(secret.unwrap().as_str(), "synthetic-original-key");
    let (_, secret) = s
        .android_personal_provider_dispatch(
            &a,
            &android_personal_provider::binding(&updated).unwrap(),
            &Keys,
        )
        .await
        .unwrap();
    assert_eq!(secret.unwrap().as_str(), "synthetic-replacement-key");
    let mut forged = binding.clone();
    forged["model"] = "changed-model".into();
    assert!(
        s.android_personal_provider_dispatch(&a, &forged, &Keys)
            .await
            .is_err()
    );
    drop(s);
    let s = Store::sqlite(&path).await.unwrap();
    let a = s.identity().authenticate_session(&token).await.unwrap();
    assert_eq!(
        s.android_personal_provider_dispatch(&a, &binding, &Keys)
            .await
            .unwrap()
            .1
            .unwrap()
            .as_str(),
        "synthetic-original-key"
    );
    s.android_personal_provider_mutate(
        &a,
        "delete",
        Some(id),
        json!({"expected_revision":2}),
        Some("delete"),
        &Keys,
        key,
    )
    .await
    .unwrap();
    assert!(matches!(
        s.android_personal_provider_dispatch(&a, &binding, &Keys)
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        s.android_personal_provider_dispatch(
            &a,
            &android_personal_provider::binding(&updated).unwrap(),
            &Keys
        )
        .await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn actor_chat_copies_cas_and_lost_response_are_private_and_durable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let (s, a, token, _, thread) = setup(&path).await;
    let empty = s.actor_chat_configuration(&a, thread).await.unwrap();
    assert_eq!(empty.revision, 0);
    assert!(empty.value.is_none());
    let mut request = WriteActorChatConfiguration {
        expected_revision: 0,
        idempotency_key: "original-save".into(),
        value: ActorChatRunConfiguration {
            model_profile_id: Uuid::new_v4().to_string(),
            provider_revision: 7,
            run_configuration: ActorRunConfiguration {
                agent_config: EffectiveAgentConfig {
                    instructions: "Original copied instructions".into(),
                    ..Default::default()
                },
                access_mode: AccessMode::ReadOnly,
                budgets: Budgets::default(),
            },
            skill_ids: vec![],
            memory_ids: vec![],
            mcp_metadata_ids: vec![],
            expected_mcp_revisions: Default::default(),
            model_binding: None,
            output: None,
        },
    };
    let first = s
        .write_actor_chat_configuration(&a, thread, &request)
        .await
        .unwrap();
    let mut second = request.clone();
    second.expected_revision = 1;
    second.idempotency_key = "second-save".into();
    second.value.run_configuration.agent_config.instructions = "New copied instructions".into();
    s.write_actor_chat_configuration(&a, thread, &second)
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(&first),
        canonical_hash(
            &s.write_actor_chat_configuration(&a, thread, &request)
                .await
                .unwrap()
        )
    );
    request.idempotency_key = "stale-save".into();
    assert!(matches!(
        s.write_actor_chat_configuration(&a, thread, &request).await,
        Err(Error::Conflict)
    ));
    request.idempotency_key = "original-save".into();
    request.value.run_configuration.agent_config.instructions = "Changed retry".into();
    assert!(matches!(
        s.write_actor_chat_configuration(&a, thread, &request).await,
        Err(Error::Conflict)
    ));
    let i = s.identity();
    i.register_local_identity(
        "other-configuration-owner",
        "Other",
        "OtherSyntheticPassword-2026!",
    )
    .await
    .unwrap();
    let other = i
        .login(
            "other-configuration-owner",
            "OtherSyntheticPassword-2026!",
            3600000,
        )
        .await
        .unwrap();
    let other = i.authenticate_session(&other.token).await.unwrap();
    assert!(s.actor_chat_configuration(&other, thread).await.is_err());
    drop(s);
    let s = Store::sqlite(&path).await.unwrap();
    let a = s.identity().authenticate_session(&token).await.unwrap();
    let saved = s.actor_chat_configuration(&a, thread).await.unwrap();
    assert_eq!(saved.revision, 2);
    assert_eq!(
        saved
            .value
            .unwrap()
            .run_configuration
            .agent_config
            .instructions,
        "New copied instructions"
    );
}

#[tokio::test]
async fn actor_hybrid_plan_checkpoint_continues_once_and_preserves_original_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hybrid.sqlite");
    let (s, a, _, project, thread) = setup(&path).await;
    let key = key();
    let p = s
        .android_personal_provider_mutate(&a, "create", None, create(), Some("create"), &Keys, key)
        .await
        .unwrap();
    let request = json!({"project_id":project,"thread_id":thread,"project_revision":1,"project_privacy":"private_local","executor_target":{"kind":"server_linux","pool_id":null},"required_capabilities":[],"model_profile_id":p["id"],"snapshot_id":null,"task":null,"input":{"prompt":"Plan then execute","provider_binding":android_personal_provider::binding(&p).unwrap()},"idempotency_key":"hybrid-run"});
    let run = s.android_private_server_create(&a, &request).await.unwrap();
    let run = uuid(&run["spec"], "id");
    s.android_personal_provider_mutate(&a,"update",Some(uuid(&p,"id")),json!({"expected_revision":1,"name":"Changed after acceptance","provider_kind":"openai_compatible","api_key":"replacement-after-acceptance","model_defaults":{"base_url":"https://later.example.test/v1","model":"later-model","auth_mode":"bearer","context_tokens":32768}}),Some("edit-after-admission"),&Keys,key).await.unwrap();
    let lease = s.android_private_server_claim().await.unwrap().unwrap();
    let spec:RunSpec=serde_json::from_value(json!({"order_id":thread,"project_id":project,"prompt":"Plan then execute","provider":cowork_storage::actor_model_registry::provider_profile(&p).unwrap(),"agent_config":{"instructions":"","generation":{},"execution_mode":"hybrid"},"access_mode":"read_only","executor":"android_private_server_linux","policy_revision":1,"allowed_tools":["read_text_file"],"budgets":{"max_steps":32,"max_seconds":1800,"max_output_tokens":1024,"max_total_tokens":262144},"folders":[{"id":cowork_storage::android_private_tools::folder_id(run),"project_id":project,"name":"Private","writable":false,"revision":"1","resource_id":run,"order_id":thread}]})).unwrap();
    let cp = s
        .android_private_loop_initialize(
            run,
            lease.lease,
            &spec,
            &[json!({"role":"user","content":spec.prompt})],
            &Keys,
            key,
        )
        .await
        .unwrap();
    let logical = cowork_domain::model_request_fingerprint(
        &spec,
        cp["messages"].as_array().unwrap(),
        &cowork_domain::model_tool_catalog(&spec),
    )
    .unwrap();
    let intent = json!({"format":1,"run":run,"lease":lease.lease,"provider_sha256":canonical_hash(&p),"spec_sha256":canonical_hash(&spec),"messages":cp["messages"],"logical_sha256":logical,"context_measurement":{"mode":"conservative_bytes","exact":false},"wire_sha256":"11".repeat(32),"request_payload_base64":"e30="});
    let (seq, _) = s
        .android_private_loop_model_admit(
            run,
            lease.lease,
            &canonical_hash(&cp),
            &intent,
            &Keys,
            key,
        )
        .await
        .unwrap();
    let received = json!({"format":1,"intent_sha256":canonical_hash(&intent),"wire_evidence":{"completed_stream":true},"content":"The reviewed plan","tools":[],"usage":{"input_tokens":10,"output_tokens":5}});
    s.android_private_loop_model_received(run, lease.lease, seq, &received, &Keys, key)
        .await
        .unwrap();
    let cp = s
        .android_private_loop_checkpoint(run, lease.lease, &Keys)
        .await
        .unwrap();
    assert!(cp["last_content"].is_null());
    assert_eq!(cp["model_steps"], 1);
    assert_eq!(
        cp["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["cowork_phase"] == cowork_domain::HYBRID_EXECUTION_MARKER)
            .count(),
        1
    );
    assert!(!cowork_domain::hybrid_planning(
        &spec,
        cp["messages"].as_array().unwrap()
    ));
    s.android_private_loop_model_received(run, lease.lease, seq, &received, &Keys, key)
        .await
        .unwrap();
    let again = s
        .android_private_loop_checkpoint(run, lease.lease, &Keys)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&cp), canonical_hash(&again));
}
