//! Narrow actual SQLite admission/receipt/lease fixture; qualifies no real deployed model.
use cowork_contracts::*;
use cowork_domain::{Error, canonical_hash, model_request_fingerprint, model_tool_catalog};
use cowork_model_accounting::{digest, registry::*, sha256, tokenizer::*};
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{Store, actor_model_registry, android_personal_provider};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;
use zeroize::Zeroizing;
struct Keys;
impl KeyStore for Keys {
    fn load(&self, _: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        Ok(Zeroizing::new([39; 32]))
    }
}
fn id(value: &Value, key: &str) -> Uuid {
    Uuid::parse_str(value[key].as_str().unwrap()).unwrap()
}

#[test]
fn neutral_profile_projection_preserves_pre_generation_qualification_hashes() {
    let provider = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let defaults=android_personal_provider::normalized_defaults(json!({"base_url":"https://fixture.invalid/v1","model":"fixture","auth_mode":"none","temperature":0.0})).unwrap();
    assert_eq!(defaults["timeout_ms"], 120000);
    let source = json!({"id":provider,"owner_user_id":owner,"team_id":null,"deleted_at":null,"revision":1,"has_secret":false,"name":"Fixture","provider_kind":"openai_compatible","model_defaults":defaults});
    let projected = actor_model_registry::provider_profile(&source).unwrap();
    let original = json!({"id":provider,"name":"Fixture","kind":"openai_compatible","endpoint":"https://fixture.invalid/v1","model":"fixture","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true});
    assert_eq!(serde_json::to_value(&projected).unwrap(), original);
    assert_eq!(canonical_hash(&projected), canonical_hash(&original));
}
fn qualification(profile: &ProviderProfile) -> QualificationBundle {
    let vocab: BTreeMap<String, u32> = BTreeMap::from([("[UNK]".into(), 0)]);
    let artifact=json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":null,"pre_tokenizer":{"type":"WhitespaceSplit"},"post_processor":null,"decoder":null,"model":{"type":"WordLevel","vocab":vocab,"unk_token":"[UNK]"}}).to_string();
    let prefixes = BTreeMap::from([
        ("system".into(), "system: ".into()),
        ("user".into(), "user: ".into()),
        ("assistant".into(), "assistant: ".into()),
        ("tool".into(), "tool: ".into()),
    ]);
    let suffixes = BTreeMap::from([
        ("system".into(), " ".into()),
        ("user".into(), " ".into()),
        ("assistant".into(), " ".into()),
        ("tool".into(), " ".into()),
    ]);
    let template = CompleteChatTemplate {
        format: 1,
        prefix: "synthetic fixture ".into(),
        role_prefix: prefixes,
        role_suffix: suffixes,
        tools_prefix: "tools: ".into(),
        tools_suffix: " ".into(),
        output_prefix: "output: ".into(),
        output_suffix: " ".into(),
        generation_prompt: "assistant: ".into(),
    };
    let chat_template_artifact = serde_json::to_string(&template).unwrap();
    let inputs = [
        (
            vec![
                json!({"role":"system","content":"fixture"}),
                json!({"role":"user","content":"Hello 你好"}),
            ],
            json!([]),
            Value::Null,
        ),
        (
            vec![
                json!({"role":"assistant","content":"create","tool_calls":[{"id":"artifact-call","type":"function","function":{"name":"create_artifact","arguments":"{\"name\":\"a.txt\",\"content\":\"fixture\"}"}}]}),
                json!({"role":"tool","tool_call_id":"artifact-call","name":"create_artifact","content":"confirmed"}),
            ],
            json!([{"type":"function","function":{"name":"create_artifact","parameters":{"type":"object"}}}]),
            Value::Null,
        ),
        (
            vec![json!({"role":"user","content":"Structured final"})],
            json!([]),
            json!({"name":"final","schema":{"type":"object","properties":{"result":{"type":"string"}},"required":["result"],"additionalProperties":false}}),
        ),
    ];
    // Independent explicit synthetic whitespace vectors, not an attestation
    // about any deployed model or a self-qualification product mechanism.
    let requests: Vec<_> = inputs
        .into_iter()
        .map(|(messages, tools, output)| {
            let mut text = String::from("synthetic fixture ");
            for m in &messages {
                text += &format!("{}: {} ", m["role"].as_str().unwrap(), m);
            }
            text += &format!("tools: {TOOL_PROTOCOL}{tools} output: {output} assistant: ");
            let token_ids = vec![0; text.split_whitespace().count()];
            ReferenceRequest {
                messages,
                tools,
                output,
                rendered_text: text,
                token_ids,
            }
        })
        .collect();
    let evidence = DeploymentEvidence {
        issuer: "explicit synthetic test fixture".into(),
        reference_implementation: "synthetic whitespace reference; no real model qualification"
            .into(),
        deployment_sha256: sha256(b"synthetic-deployment"),
        reviewed_at: 1,
        reference_requests: requests,
    };
    let identity = ModelIdentity {
        provider_id: profile.id.clone(),
        model_id: profile.model.clone(),
        model_sha256: sha256(b"synthetic-weights"),
        tokenizer_sha256: sha256(artifact.as_bytes()),
        vocabulary_sha256: digest(&vocab).unwrap(),
        chat_template_sha256: sha256(chat_template_artifact.as_bytes()),
        context_tokens: u64::from(profile.context_tokens),
    };
    let qualification = TokenizerQualification {
        model: identity,
        engine: ENGINE.into(),
        reference_implementation: evidence.reference_implementation.clone(),
        parity_evidence_sha256: digest(&evidence).unwrap(),
        reference_encodings: evidence
            .reference_requests
            .iter()
            .map(|s| ReferenceEncoding {
                rendered_text: s.rendered_text.clone(),
                token_ids: s.token_ids.clone(),
            })
            .collect(),
    };
    QualificationBundle {
        format: 1,
        id: "synthetic-operator-model".into(),
        version: 1,
        provider_sha256: canonical_hash(profile),
        wire_mode: RawWireMode::OpenaiCompletions,
        renderer: RENDERER.into(),
        tokenizer_artifact: artifact,
        chat_template_artifact,
        qualification,
        evidence,
    }
}

#[tokio::test]
async fn actor_admission_uses_original_receipt_and_live_private_lease() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&temp.path().join("actor-registry.sqlite"))
        .await
        .unwrap();
    let identity = store.identity();
    let bootstrap = "b".repeat(64);
    identity
        .initialize_credentials(&bootstrap, &"s".repeat(64))
        .await
        .unwrap();
    let (_, issued) = identity
        .bootstrap(
            &bootstrap,
            "actor-model-owner",
            "Model owner",
            "SyntheticPassword-2026!",
            "Fixture organization",
        )
        .await
        .unwrap();
    let actor = identity.authenticate_session(&issued.token).await.unwrap();
    identity
        .register_local_identity(
            "actor-model-other",
            "Other actor",
            "OtherSyntheticPassword-2026!",
        )
        .await
        .unwrap();
    let issued_other = identity
        .login(
            "actor-model-other",
            "OtherSyntheticPassword-2026!",
            3_600_000,
        )
        .await
        .unwrap();
    let other = identity
        .authenticate_session(&issued_other.token)
        .await
        .unwrap();
    let key = KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    };
    let project=store.android_personal_create_project(&actor,json!({"privacy":"private_local","team_id":null,"name":"Qualified private fixture","description":"","policy":{},"preferred_executor_target":{"kind":"server_linux","pool_id":null}})).await.unwrap();
    let project_id = id(&project, "id");
    let second=store.android_personal_create_project(&actor,json!({"privacy":"private_local","team_id":null,"name":"Other private fixture","description":"","policy":{},"preferred_executor_target":{"kind":"server_linux","pool_id":null}})).await.unwrap();
    let thread=store.android_personal_create_thread(&actor,json!({"project_id":project_id,"title":"Qualified fixture thread","forked_from_thread_id":null,"forked_from_message_id":null}),None).await.unwrap();
    let profile=store.android_personal_provider_mutate(&actor,"create",None,json!({"team_id":null,"name":"Actual actor fixture profile","provider_kind":"openai_compatible","model_defaults":{"base_url":"https://example.test/v1","model":"fixture-model","auth_mode":"none","context_tokens":8192}}),Some("fixture-profile"),&Keys,key).await.unwrap();
    let context = store
        .actor_model_context(&actor, project_id, id(&profile, "id"))
        .await
        .unwrap();
    let neutral = actor_model_registry::provider_profile(&profile).unwrap();
    assert_eq!(canonical_hash(&context.provider), canonical_hash(&neutral));
    let bundle = qualification(&neutral);
    let input = ImportActorModelQualification {
        idempotency_key: Uuid::new_v4().to_string(),
        context,
        source_sha256: sha256(b"independent explicit synthetic fixture source"),
        qualification_bundle: json!(bundle),
    };
    assert!(
        store
            .import_actor_model_qualification(&other, &input)
            .await
            .is_err()
    );
    assert!(
        store
            .actor_model_context(&other, project_id, id(&profile, "id"))
            .await
            .is_err()
    );
    let receipt = store
        .import_actor_model_qualification(&actor, &input)
        .await
        .unwrap();
    assert_eq!(receipt.context.actor_id, actor.identity_id().to_string());
    assert_eq!(receipt.request_sha256, canonical_hash(&input));
    assert!(store.providers().await.unwrap().is_empty());
    assert!(store.model_registry().await.unwrap().is_empty());
    assert!(
        store
            .actor_model_registry(&other, project_id)
            .await
            .is_err()
    );
    assert!(
        store
            .actor_model_registry(&actor, id(&second, "id"))
            .await
            .unwrap()
            .is_empty()
    );
    let mut moved = input.clone();
    moved.context = store
        .actor_model_context(&actor, id(&second, "id"), id(&profile, "id"))
        .await
        .unwrap();
    assert!(matches!(
        store.import_actor_model_qualification(&actor, &moved).await,
        Err(Error::Conflict)
    ));
    let mut request = json!({"project_id":project_id,"thread_id":thread["id"],"project_revision":project["revision"],"project_privacy":"private_local","executor_target":{"kind":"server_linux","pool_id":null},"required_capabilities":[],"model_profile_id":profile["id"],"snapshot_id":null,"task":null,"input":{"prompt":"Qualified actor turn","provider_binding":android_personal_provider::binding(&profile).unwrap(),"model_binding":receipt.qualification.binding},"idempotency_key":"qualified-original-run"});
    let created = store
        .android_private_server_create(&actor, &request)
        .await
        .unwrap();
    let run = id(&created["spec"], "id");
    let lease = store.android_private_server_claim().await.unwrap().unwrap();
    assert_eq!(lease.run, run);
    let mut spec:RunSpec=serde_json::from_value(json!({"order_id":thread["id"],"project_id":project_id,"prompt":"Qualified actor turn","provider":neutral,"model_binding":receipt.qualification.binding,"executor":"android_private_server_linux","policy_revision":1,"access_mode":"read_only","allowed_tools":[],"folders":[],"budgets":{"max_steps":16,"max_seconds":120,"max_output_tokens":1024,"max_total_tokens":131072}})).unwrap();
    let registered = store
        .android_private_registered_model(run, lease.lease, &spec)
        .await
        .unwrap()
        .unwrap();
    assert!(store.registered_model(&spec).await.is_err());
    assert!(
        store
            .android_private_registered_model(run, Uuid::new_v4(), &spec)
            .await
            .is_err()
    );
    let messages = vec![
        json!({"role":"user","content":"actual full prompt"}),
        json!({"role":"assistant","tool_calls":[{"id":"fixture-call","type":"function","function":{"name":"fixture","arguments":"{}"}}]}),
        json!({"role":"tool","tool_call_id":"fixture-call","name":"fixture","content":"actual complete tool result"}),
    ];
    let logical = model_request_fingerprint(&spec, &messages, &model_tool_catalog(&spec)).unwrap();
    let count =
        actor_model_registry::validate_context(&registered, &spec, &messages, &logical).unwrap();
    assert!(count.input_tokens > 0);
    assert_eq!(count.output_allowance, 1024);
    assert!(
        actor_model_registry::validate_context(&registered, &spec, &messages, &"a".repeat(64))
            .is_err()
    );
    let huge = vec![json!({"role":"user","content":"word ".repeat(9000)})];
    let logical = model_request_fingerprint(&spec, &huge, &model_tool_catalog(&spec)).unwrap();
    assert!(matches!(
        actor_model_registry::validate_context(&registered, &spec, &huge, &logical),
        Err(Error::ContextLimit)
    ));
    spec.project_id = second["id"].as_str().unwrap().into();
    assert!(
        store
            .android_private_registered_model(run, lease.lease, &spec)
            .await
            .is_err()
    );
    spec.project_id = project_id.to_string();
    store
        .revoke_actor_model_qualification(
            &actor,
            &RevokeActorModelQualification {
                actor_id: actor.identity_id().to_string(),
                project_id: project_id.to_string(),
                binding: receipt.qualification.binding.clone(),
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .android_private_registered_model(run, lease.lease, &spec)
            .await
            .is_err()
    );
    let replay = store
        .import_actor_model_qualification(&actor, &input)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&receipt), canonical_hash(&replay));
    assert!(!replay.qualification.revoked);
    assert!(
        store
            .actor_model_registry(&actor, project_id)
            .await
            .unwrap()[0]
            .qualification
            .revoked
    );
    let mut altered = input.clone();
    altered.qualification_bundle["id"] = json!("changed-under-original-key");
    assert!(matches!(
        store
            .import_actor_model_qualification(&actor, &altered)
            .await,
        Err(Error::Conflict)
    ));
    let updated=store.android_personal_provider_mutate(&actor,"update",Some(id(&profile,"id")),json!({"expected_revision":1,"name":"Changed actual profile","provider_kind":"openai_compatible","model_defaults":profile["model_defaults"]}),Some("profile-change"),&Keys,key).await.unwrap();
    assert_eq!(updated["revision"], 2);
    assert_eq!(
        canonical_hash(
            &store
                .import_actor_model_qualification(&actor, &input)
                .await
                .unwrap()
        ),
        canonical_hash(&receipt)
    );
    request["idempotency_key"] = json!("new-stale-qualified-run");
    request["input"]["provider_binding"] = android_personal_provider::binding(&updated).unwrap();
    assert!(
        store
            .android_private_server_create(&actor, &request)
            .await
            .is_err()
    );
    identity
        .revoke_account_session(&actor, actor.session_id(), 1)
        .await
        .unwrap();
    assert!(
        store
            .import_actor_model_qualification(&actor, &input)
            .await
            .is_err()
    );
}
