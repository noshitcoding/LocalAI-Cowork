use cowork_contracts::{ProviderKind, ProviderProfile};
use cowork_domain::{Error, canonical_hash, workflows::*};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn program() -> WorkflowComposition {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-composition.json"
    ))
    .unwrap()
}
fn input() -> Value {
    json!({"amount":7,"private_note":"Do not pass this into the child"})
}
fn scope(p: &WorkflowComposition) -> WorkflowScope {
    WorkflowScope {
        workspace_folder: None,
        rights: p.workflow.rights.clone(),
        budgets: p.workflow.budgets.clone(),
        max_parallel: 4,
    }
}
fn compile(p: &WorkflowComposition) -> WorkflowPlan {
    WorkflowPlan::compile_composition(p, &scope(p), &[]).unwrap()
}
fn native(p: &mut WorkflowProgress, plan: &WorkflowPlan, node: &str, at: i64) {
    let claim = p.claim(plan, p.revision(), node, at).unwrap();
    p.complete_control(
        plan,
        p.revision(),
        &claim,
        &format!("native-{}", p.revision()),
        Some(&input()),
        at,
    )
    .unwrap();
}
fn finish(plan: &WorkflowPlan) -> WorkflowProgress {
    let mut p = WorkflowProgress::new(plan).unwrap();
    for _ in 0..64 {
        let ready = p.ready_nodes(plan).unwrap();
        if ready.is_empty() {
            break;
        }
        native(&mut p, plan, &ready[0], 100);
    }
    p
}
fn node(plan: &WorkflowPlan, goal: &str) -> String {
    plan.nodes
        .values()
        .find(|n| n.goal == goal)
        .unwrap()
        .id
        .clone()
}

#[test]
fn native_subflow_uses_only_selected_inputs_and_returns_one_frozen_result() {
    let source = program();
    let plan = compile(&source);
    plan.validate().unwrap();
    assert_eq!(plan.source_sha256, canonical_hash(&source));
    assert!(plan.roles.is_empty() && plan.profiles.is_empty());
    let p = finish(&plan);
    assert_eq!(p.state(), WorkflowState::Completed);
    assert_eq!(p.attempts().len(), 3);
    assert_eq!(p.charged_tokens(), 0);
    assert_eq!(serde_json::to_value(&p).unwrap()["format_version"], 10);
    let WorkflowNodeState::Completed { output, .. } = &p.nodes()["after"].state else {
        panic!()
    };
    assert_eq!(
        serde_json::from_str::<Value>(output).unwrap(),
        json!({"final":7})
    );
    let entry = &plan.composition_scopes["invoke"].entry_node;
    let WorkflowNodeState::Completed { output, .. } = &p.nodes()[entry].state else {
        panic!()
    };
    assert_eq!(
        serde_json::from_str::<Value>(output).unwrap(),
        json!({"amount":7})
    );
    let saved: WorkflowProgress = serde_json::from_value(serde_json::to_value(p).unwrap()).unwrap();
    saved.validate(&plan).unwrap();
}

#[test]
fn a_real_pre_composition_plan_retains_its_wire_shape_and_digest() {
    let original: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-plan-before-composition.json"
    ))
    .unwrap();
    let plan: WorkflowPlan = serde_json::from_value(original.clone()).unwrap();
    plan.validate().unwrap();
    assert_eq!(plan.sha256, original["sha256"].as_str().unwrap());
    assert_eq!(
        canonical_hash(&serde_json::to_value(plan).unwrap()),
        canonical_hash(&original)
    );
}

#[test]
fn a_public_subflow_source_binds_the_original_tree_and_reconstructs_its_plan() {
    let source: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-subflow.json"
    ))
    .unwrap();
    source.validate_definition().unwrap();
    let plan = WorkflowPlan::compile(
        &source,
        &WorkflowScope {
            workspace_folder: None,
            rights: source.rights.clone(),
            budgets: source.budgets.clone(),
            max_parallel: 4,
        },
        &[],
    )
    .unwrap();
    assert!(plan.composition_definition);
    assert_eq!(plan.source_sha256, canonical_hash(&source));
    assert_ne!(plan.source_sha256, canonical_hash(&plan.definition));
    assert_eq!(plan.definition.format_version, 3);
    assert!(plan.definition.subflows.is_empty());
    assert_eq!(
        canonical_hash(
            &plan
                .composition
                .as_ref()
                .unwrap()
                .source_definition()
                .unwrap()
        ),
        canonical_hash(&source)
    );
    let saved: WorkflowPlan = serde_json::from_value(serde_json::to_value(&plan).unwrap()).unwrap();
    saved.validate().unwrap();
    let progress = finish(&saved);
    assert_eq!(progress.state(), WorkflowState::Completed);
    let mut changed = source.clone();
    changed.subflows.get_mut("invoke").unwrap().name = "Changed after review".into();
    assert_ne!(canonical_hash(&changed), plan.source_sha256);
    let mut wrong_version = source;
    wrong_version.format_version = 3;
    assert!(wrong_version.validate_definition().is_err());
}

#[test]
fn private_argument_frames_do_not_consume_the_callee_body_step_allowance() {
    let mut source = program();
    source.calls.get_mut("invoke").unwrap().budgets.max_steps = 1;
    source
        .calls
        .get_mut("invoke")
        .unwrap()
        .body
        .workflow
        .budgets
        .max_steps = 1;
    let plan = compile(&source);
    let p = finish(&plan);
    assert_eq!(p.state(), WorkflowState::Completed);
    assert_eq!(p.attempts().len(), 3);
}

#[test]
fn subflow_parallel_limit_is_enforced_at_claim_even_when_parent_has_free_slots() {
    let mut source = program();
    let body = &mut source.calls.get_mut("invoke").unwrap().body.workflow;
    let mut second = body.controls["echo"].clone();
    second.name = "Second body node".into();
    body.controls.insert("second".into(), second);
    let config = body.dataflow.as_ref().unwrap().steps["echo"].clone();
    body.dataflow
        .as_mut()
        .unwrap()
        .steps
        .insert("second".into(), config);
    let plan = compile(&source);
    let mut p = WorkflowProgress::new(&plan).unwrap();
    native(
        &mut p,
        &plan,
        &plan.composition_scopes["invoke"].entry_node,
        100,
    );
    let ready = p.ready_nodes(&plan).unwrap();
    assert_eq!(ready.len(), 2);
    p.claim(&plan, p.revision(), &ready[0], 100).unwrap();
    assert!(p.ready_nodes(&plan).unwrap().is_empty());
    let unchanged = canonical_hash(&p);
    assert!(matches!(
        p.claim(&plan, p.revision(), &ready[1], 100),
        Err(Error::WorkspaceBusy)
    ));
    assert_eq!(canonical_hash(&p), unchanged);
}

#[test]
fn recovery_of_an_argument_claim_cannot_restart_the_subflow_deadline() {
    let mut source = program();
    source.calls.get_mut("invoke").unwrap().budgets.max_seconds = 5;
    let plan = compile(&source);
    let mut p = WorkflowProgress::new(&plan).unwrap();
    let entry = &plan.composition_scopes["invoke"].entry_node;
    p.claim(&plan, 0, entry, 100).unwrap();
    p.recover(&plan, p.revision(), 101).unwrap();
    p.resume_settled(&plan, p.revision(), BTreeMap::new(), 102)
        .unwrap();
    native(&mut p, &plan, entry, 103);
    assert_eq!(p.nodes()["invoke"].started_at, Some(100));
    let child = node(&plan, "Copy selected input");
    let unchanged = canonical_hash(&p);
    assert!(matches!(
        p.claim(&plan, p.revision(), &child, 105),
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(canonical_hash(&p), unchanged);
    p.validate(&plan).unwrap();
}

#[test]
fn local_stop_blocks_the_body_but_preserves_an_independent_parent_branch() {
    let mut source = program();
    let body = &mut source.calls.get_mut("invoke").unwrap().body.workflow;
    let mut pending = body.controls["echo"].clone();
    pending.name = "Pending sibling".into();
    body.controls.insert("pending".into(), pending);
    let config = body.dataflow.as_ref().unwrap().steps["echo"].clone();
    body.dataflow
        .as_mut()
        .unwrap()
        .steps
        .insert("pending".into(), config);
    let independent: WorkflowControlStep = serde_json::from_value(
        json!({"name":"Independent parent","dependencies":[],"operation":{"kind":"compose"}}),
    )
    .unwrap();
    source
        .workflow
        .controls
        .insert("independent".into(), independent);
    let config=serde_json::from_value(json!({"inputs":{"value":{"value_type":{"type":"integer"},"value":{"source":"literal","value":42}}},"output":{"type":"object","fields":{"value":{"type":"integer"}}}})).unwrap();
    source
        .workflow
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .insert("independent".into(), config);
    let plan = compile(&source);
    let mut p = WorkflowProgress::new(&plan).unwrap();
    native(
        &mut p,
        &plan,
        &plan.composition_scopes["invoke"].entry_node,
        100,
    );
    let bad = node(&plan, "Copy selected input");
    let claim = p.claim(&plan, p.revision(), &bad, 100).unwrap();
    p.fail(&plan, p.revision(), &claim, "known_failure", 100)
        .unwrap();
    assert!(matches!(
        p.nodes()[&node(&plan, "Pending sibling")].state,
        WorkflowNodeState::Blocked
    ));
    native(&mut p, &plan, "independent", 100);
    assert!(matches!(
        p.nodes()["independent"].state,
        WorkflowNodeState::Completed { .. }
    ));
    assert_eq!(p.state(), WorkflowState::Failed);
    p.validate(&plan).unwrap();
}

#[test]
fn invalid_types_cross_scope_references_and_forged_scope_limits_are_rejected() {
    let mut bad = program();
    bad.workflow
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("invoke")
        .unwrap()
        .inputs
        .get_mut("amount")
        .unwrap()
        .value_type = WorkflowDataType::Text;
    assert!(WorkflowPlan::compile_composition(&bad, &scope(&bad), &[]).is_err());
    let mut bad = program();
    bad.calls
        .get_mut("invoke")
        .unwrap()
        .body
        .workflow
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("echo")
        .unwrap()
        .inputs
        .get_mut("amount")
        .unwrap()
        .value = WorkflowDataSource::Step {
        step_id: "after".into(),
        pointer: "/final".into(),
    };
    assert!(WorkflowPlan::compile_composition(&bad, &scope(&bad), &[]).is_err());
    let plan = compile(&program());
    let mut forged = serde_json::to_value(plan).unwrap();
    forged["composition_scopes"]["invoke"]["max_parallel"] = json!(32);
    forged["sha256"] = json!("");
    forged["sha256"] = json!(canonical_hash(&forged));
    assert!(
        serde_json::from_value::<WorkflowPlan>(forged)
            .unwrap()
            .validate()
            .is_err()
    );
}

fn model_program() -> (WorkflowComposition, ProviderProfile) {
    let mut source = program();
    let body = &mut source.calls.get_mut("invoke").unwrap().body.workflow;
    body.controls.clear();
    body.mode = WorkflowMode::Parallel { max_parallel: 2 };
    let budgets = body.budgets.clone();
    let rights = body.rights.clone();
    body.roles.push(WorkflowRole {
        id: "reader".into(),
        agent_config: None,
        model_binding: None,
        output: None,
        instructions: "Use declared inputs only".into(),
        provider_id: "local".into(),
        provider_revision: 1,
        rights: rights.clone(),
        budgets: budgets.clone(),
        delegate_to: BTreeSet::new(),
    });
    for id in ["echo", "second"] {
        body.steps.push(WorkflowStep {
            id: id.into(),
            execution: None,
            zero_context: false,
            role_id: "reader".into(),
            goal: format!("Model {id}"),
            dependencies: vec![],
            rights: rights.clone(),
            budgets: budgets.clone(),
        });
    }
    let config = body.dataflow.as_ref().unwrap().steps["echo"].clone();
    body.dataflow
        .as_mut()
        .unwrap()
        .steps
        .insert("second".into(), config);
    let profile = ProviderProfile {
        id: "local".into(),
        name: "Synthetic profile".into(),
        kind: ProviderKind::Ollama,
        endpoint: "http://127.0.0.1:11434".into(),
        model: "explicit".into(),
        revision: 1,
        generation: Default::default(),
        context_tokens: 2048,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    (source, profile)
}

#[test]
fn model_rights_and_simultaneous_reservations_share_the_call_budget() {
    let (mut source, profile) = model_program();
    source
        .calls
        .get_mut("invoke")
        .unwrap()
        .budgets
        .max_total_tokens = 2048;
    let plan = WorkflowPlan::compile_composition(&source, &scope(&source), &[profile]).unwrap();
    let mut p = WorkflowProgress::new(&plan).unwrap();
    native(
        &mut p,
        &plan,
        &plan.composition_scopes["invoke"].entry_node,
        100,
    );
    let first = node(&plan, "Model echo");
    let second = node(&plan, "Model second");
    assert_eq!(
        plan.nodes[&first].rights.tools,
        BTreeSet::from(["read".into()])
    );
    assert_eq!(
        plan.nodes[&first].rights.folders,
        BTreeSet::from(["project".into()])
    );
    let a = p.claim(&plan, p.revision(), &first, 100).unwrap();
    let b = p.claim(&plan, p.revision(), &second, 100).unwrap();
    let operation = WorkflowOperation::Model {
        request_sha256: "1".repeat(64),
        reserved_tokens: None,
    };
    assert!(matches!(
        p.admit(&plan, p.revision(), &a, "model-a", operation.clone(), 100)
            .unwrap(),
        WorkflowAdmission::Dispatch(_)
    ));
    let unchanged = canonical_hash(&p);
    assert!(matches!(
        p.admit(&plan, p.revision(), &b, "model-b", operation, 100),
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(canonical_hash(&p), unchanged);
    assert_eq!(p.charged_tokens(), 2048);
    p.validate(&plan).unwrap();
}

fn wrap(body: WorkflowComposition) -> WorkflowComposition {
    let mut workflow = body.workflow.clone();
    workflow.controls.clear();
    workflow.steps.clear();
    workflow.roles.clear();
    let data = workflow.dataflow.as_mut().unwrap();
    let output = data.input.clone();
    data.steps = BTreeMap::from([(
        "child".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: BTreeMap::from([(
                "amount".into(),
                WorkflowValueBinding {
                    value_type: WorkflowDataType::Integer,
                    value: WorkflowDataSource::Input {
                        pointer: "/amount".into(),
                    },
                },
            )]),
            output,
        },
    )]);
    let result = if body.calls.is_empty() {
        "echo"
    } else {
        "child"
    };
    let call = WorkflowSubflowCall {
        name: "Nested frozen call".into(),
        dependencies: vec![],
        result: WorkflowOutputRef {
            step_id: result.into(),
            pointer: "".into(),
        },
        rights: workflow.rights.clone(),
        budgets: workflow.budgets.clone(),
        body: Box::new(body),
    };
    WorkflowComposition {
        format_version: 1,
        workflow,
        calls: BTreeMap::from([("child".into(), call)]),
    }
}

#[test]
fn nested_frozen_calls_keep_parent_links_and_reject_excessive_depth() {
    let mut source = program();
    let mut body = *source.calls["invoke"].body.clone();
    for _ in 0..3 {
        body = wrap(body);
    }
    source.calls.get_mut("invoke").unwrap().result.step_id = "child".into();
    *source.calls.get_mut("invoke").unwrap().body = body.clone();
    let plan = compile(&source);
    let p = finish(&plan);
    assert_eq!(plan.composition_scopes.len(), 4);
    assert_eq!(p.state(), WorkflowState::Completed);
    let mut child = node(&plan, "Copy selected input");
    let mut parents = 0;
    while let Some(parent) = &plan.nodes[&child].parent {
        parents += 1;
        child = parent.clone();
    }
    assert_eq!(parents, 4);
    p.validate(&plan).unwrap();
    *source.calls.get_mut("invoke").unwrap().body = wrap(body);
    assert!(WorkflowPlan::compile_composition(&source, &scope(&source), &[]).is_err());
}

#[test]
fn a_timer_inside_a_subflow_keeps_its_original_scope_deadline_across_recovery() {
    let mut source = program();
    let body = &mut source.calls.get_mut("invoke").unwrap().body.workflow;
    body.controls
        .get_mut("echo")
        .unwrap()
        .dependencies
        .push("pause".into());
    body.controls.insert(
        "pause".into(),
        WorkflowControlStep {
            name: "Inner timer".into(),
            dependencies: vec![],
            operation: WorkflowControlOperation::Wait {
                seconds_input: "seconds".into(),
            },
        },
    );
    body.dataflow.as_mut().unwrap().steps.insert(
        "pause".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: BTreeMap::from([(
                "seconds".into(),
                WorkflowValueBinding {
                    value_type: WorkflowDataType::Integer,
                    value: WorkflowDataSource::Input {
                        pointer: "/amount".into(),
                    },
                },
            )]),
            output: wait_output_type(),
        },
    );
    let plan = compile(&source);
    plan.validate_wait_inputs(Some(&input())).unwrap();
    let mut too_long = input();
    too_long["amount"] = json!(19);
    assert!(plan.validate_wait_inputs(Some(&too_long)).is_err());
    let mut invalid = input();
    invalid["amount"] = json!(-1);
    assert!(plan.validate_wait_inputs(Some(&invalid)).is_err());
    let mut p = WorkflowProgress::new(&plan).unwrap();
    native(
        &mut p,
        &plan,
        &plan.composition_scopes["invoke"].entry_node,
        100,
    );
    let timer = node(&plan, "Inner timer");
    let claim = p.claim(&plan, p.revision(), &timer, 100).unwrap();
    p.complete_timer(
        &plan,
        p.revision(),
        &claim,
        "inner-pause",
        Some(&input()),
        100001,
    )
    .unwrap();
    assert_eq!(p.next_wake_at(), Some(108));
    assert!(p.can_park_timer(&plan).unwrap());
    p.recover(&plan, p.revision(), 101).unwrap();
    p.resume_settled(&plan, p.revision(), BTreeMap::new(), 102)
        .unwrap();
    assert_eq!(p.nodes()["invoke"].started_at, Some(100));
    assert!(!p.advance_timers(&plan, p.revision(), 107).unwrap());
    assert!(p.advance_timers(&plan, p.revision(), 108).unwrap());
    for _ in 0..16 {
        let ready = p.ready_nodes(&plan).unwrap();
        if ready.is_empty() {
            break;
        }
        native(&mut p, &plan, &ready[0], 108);
    }
    assert_eq!(p.state(), WorkflowState::Completed);
    assert_eq!(p.charged_tokens(), 0);
    p.validate(&plan).unwrap();
}

#[test]
fn typed_model_results_return_through_the_same_parent_ledger() {
    let (mut source, profile) = model_program();
    let body = &mut source.calls.get_mut("invoke").unwrap().body.workflow;
    body.steps.retain(|step| step.id == "echo");
    body.dataflow.as_mut().unwrap().steps.remove("second");
    let plan = WorkflowPlan::compile_composition(&source, &scope(&source), &[profile]).unwrap();
    let mut p = WorkflowProgress::new(&plan).unwrap();
    native(
        &mut p,
        &plan,
        &plan.composition_scopes["invoke"].entry_node,
        100,
    );
    let child = node(&plan, "Model echo");
    let claim = p.claim(&plan, p.revision(), &child, 100).unwrap();
    p.admit(
        &plan,
        p.revision(),
        &claim,
        "only-model",
        WorkflowOperation::Model {
            request_sha256: "2".repeat(64),
            reserved_tokens: None,
        },
        100,
    )
    .unwrap();
    p.settle(
        &plan,
        p.revision(),
        &claim,
        "only-model",
        WorkflowOutcome::Measured {
            input_tokens: 10,
            output_tokens: 20,
        },
        101,
    )
    .unwrap();
    p.complete(&plan, p.revision(), &claim, "{\"amount\":8}", 101)
        .unwrap();
    native(&mut p, &plan, "invoke", 101);
    native(&mut p, &plan, "after", 101);
    assert_eq!(p.state(), WorkflowState::Completed);
    assert_eq!(p.charged_tokens(), 30);
    assert_eq!(
        p.attempts()
            .values()
            .filter(|a| matches!(a.operation, WorkflowOperation::Model { .. }))
            .count(),
        1
    );
    let WorkflowNodeState::Completed { output, .. } = &p.nodes()["after"].state else {
        panic!()
    };
    assert_eq!(
        serde_json::from_str::<Value>(output).unwrap(),
        json!({"final":8})
    );
    p.validate(&plan).unwrap();
}
