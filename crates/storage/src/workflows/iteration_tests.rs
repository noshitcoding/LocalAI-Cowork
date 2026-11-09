//! Private derived-graph adapter tests. No public start, database seed, provider
//! request, broker grant or working-copy authority is created by these fixtures.
use super::*;
use std::collections::BTreeSet;

fn fixture(deterministic: bool) -> (StoredRun, StoredWorkflow) {
    let mut source: WorkflowIterationProgram = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/workflow-iteration-program.json"
    ))
    .unwrap();
    let WorkflowDataType::Object { fields } = &mut source.workflow.dataflow.as_mut().unwrap().input
    else {
        panic!()
    };
    fields.insert("private_note".into(), WorkflowDataType::Text);
    let rights = if deterministic {
        WorkflowRights {
            current_workspace: false,
            tools: BTreeSet::from(["browser".into()]),
            folders: BTreeSet::new(),
        }
    } else {
        WorkflowRights::default()
    };
    source.workflow.rights = rights.clone();
    let iteration = source.iterations.get_mut("loop").unwrap();
    iteration.source.rights = rights.clone();
    let body = &mut iteration.source.body;
    body.rights = rights.clone();
    let profile: ProviderProfile = serde_json::from_value(json!({
        "id":"model","name":"Private adapter fixture","kind":"test","endpoint":"",
        "model":"frozen-list-model","revision":1,"context_tokens":2048,
        "credential_required":false,"credential_ready":true
    }))
    .unwrap();
    body.roles = vec![
        serde_json::from_value(json!({
            "id":"worker","instructions":"Use only the selected item.",
            "provider_id":profile.id,"provider_revision":1,"rights":rights,
            "budgets":body.budgets,"delegate_to":[]
        }))
        .unwrap(),
    ];
    body.controls.clear();
    body.steps=vec![serde_json::from_value(json!({
        "id":"echo","role_id":"worker","goal":"Process this item","dependencies":[],
        "rights":rights,"budgets":body.budgets,
        "execution":if deterministic {json!({"kind":"web_fetch","url":"https://example.invalid/frozen"})} else {Value::Null}
    })).unwrap()];
    if deterministic {
        let data = body
            .dataflow
            .as_mut()
            .unwrap()
            .steps
            .get_mut("echo")
            .unwrap();
        data.inputs.clear();
        data.output = WorkflowDataType::Text;
        iteration.source.result.pointer.clear();
        let list = WorkflowDataType::List {
            item: Box::new(WorkflowDataType::Text),
            max_items: 1000,
        };
        let data = source
            .workflow
            .dataflow
            .as_mut()
            .unwrap()
            .steps
            .get_mut("after")
            .unwrap();
        data.inputs.get_mut("results").unwrap().value_type = list.clone();
        data.output = WorkflowDataType::Object {
            fields: BTreeMap::from([("results".into(), list)]),
        };
    }
    let scope = WorkflowScope {
        workspace_folder: None,
        rights: rights.clone(),
        budgets: source.workflow.budgets.clone(),
        max_parallel: 4,
    };
    let plan = WorkflowPlan::compile_iterations(&source, &scope, &[profile]).unwrap();
    let binding = WorkflowBinding {
        workflow_id: "private-test-workflow".into(),
        version_id: "private-test-version".into(),
        inputs: Some(Box::new(
            json!({"values":[7,-3,7],"private_note":"UNBOUND_PRIVATE_MARKER"}),
        )),
        plan_sha256: plan.sha256.clone(),
        definition_sha256: plan.source_sha256.clone(),
    };
    let spec: RunSpec = serde_json::from_value(json!({
        "project_id":"project","order_id":"order","prompt":"Process selected items.",
        "provider":null,"file_action":null,"workflow":binding,
        "access_mode":"confirm_mutations","executor":"local-v2","policy_revision":1,
        "allowed_tools":rights.tools,"budgets":plan.budgets,"folders":[],"environment":null,
        "input_workspace":{"id":"original-private-copy","input":{"snapshot_id":"original-snapshot",
            "sha256":"a".repeat(64),"manifest_sha256":"b".repeat(64)}}
    }))
    .unwrap();
    let parent = StoredRun {
        record: RunRecord {
            id: "parent".into(),
            spec,
            state: RunState::Running,
            revision: 1,
            last_sequence: 2,
            steps: 0,
            created_at: 100_000,
            updated_at: 100_000,
            started_at: Some(100_000),
            error_code: None,
            usage: Usage::default(),
            model_attempts: vec![],
        },
        checkpoint: cowork_domain::Checkpoint {
            messages: vec![json!({"role":"user","content":"Process selected items."})],
            ..Default::default()
        },
        lease: None,
    };
    let progress = WorkflowProgress::new(&plan).unwrap();
    let workflow = StoredWorkflow {
        format_version: 1,
        run_id: parent.record.id.clone(),
        project_id: "project".into(),
        binding,
        plan,
        progress,
        agents: BTreeMap::new(),
        inputs: BTreeMap::new(),
        samplings: BTreeMap::new(),
    };
    workflow.validate(&parent.record).unwrap();
    (parent, workflow)
}
fn native(workflow: &mut StoredWorkflow, node: &str) {
    let claim = workflow
        .progress
        .claim(&workflow.plan, workflow.progress.revision(), node, 100)
        .unwrap();
    workflow
        .progress
        .complete_control(
            &workflow.plan,
            workflow.progress.revision(),
            &claim,
            &format!("native-{}", workflow.progress.revision()),
            workflow.binding.inputs.as_deref(),
            100,
        )
        .unwrap();
}
fn materialize(workflow: &mut StoredWorkflow) {
    native(
        workflow,
        &workflow.plan.iterations["loop"].capture_node.clone(),
    );
    let entries: Vec<_> = workflow.execution_graph().unwrap().iteration_instances["loop"]
        .iter()
        .take(2)
        .map(|item| item.entry.clone())
        .collect();
    for entry in entries {
        native(workflow, &entry);
    }
}
fn model_claim(workflow: &mut StoredWorkflow, index: usize) -> WorkflowClaim {
    let graph = workflow.execution_graph().unwrap();
    let item = &graph.iteration_instances["loop"][index];
    let node = graph
        .nodes
        .values()
        .find(|node| node.parent.as_deref() == Some(&item.exit) && node.role_id.is_some())
        .unwrap()
        .id
        .clone();
    workflow
        .progress
        .claim(&workflow.plan, workflow.progress.revision(), &node, 100)
        .unwrap()
}

#[test]
fn dynamic_model_context_uses_only_selected_item_and_preserves_the_parent_working_copy() {
    let (parent, mut workflow) = fixture(false);
    materialize(&mut workflow);
    for (index, expected) in [(0, 7), (1, -3)] {
        let claim = model_claim(&mut workflow, index);
        let context = build_context(&parent, &workflow, &claim, json!([])).unwrap();
        let frame: Value = serde_json::from_str(
            context.messages.last().unwrap()["content"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(frame["inputs"], json!({"amount":expected}));
        assert!(
            !json_string(&context.messages)
                .unwrap()
                .contains("UNBOUND_PRIVATE_MARKER")
        );
        assert_eq!(
            context.spec.input_workspace,
            parent.record.spec.input_workspace
        );
        assert_eq!(context.spec.workflow, parent.record.spec.workflow);
        assert_eq!(context.spec.allowed_tools, parent.record.spec.allowed_tools);
        assert_eq!(
            context.spec.output,
            workflow
                .execution_graph()
                .unwrap()
                .definition
                .step_output_format(&claim.node_id)
                .unwrap()
        );
        assert!(context.spec.budgets.max_steps <= parent.record.spec.budgets.max_steps);
        assert!(workflow.step_execution(&claim.node_id).unwrap().is_none());
    }
    workflow.validate(&parent.record).unwrap();
}

#[test]
fn serialized_dynamic_agent_context_and_original_response_are_replay_validated() {
    let (parent, mut workflow) = fixture(false);
    materialize(&mut workflow);
    let claim = model_claim(&mut workflow, 0);
    let context = build_context(&parent, &workflow, &claim, json!([])).unwrap();
    let mut agent = make_agent(&parent, &workflow, &claim.node_id).unwrap();
    workflow
        .progress
        .admit(
            &workflow.plan,
            workflow.progress.revision(),
            &claim,
            "model-turn",
            WorkflowOperation::Model {
                request_sha256: context.request_sha256.clone(),
                reserved_tokens: None,
            },
            100,
        )
        .unwrap();
    agent.turns.push(WorkflowModelTurn {
        deterministic: false,
        id: "model-turn".into(),
        claim: claim.clone(),
        context,
        response: None,
        response_sha256: None,
        tool_results: vec![],
    });
    workflow.agents.insert(claim.node_id.clone(), agent);
    let reopened: StoredWorkflow = serde_json::from_str(&json_string(&workflow).unwrap()).unwrap();
    reopened.validate(&parent.record).unwrap();
    let mut response = WorkflowModelResponse {
        content: "{\"amount\":7}".into(),
        tools: vec![],
        usage: Usage {
            input_tokens: Some(3),
            output_tokens: Some(4),
            ..Default::default()
        },
        provider_continuation: None,
        received_cost_metadata: None,
        rejection_code: None,
    };
    response.rejection_code =
        response_rejection(&workflow.agents[&claim.node_id].turns[0].context, &response).unwrap();
    let digest = canonical_hash(&response);
    workflow
        .progress
        .settle_response(
            &workflow.plan,
            workflow.progress.revision(),
            &claim,
            "model-turn",
            WorkflowResponseEvidence {
                outcome: outcome(&response.usage),
                sha256: digest.clone(),
            },
            100,
        )
        .unwrap();
    let turn = &mut workflow.agents.get_mut(&claim.node_id).unwrap().turns[0];
    turn.response = Some(response);
    turn.response_sha256 = Some(digest);
    workflow
        .progress
        .complete(
            &workflow.plan,
            workflow.progress.revision(),
            &claim,
            "{\"amount\":7}",
            100,
        )
        .unwrap();
    let reopened: StoredWorkflow = serde_json::from_str(&json_string(&workflow).unwrap()).unwrap();
    reopened.validate(&parent.record).unwrap();
    assert_eq!(reopened.progress.charged_tokens(), 7);
    assert_eq!(reopened.plan.sha256, workflow.binding.plan_sha256);
    let mut edited = reopened;
    let agent = edited.agents.get_mut(&claim.node_id).unwrap();
    let last = agent.seed.last_mut().unwrap();
    last["content"] = json!({"inputs":{"amount":999}}).to_string().into();
    agent.seed_sha256 = canonical_hash(&agent.seed);
    assert!(edited.validate(&parent.record).is_err());
}

#[test]
fn dynamic_deterministic_steps_keep_their_original_native_proposal_and_fingerprint() {
    let (parent, mut workflow) = fixture(true);
    materialize(&mut workflow);
    let claim = model_claim(&mut workflow, 0);
    let catalog = json!([{"type":"function","function":{"name":"browser"}}]);
    let context = build_context(&parent, &workflow, &claim, catalog).unwrap();
    assert!(workflow.step_execution(&claim.node_id).unwrap().is_some());
    let response = step_response(&workflow, &claim.node_id).unwrap();
    assert_eq!(
        response.tools[0].arguments,
        json!({"operation":"fetch_text","url":"https://example.invalid/frozen"})
    );
    assert_eq!(
        context.request_sha256,
        cowork_domain::workflow_preparation_fingerprint(
            &context.spec,
            &context.messages,
            &context.tools
        )
        .unwrap()
    );
    assert!(response_rejection(&context, &response).unwrap().is_none());
    let mut agent = make_agent(&parent, &workflow, &claim.node_id).unwrap();
    let digest = canonical_hash(&response);
    workflow
        .progress
        .prepare_step(
            &workflow.plan,
            workflow.progress.revision(),
            &claim,
            "native-proposal",
            &context.request_sha256,
            &digest,
            100,
        )
        .unwrap();
    agent.turns.push(WorkflowModelTurn {
        deterministic: true,
        id: "native-proposal".into(),
        claim: claim.clone(),
        context,
        response: Some(response),
        response_sha256: Some(digest),
        tool_results: vec![],
    });
    workflow.agents.insert(claim.node_id.clone(), agent);
    workflow.validate(&parent.record).unwrap();
    assert_eq!(workflow.progress.charged_tokens(), 0);
    let reopened: StoredWorkflow = serde_json::from_str(&json_string(&workflow).unwrap()).unwrap();
    reopened.validate(&parent.record).unwrap();
    assert_eq!(
        reopened.step_execution(&claim.node_id).unwrap(),
        workflow.step_execution(&claim.node_id).unwrap()
    );
}
