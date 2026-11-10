//! Reproduce undeclared context channels through the actual accepted parent checkpoint.
use super::*;

const HISTORY: &str = "UNBOUND_PARENT_HISTORY_SENTINEL_OLD_INVOICE_99999";
const PROJECT: &str = "UNBOUND_PROJECT_CONTEXT_SENTINEL_OTHER_CUSTOMER";
const CURRENT: &str = "UNBOUND_CURRENT_PROMPT_SENTINEL_DO_NOT_EXTRACT";
const BASE: &str = "UNBOUND_BASE_INSTRUCTION_SENTINEL_OLD_PERSONA";
const ROLE: &str = "EXPLICIT_ADDITIONAL_ROLE_INSTRUCTIONS_SENTINEL";

async fn fixture(bound: bool) -> (Store, StoredRun, StoredWorkflow) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Context boundaries".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Used invoice chat".into(),
        })
        .await
        .unwrap();
    let provider: ProviderProfile = serde_json::from_value(json!({"id":"typed-fixture","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true})).unwrap();
    store.insert_provider(&provider).await.unwrap();
    let skill: WriteSkill = serde_json::from_value(json!({"id":null,"expected_revision":0,"name":"Unbound project context","description":"","instructions":PROJECT,"enabled":true})).unwrap();
    store
        .productivity_write_skill(&project.id, skill)
        .await
        .unwrap();
    let budgets = Budgets::default();
    let previous: CreateRun = serde_json::from_value(json!({"order_id":order.id,"base_revision":order.revision,"idempotency_key":"previous-chat","prompt":HISTORY,"provider_id":provider.id,"provider_revision":1,"access_mode":"read_only","budgets":budgets,"native_tools":[],"folders":[],"mcp_tools":[]})).unwrap();
    let single: RunSpec = serde_json::from_value(json!({"project_id":project.id,"order_id":order.id,"prompt":HISTORY,"provider":provider,"access_mode":"read_only","executor":"local-v2","policy_revision":1,"allowed_tools":[],"budgets":budgets,"folders":[],"mcp_tools":[],"workflow":null,"environment":null,"file_action":null,"output":null,"model_binding":null,"agent_config":null})).unwrap();
    store.create_run(&previous, single).await.unwrap();
    let mut raw: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/workflow-dataflow.json"
    ))
    .unwrap();
    if bound {
        raw["dataflow"]["steps"]["extract"]["context_policy"] = json!("bound_inputs_only");
    }
    raw["roles"][0]["agent_config"] = json!({"instructions":ROLE});
    let definition: WorkflowDefinition = serde_json::from_value(raw).unwrap();
    let entry = store
        .create_workflow(&project.id, "typed-workflow", &definition)
        .await
        .unwrap();
    let order = store.order(&order.id).await.unwrap();
    let input: cowork_workflow_contracts::WorkflowRunRequest = serde_json::from_value(json!({"kind":"workflow","order_id":order.id,"base_revision":order.revision,"idempotency_key":"extract-used-chat","prompt":CURRENT,"workflow_id":entry.id,"version_id":entry.versions[0].id,"access_mode":"read_only","budgets":definition.budgets,"inputs":{"path":"EXPLICIT_BOUND_DOCUMENT_TEXT","sha256":"EXPLICIT_BOUND_SHA","currency":"EUR","private_note":"UNBOUND_WORKFLOW_INPUT_SENTINEL"}})).unwrap();
    let allowed: Vec<_> = [
        "create_artifact",
        "list_artifacts",
        "read_artifact",
        "request_input",
    ]
    .into_iter()
    .filter(|name| cowork_domain::read_only_tool(name))
    .collect();
    let mut spec: RunSpec = serde_json::from_value(json!({"project_id":project.id,"order_id":order.id,"prompt":CURRENT,"provider":null,"access_mode":"read_only","executor":"local-v2","policy_revision":1,"allowed_tools":allowed,"budgets":definition.budgets,"folders":[],"mcp_tools":[],"workflow":null,"environment":null,"file_action":null,"output":null,"model_binding":null,"agent_config":null})).unwrap();
    spec.agent_config = Some(EffectiveAgentConfig {
        instructions: BASE.into(),
        ..Default::default()
    });
    let plan = store
        .product_preflight_compilation(&input, &spec)
        .await
        .unwrap();
    let binding = WorkflowBinding {
        inputs: input.inputs.clone(),
        workflow_id: input.workflow_id.clone(),
        version_id: input.version_id.clone(),
        definition_sha256: plan.source_sha256.clone(),
        plan_sha256: plan.sha256.clone(),
    };
    let run = store
        .create_product_workflow_run(&input, spec, &binding)
        .await
        .unwrap();
    let parent = store.stored_run(&run.id).await.unwrap();
    let workflow = store.stored_workflow(&project.id, &run.id).await.unwrap();
    let frozen = serde_json::to_string(&parent.checkpoint.messages).unwrap();
    assert!(frozen.contains(HISTORY) && frozen.contains(PROJECT) && frozen.contains(CURRENT));
    (store, parent, workflow)
}

#[tokio::test]
async fn bound_extraction_must_exclude_unbound_parent_project_and_current_request_text() {
    let (_, parent, workflow) = fixture(true).await;
    let seed =
        serde_json::to_string(&make_agent(&parent, &workflow, "extract").unwrap().seed).unwrap();
    assert!(seed.contains("EXPLICIT_BOUND_DOCUMENT_TEXT") && seed.contains("EXPLICIT_BOUND_SHA"));
    assert!(!seed.contains("UNBOUND_WORKFLOW_INPUT_SENTINEL"));
    let leaked: Vec<_> = [HISTORY, PROJECT, CURRENT, BASE, ROLE]
        .into_iter()
        .filter(|value| seed.contains(value))
        .collect();
    assert!(
        leaked.is_empty(),
        "undeclared context reached extraction model: {leaked:?}"
    );
}

#[tokio::test]
async fn ordinary_model_keeps_frozen_parent_project_and_exact_additional_system_instructions_at_admission()
 {
    let (store, parent, _) = fixture(false).await;
    let project = &parent.record.spec.project_id;
    let run = &parent.record.id;
    let claimed = store.claim_workflow_run(project, run).await.unwrap();
    let lease = claimed.lease.unwrap().token;
    let before = store.stored_workflow(project, run).await.unwrap();
    let WorkflowCommandResult::Claimed(claim) = store
        .workflow_command(
            project,
            run,
            &lease,
            before.progress.revision(),
            WorkflowCommand::Claim {
                node_id: "extract".into(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("claim")
    };
    let (revision, context) = store
        .workflow_model_context(project, run, &claim, json!([]))
        .await
        .unwrap();
    let full = serde_json::to_string(&context.messages).unwrap();
    for value in [HISTORY, PROJECT, CURRENT, BASE, ROLE] {
        assert!(full.contains(value), "ordinary context omitted {value}");
    }
    let admission = store
        .begin_workflow_model(
            project,
            run,
            &lease,
            revision,
            &claim,
            "ordinary-context-control",
            &context,
        )
        .await;
    assert!(
        admission.is_ok(),
        "valid additional frozen system instructions blocked model admission: {admission:?}"
    );
    let saved = store.stored_workflow(project, run).await.unwrap();
    saved
        .validate(&store.stored_run(run).await.unwrap().record)
        .unwrap();
}

#[tokio::test]
async fn bound_model_rejects_a_consistently_hashed_cached_seed_with_extra_context_frames() {
    let (_, parent, mut workflow) = fixture(true).await;
    let claim = workflow
        .progress
        .claim(
            &workflow.plan,
            workflow.progress.revision(),
            "extract",
            seconds(),
        )
        .unwrap();
    let original = make_agent(&parent, &workflow, "extract").unwrap();
    for frame in [
        json!({"role":"user","content":HISTORY}),
        json!({"role":"system","content":BASE}),
    ] {
        let mut contaminated = original.clone();
        contaminated.seed.insert(contaminated.seed.len() - 1, frame);
        contaminated.seed_sha256 = canonical_hash(&contaminated.seed);
        workflow.agents.insert("extract".into(), contaminated);
        assert!(matches!(
            build_context(&parent, &workflow, &claim, json!([])),
            Err(Error::Forbidden)
        ));
    }
    workflow.agents.insert("extract".into(), original);
    let accepted = build_context(&parent, &workflow, &claim, json!([])).unwrap();
    assert!(
        serde_json::to_string(&accepted.messages)
            .unwrap()
            .contains("EXPLICIT_BOUND_DOCUMENT_TEXT")
    );
}
