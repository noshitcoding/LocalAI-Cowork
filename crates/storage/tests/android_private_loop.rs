//! One product SQLite smoke: actual actor authority, approvals, encrypted files and unknown-effect fencing.
use cowork_contracts::{RunSpec, ToolCall};
use cowork_domain::{Error, canonical_hash};
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{
    Store, android_personal_provider,
    android_private_tools::{FILE_TOOLS, PrivateToolProgress, folder_id},
};
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
fn intent(run: Uuid, lease: Uuid, profile: &Value, cp: &Value) -> Value {
    let spec: RunSpec = serde_json::from_value(cp["spec"].clone()).unwrap();
    let logical = cowork_domain::model_request_fingerprint(
        &spec,
        cp["messages"].as_array().unwrap(),
        &cowork_domain::model_tool_catalog(&spec),
    )
    .unwrap();
    json!({"format":1,"run":run,"lease":lease,"provider_sha256":canonical_hash(profile),"spec_sha256":canonical_hash(&cp["spec"]),"messages":cp["messages"],"logical_sha256":logical,"context_measurement":{"mode":"conservative_bytes","exact":false},"wire_sha256":"11".repeat(32),"request_payload_base64":"e30="})
}
fn response(intent: &Value, content: &str, tools: &[ToolCall]) -> Value {
    json!({"format":1,"intent_sha256":canonical_hash(intent),"wire_evidence":{"completed_stream":true,"synthetic_sqlite_fixture":true},"content":content,"tools":tools,"usage":{"input_tokens":10,"output_tokens":5,"cost":null,"price_source_version":null}})
}
#[tokio::test]
async fn typed_actor_pipeline_requires_own_review_and_never_grants_duplicate_dispatch_after_reopen()
{
    use cowork_contracts::AndroidWorkflowWrite;
    use cowork_storage::android_private_mcp_runtime::PrivateMcpGate;
    use cowork_storage::android_private_workflows::with_private_workflow_node;
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("actor-pipeline.sqlite");
    let store = Store::sqlite(&database).await.unwrap();
    let identities = store.identity();
    let bootstrap = "b".repeat(64);
    identities
        .initialize_credentials(&bootstrap, &"s".repeat(64))
        .await
        .unwrap();
    let (_, session) = identities
        .bootstrap(
            &bootstrap,
            "pipeline-owner",
            "Owner",
            "SyntheticPassword-2026!",
            "Fixture",
        )
        .await
        .unwrap();
    let actor = identities
        .authenticate_session(&session.token)
        .await
        .unwrap();
    let key = KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    };
    let project=uuid(&store.android_personal_create_project(&actor,json!({"privacy":"private_local","team_id":null,"name":"Actor pipeline","description":"","policy":{},"preferred_executor_target":{"kind":"server_linux","pool_id":null}})).await.unwrap(),"id");
    let thread=uuid(&store.android_personal_create_thread(&actor,json!({"project_id":project,"title":"Fetch reviewed evidence","forked_from_thread_id":null,"forked_from_message_id":null}),None).await.unwrap(),"id");
    let profile=store.android_personal_provider_mutate(&actor,"create",None,json!({"team_id":null,"name":"Synthetic model","provider_kind":"openai_compatible","model_defaults":{"base_url":"https://fixture.invalid/v1","model":"fixture","auth_mode":"none","context_tokens":65536}}),Some("pipeline-model"),&Keys,key).await.unwrap();
    let budgets = json!({"max_steps":32,"max_seconds":1800,"max_output_tokens":1024,"max_total_tokens":262144});
    let rights = json!({"tools":["browser"],"folders":[]});
    let definition = json!({"format_version":1,"name":"Private deterministic fetch","mode":{"mode":"sequential"},"failure":"stop","rights":rights,"budgets":budgets,"roles":[{"id":"reader","instructions":"Read exactly the selected page","provider_id":profile["id"],"provider_revision":1,"rights":rights,"budgets":budgets,"delegate_to":[]}],"steps":[{"id":"fetch","role_id":"reader","goal":"Fetch the selected evidence","dependencies":[],"rights":rights,"budgets":budgets,"execution":{"kind":"web_fetch","url":"https://example.org/evidence"},"zero_context":true}]});
    let saved=store.android_workflow_write(&actor,None,&serde_json::from_value::<AndroidWorkflowWrite>(json!({"request_id":Uuid::new_v4(),"project_id":project,"expected_revision":0,"name":"Private deterministic fetch","engine":"workflow","definition":definition,"publish":true})).unwrap(),&Keys,key).await.unwrap();
    let engine = json!({"format":"android-private-workflow-engine/1","engine":"workflow","instance_id":Uuid::new_v4(),"protocol":"actor-private-workflow/1","dispatcher_sha256":"a".repeat(64)});
    store
        .android_workflow_engine_register(&engine)
        .await
        .unwrap();
    let engine_stamp = store
        .android_workflow_engine_admission(&actor, project, &engine)
        .await
        .unwrap();
    let runner = json!({"format":2,"runtime":"runsc","network":"none","host_id":Uuid::new_v4(),"engine_id":"fixture-engine","image":"fixture@sha256:".to_owned()+&"a".repeat(64),"supervisor_uid":0,"process_uid":1000,"capabilities":["SETUID","SETGID","KILL","DAC_OVERRIDE","CHOWN"],"programs":{},"browser":true,"office":false,"github":false});
    store
        .android_private_runner_register(Some(&runner))
        .await
        .unwrap();
    let runner_stamp = store
        .android_private_runner_admission(&actor, project, &runner)
        .await
        .unwrap();
    let config = json!({"runner":"workflow","workflow_id":saved.workflow.id,"workflow_version_id":saved.version.unwrap().id});
    let frozen = store
        .android_workflow_freeze_task(
            &actor,
            project,
            &config,
            uuid(&profile, "id"),
            &Keys,
            &engine_stamp,
        )
        .await
        .unwrap()
        .unwrap();
    let input = json!({"prompt":"Read reviewed evidence","provider_binding":android_personal_provider::binding(&profile).unwrap(),"private_workflow":frozen,"frozen_private_runner":runner,"private_runner_authority":runner_stamp});
    let request = json!({"project_id":project,"thread_id":thread,"project_revision":1,"project_privacy":"private_local","executor_target":{"kind":"server_linux","pool_id":null},"required_capabilities":["browser.headless","workflow.pure"],"model_profile_id":profile["id"],"snapshot_id":null,"task":null,"input":input,"idempotency_key":"pipeline-original"});
    let run = uuid(
        &store
            .android_private_server_create(&actor, &request)
            .await
            .unwrap()["spec"],
        "id",
    );
    let lease = store
        .android_private_server_claim_for_runner(Some(&runner))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.run, run);
    let spec:RunSpec=serde_json::from_value(json!({"order_id":thread,"project_id":project,"prompt":"Read reviewed evidence","provider":cowork_storage::actor_model_registry::provider_profile(&profile).unwrap(),"access_mode":"confirm_mutations","executor":"android_private_server_linux","policy_revision":1,"allowed_tools":["browser"],"budgets":budgets,"folders":[{"id":folder_id(run),"project_id":project,"name":"Private fixture workspace","writable":true,"revision":"1","resource_id":run,"order_id":thread}]})).unwrap();
    store
        .android_private_workflow_initialize(run, lease.lease, &spec, &Keys, key)
        .await
        .unwrap();
    let scope = store
        .android_private_workflow_claim_node(run, lease.lease, "fetch", &Keys, key)
        .await
        .unwrap();
    with_private_workflow_node(scope.clone(), async {
        assert!(
            store
                .android_private_workflow_pipeline_progress(run, lease.lease, &Keys, key)
                .await
                .unwrap()
        );
        let cp = store
            .android_private_loop_checkpoint(run, lease.lease, &Keys)
            .await
            .unwrap();
        assert_eq!(cp["model_steps"], 0);
        assert_eq!(cp["total_tokens"], 0);
        assert_eq!(
            cp["pending_tools"][0]["arguments"]["allowed_origins"],
            json!(["https://example.org"])
        );
        assert_eq!(
            cp["pending_tools"][0]["arguments"]["allowed_methods"],
            json!(["GET", "HEAD"])
        );
        assert!(
            store
                .android_private_workflow_pipeline_progress(run, lease.lease, &Keys, key)
                .await
                .is_err()
        );
        let PrivateMcpGate::Waiting(review) = store
            .android_private_browser_gate(run, lease.lease, &Keys, key)
            .await
            .unwrap()
        else {
            panic!("Exact actor review must be requested")
        };
        assert!(
            store
                .android_private_browser_admit(run, lease.lease, &Keys, key)
                .await
                .is_err()
        );
        store
            .android_private_loop_resolve(
                &actor,
                run,
                uuid(&review, "id"),
                "approval",
                1,
                json!("approved"),
                &Keys,
            )
            .await
            .unwrap();
        let admitted = store
            .android_private_browser_admit(run, lease.lease, &Keys, key)
            .await
            .unwrap();
        assert_eq!(admitted.action["operation"], "fetch_text");
        assert!(admitted.new_session);
        assert!(
            store
                .android_private_browser_admit(run, lease.lease, &Keys, key)
                .await
                .is_err()
        );
    })
    .await;
    let reopened = Store::sqlite(&database).await.unwrap();
    with_private_workflow_node(scope, async {
        assert!(
            reopened
                .android_private_browser_admit(run, lease.lease, &Keys, key)
                .await
                .is_err()
        );
        assert!(
            reopened
                .android_private_workflow_pipeline_progress(run, lease.lease, &Keys, key)
                .await
                .is_err()
        );
        let cp = reopened
            .android_private_loop_checkpoint(run, lease.lease, &Keys)
            .await
            .unwrap();
        assert_eq!(cp["model_steps"], 0);
        assert_eq!(cp["pending_tools"].as_array().unwrap().len(), 1);
    })
    .await;
}
#[tokio::test]
async fn private_loop_keeps_exact_receipts_and_encrypted_results() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("private-loop.sqlite"))
        .await
        .unwrap();
    let identities = store.identity();
    let bootstrap = "b".repeat(64);
    identities
        .initialize_credentials(&bootstrap, &"s".repeat(64))
        .await
        .unwrap();
    let (_, issued) = identities
        .bootstrap(
            &bootstrap,
            "private-loop-owner",
            "Private loop owner",
            "SyntheticPassword-2026!",
            "Fixture organization",
        )
        .await
        .unwrap();
    let actor = identities
        .authenticate_session(&issued.token)
        .await
        .unwrap();
    identities
        .register_local_identity(
            "private-loop-other",
            "Other actor",
            "OtherSyntheticPassword-2026!",
        )
        .await
        .unwrap();
    let other = identities
        .login(
            "private-loop-other",
            "OtherSyntheticPassword-2026!",
            3_600_000,
        )
        .await
        .unwrap();
    let other = identities.authenticate_session(&other.token).await.unwrap();
    let key = KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    };
    let p=store.android_personal_create_project(&actor,json!({"privacy":"private_local","team_id":null,"name":"Private SQL smoke","description":"","policy":{},"preferred_executor_target":{"kind":"server_linux","pool_id":null}})).await.unwrap();
    let project = uuid(&p, "id");
    let t=store.android_personal_create_thread(&actor,json!({"project_id":project,"title":"Original thread","forked_from_thread_id":null,"forked_from_message_id":null}),None).await.unwrap();
    let thread = uuid(&t, "id");
    let learning_skill = Uuid::new_v4();
    let skill_request = json!({"request_id":Uuid::new_v4(),"expected_revision":0,"value":{"project_id":project,"name":"Exact result review","description":"Selected frozen guidance","prompt_template":"ORIGINAL_LEARNING_TEMPLATE {{input}}","trigger_pattern":null,"run_mode":"execute","enabled":true,"auto_learn":true}});
    store
        .actor_context_mutate(&actor, "skills", learning_skill, &skill_request, false)
        .await
        .unwrap();
    let learning_context = json!({"skill":store.android_wire_runtime_metadata(&actor,project,"skill",&[learning_skill]).await.unwrap()});
    let profile=store.android_personal_provider_mutate(&actor,"create",None,json!({"team_id":null,"name":"Synthetic server provider","provider_kind":"openai_compatible","model_defaults":{"base_url":"https://example.test/v1","model":"fixture-model","auth_mode":"none","context_tokens":65536}}),Some("fixture-provider"),&Keys,key).await.unwrap();
    let request = json!({"project_id":project,"thread_id":thread,"project_revision":1,"project_privacy":"private_local","executor_target":{"kind":"server_linux","pool_id":null},"required_capabilities":[],"model_profile_id":profile["id"],"snapshot_id":null,"task":null,"input":{"prompt":"Create an exact result file","provider_binding":android_personal_provider::binding(&profile).unwrap(),"frozen_runtime_context":learning_context},"idempotency_key":"original-run"});
    let initial = store
        .android_private_server_create(&actor, &request)
        .await
        .unwrap();
    let run = uuid(&initial["spec"], "id");
    let lease = store.android_private_server_claim().await.unwrap().unwrap();
    assert_eq!(lease.run, run);
    let spec:RunSpec=serde_json::from_value(json!({"order_id":thread,"project_id":project,"prompt":"Create an exact result file","provider":{"id":profile["id"],"name":profile["name"],"kind":"openai_compatible","endpoint":profile["model_defaults"]["base_url"],"model":profile["model_defaults"]["model"],"revision":1,"context_tokens":65536,"credential_required":false,"credential_ready":true},"access_mode":"confirm_mutations","executor":"android_private_server_linux","policy_revision":1,"allowed_tools":FILE_TOOLS,"budgets":{"max_steps":32,"max_seconds":1800,"max_output_tokens":1024,"max_total_tokens":262144},"folders":[{"id":folder_id(run),"project_id":project,"name":"Private fixture workspace","writable":true,"revision":"1","resource_id":run,"order_id":thread}]})).unwrap();
    let cp = store
        .android_private_loop_initialize(
            run,
            lease.lease,
            &spec,
            &[json!({"role":"system","content":cowork_storage::android_private_workflows::render_actor_skills(&learning_context,&spec.prompt).unwrap()}),json!({"role":"user","content":spec.prompt})],
            &Keys,
            key,
        )
        .await
        .unwrap();
    let original = intent(run, lease.lease, &profile, &cp);
    let (sequence, secret) = store
        .android_private_loop_model_admit(
            run,
            lease.lease,
            &canonical_hash(&cp),
            &original,
            &Keys,
            key,
        )
        .await
        .unwrap();
    assert!(secret.is_none());
    assert!(matches!(
        store
            .android_private_loop_model_admit(
                run,
                lease.lease,
                &canonical_hash(&cp),
                &original,
                &Keys,
                key
            )
            .await,
        Err(Error::UncertainEffect)
    ));
    let tools = vec![
        ToolCall {
            id: "write-original".into(),
            name: "write_text_file".into(),
            arguments: json!({"folder_id":folder_id(run),"path":"notes.txt","content":"original private bytes","expected_sha256":null}),
        },
        ToolCall {
            id: "read-original".into(),
            name: "read_text_file".into(),
            arguments: json!({"folder_id":folder_id(run),"path":"notes.txt"}),
        },
        ToolCall {
            id: "ask-original".into(),
            name: "request_input".into(),
            arguments: json!({"question":"Finish this original result?"}),
        },
    ];
    let received = response(&original, "Working on the file", &tools);
    store
        .android_private_loop_model_received(run, lease.lease, sequence, &received, &Keys, key)
        .await
        .unwrap();
    let PrivateToolProgress::Waiting(approval) = store
        .android_private_loop_tool(run, lease.lease, &Keys, key)
        .await
        .unwrap()
    else {
        panic!("Mutation was not approved")
    };
    let approval_id = uuid(&approval, "id");
    assert!(
        store
            .android_private_loop_resolve(
                &other,
                run,
                approval_id,
                "approval",
                1,
                json!("approved"),
                &Keys
            )
            .await
            .is_err()
    );
    store
        .android_private_loop_resolve(
            &actor,
            run,
            approval_id,
            "approval",
            1,
            json!("approved"),
            &Keys,
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .android_private_loop_tool(run, lease.lease, &Keys, key)
            .await
            .unwrap(),
        PrivateToolProgress::Completed(_)
    ));
    let PrivateToolProgress::Completed(read) = store
        .android_private_loop_tool(run, lease.lease, &Keys, key)
        .await
        .unwrap()
    else {
        panic!("Read required no mutation approval")
    };
    assert_eq!(read["result"]["content"], "original private bytes");
    let PrivateToolProgress::Waiting(input) = store
        .android_private_loop_tool(run, lease.lease, &Keys, key)
        .await
        .unwrap()
    else {
        panic!("Input did not wait")
    };
    store
        .android_private_loop_resolve(
            &actor,
            run,
            uuid(&input, "id"),
            "input",
            1,
            json!({"text":"Finish"}),
            &Keys,
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .android_private_loop_tool(run, lease.lease, &Keys, key)
            .await
            .unwrap(),
        PrivateToolProgress::Completed(_)
    ));
    let cp = store
        .android_private_loop_checkpoint(run, lease.lease, &Keys)
        .await
        .unwrap();
    let final_intent = intent(run, lease.lease, &profile, &cp);
    let (seq, _) = store
        .android_private_loop_model_admit(
            run,
            lease.lease,
            &canonical_hash(&cp),
            &final_intent,
            &Keys,
            key,
        )
        .await
        .unwrap();
    let final_received = response(&final_intent, "Finished", &[]);
    store
        .android_private_loop_model_received(run, lease.lease, seq, &final_received, &Keys, key)
        .await
        .unwrap();
    assert!(matches!(
        store
            .android_private_loop_model_received(
                run,
                lease.lease,
                seq,
                &response(&final_intent, "Changed original", &[]),
                &Keys,
                key
            )
            .await,
        Err(Error::Conflict)
    ));
    let mut edited_skill = skill_request.clone();
    edited_skill["request_id"] = json!(Uuid::new_v4());
    edited_skill["expected_revision"] = json!(1);
    edited_skill["value"]["prompt_template"] = json!("EDITED_AFTER_ADMISSION {{input}}");
    store
        .actor_context_mutate(&actor, "skills", learning_skill, &edited_skill, false)
        .await
        .unwrap();
    let finished = store
        .android_private_loop_finish(run, lease.lease, &Keys, key)
        .await
        .unwrap();
    assert_eq!(finished["state"], "completed");
    let learning = store
        .actor_skill_learning(&actor, project, learning_skill, &Keys)
        .await
        .unwrap();
    assert_eq!(learning.items.len(), 1);
    let observed = &learning.items[0];
    assert_eq!(observed.skill_revision, 1);
    assert_eq!(
        observed.source_definition["prompt_template"],
        "ORIGINAL_LEARNING_TEMPLATE {{input}}"
    );
    assert!(observed.observed_tools.contains(&"write_text_file".into()));
    let proposed = observed.proposed_prompt_template.clone().unwrap();
    assert!(proposed.starts_with("ORIGINAL_LEARNING_TEMPLATE"));
    assert!(!proposed.contains("EDITED_AFTER_ADMISSION"));
    assert_eq!(
        store
            .actor_context(&actor, "skills", Some(project))
            .await
            .unwrap()[0]
            .revision,
        2,
        "Outcome capture never silently edits a skill"
    );
    let reopened = Store::sqlite(&dir.path().join("private-loop.sqlite"))
        .await
        .unwrap();
    assert_eq!(
        reopened
            .actor_skill_learning(&actor, project, learning_skill, &Keys)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert!(
        reopened
            .actor_skill_learning(&other, project, learning_skill, &Keys)
            .await
            .is_err()
    );
    reopened.close().await;
    let mut adoption = edited_skill.clone();
    adoption["request_id"] = json!(Uuid::new_v4());
    adoption["value"]["prompt_template"] = json!(proposed);
    assert!(matches!(
        store
            .actor_context_mutate(&actor, "skills", learning_skill, &adoption, false)
            .await,
        Err(Error::Conflict)
    ));
    adoption["expected_revision"] = json!(2);
    assert_eq!(
        store
            .actor_context_mutate(&actor, "skills", learning_skill, &adoption, false)
            .await
            .unwrap()
            .revision,
        3
    );
    assert_eq!(finished["result"]["usage"]["input_tokens"], 20);
    assert_eq!(finished["result"]["tool_steps"], 3);
    let snapshot = uuid(&finished["result"], "result_snapshot_id");
    let manifest = store
        .android_private_snapshot(&actor, snapshot, false)
        .await
        .unwrap();
    let digest = manifest["files"][0]["chunks"][0]["digest"]
        .as_str()
        .unwrap();
    let (_, bytes) = store
        .android_private_snapshot_chunk(&actor, snapshot, digest, None, &Keys, key)
        .await
        .unwrap();
    assert_eq!(bytes, b"original private bytes");
    assert!(
        store
            .android_private_snapshot(&other, snapshot, false)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .android_private_server_create(&actor, &request)
            .await
            .unwrap(),
        initial
    );
    let mut unknown = request.clone();
    unknown["idempotency_key"] = json!("unknown-original");
    let unknown = store
        .android_private_server_create(&actor, &unknown)
        .await
        .unwrap();
    let unknown_id = uuid(&unknown["spec"], "id");
    let l = store.android_private_server_claim().await.unwrap().unwrap();
    assert_eq!(l.run, unknown_id);
    let mut spec = spec;
    spec.folders[0].id = folder_id(unknown_id);
    spec.folders[0].resource_id = unknown_id.to_string();
    let cp = store
        .android_private_loop_initialize(
            unknown_id,
            l.lease,
            &spec,
            &[json!({"role":"user","content":spec.prompt})],
            &Keys,
            key,
        )
        .await
        .unwrap();
    let intent = intent(unknown_id, l.lease, &profile, &cp);
    store
        .android_private_loop_model_admit(
            unknown_id,
            l.lease,
            &canonical_hash(&cp),
            &intent,
            &Keys,
            key,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .android_private_server_cancel(&actor, unknown_id)
            .await
            .unwrap()["state"],
        "interrupted"
    );
    assert!(
        store
            .android_private_server_claim()
            .await
            .unwrap()
            .is_none()
    );
    store.close().await;
}
