use cowork_contracts::ProviderProfile;
use cowork_domain::{Error, canonical_hash, workflows::*};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn program() -> WorkflowIterationProgram {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-iteration-program.json"
    ))
    .unwrap()
}
fn scope(p: &WorkflowIterationProgram) -> WorkflowScope {
    WorkflowScope {
        workspace_folder: None,
        rights: p.workflow.rights.clone(),
        budgets: p.workflow.budgets.clone(),
        max_parallel: 4,
    }
}
fn plan(p: &WorkflowIterationProgram) -> WorkflowPlan {
    WorkflowPlan::compile_iterations(p, &scope(p), &[]).unwrap()
}
fn native(progress: &mut WorkflowProgress, plan: &WorkflowPlan, id: &str, input: &Value, at: i64) {
    let claim = progress.claim(plan, progress.revision(), id, at).unwrap();
    progress
        .complete_control(
            plan,
            progress.revision(),
            &claim,
            &format!("native-{}", progress.revision()),
            Some(input),
            at,
        )
        .unwrap();
}
fn finish(progress: &mut WorkflowProgress, plan: &WorkflowPlan, input: &Value, at: i64) {
    for _ in 0..100 {
        let ready = progress.ready_nodes(plan).unwrap();
        if ready.is_empty() {
            break;
        }
        native(progress, plan, ready.last().unwrap(), input, at);
    }
}
fn output(progress: &WorkflowProgress, node: &str) -> Value {
    let WorkflowNodeState::Completed { output, .. } = &progress.nodes()[node].state else {
        panic!("{}: {:?}", node, progress.nodes()[node].state)
    };
    serde_json::from_str(output).unwrap()
}
fn profile() -> ProviderProfile {
    serde_json::from_value(json!({"id":"model","name":"Fixture profile","kind":"ollama","endpoint":"http://127.0.0.1:11434","model":"frozen-model","revision":1,"context_tokens":2048,"credential_required":false,"credential_ready":true})).unwrap()
}

#[test]
fn expansion_is_one_revision_and_keeps_the_original_plan_hash_and_shared_ledger() {
    let source = program();
    let plan = plan(&source);
    plan.validate().unwrap();
    let serialized = serde_json::to_string(&plan).unwrap();
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let input = json!({"values":[7,-3,7]});
    assert_eq!(progress.nodes().len(), 3);
    assert!(progress.iteration_manifests().is_empty());
    let capture = plan.iterations["loop"].capture_node.clone();
    let claim = progress.claim(&plan, 0, &capture, 100).unwrap();
    let before = progress.revision();
    progress
        .complete_control(&plan, before, &claim, "capture", Some(&input), 100)
        .unwrap();
    assert_eq!(progress.revision(), before + 1);
    assert_eq!(progress.nodes().len(), 12);
    assert_eq!(progress.iteration_manifests()["loop"].items.len(), 3);
    assert!(progress.attempts().is_empty());
    assert_eq!(serialized, serde_json::to_string(&plan).unwrap());
    let reopened: WorkflowProgress =
        serde_json::from_slice(&serde_json::to_vec(&progress).unwrap()).unwrap();
    reopened.validate(&plan).unwrap();
    progress = reopened;
    finish(&mut progress, &plan, &input, 101);
    assert_eq!(progress.state(), WorkflowState::Completed);
    assert_eq!(output(&progress, "after"), json!({"results":[7,-3,7]}));
    assert_eq!(progress.attempts().len(), 8);
    assert_eq!(progress.charged_tokens(), 0);
    progress.validate(&plan).unwrap();
    assert_eq!(serialized, serde_json::to_string(&plan).unwrap());
}

#[test]
fn an_empty_list_completes_without_creating_body_instances_or_model_attempts() {
    let plan = plan(&program());
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let input = json!({"values":[]});
    finish(&mut progress, &plan, &input, 100);
    assert_eq!(progress.state(), WorkflowState::Completed);
    assert_eq!(progress.nodes().len(), 3);
    assert_eq!(output(&progress, "after"), json!({"results":[]}));
    assert_eq!(progress.attempts().len(), 2);
}

#[test]
fn parallelism_counts_active_items_including_their_pending_body_work() {
    let plan = plan(&program());
    let input = json!({"values":[1,2,3]});
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    native(
        &mut progress,
        &plan,
        &plan.iterations["loop"].capture_node,
        &input,
        100,
    );
    let graph = progress.execution_graph(&plan).unwrap();
    let instances = &graph.iteration_instances["loop"];
    let first = progress
        .claim(&plan, progress.revision(), &instances[0].entry, 100)
        .unwrap();
    let second = progress
        .claim(&plan, progress.revision(), &instances[1].entry, 100)
        .unwrap();
    assert!(matches!(
        progress.claim(&plan, progress.revision(), &instances[2].entry, 100),
        Err(Error::WorkspaceBusy)
    ));
    progress
        .complete_control(
            &plan,
            progress.revision(),
            &first,
            "arguments-1",
            Some(&input),
            100,
        )
        .unwrap();
    progress
        .complete_control(
            &plan,
            progress.revision(),
            &second,
            "arguments-2",
            Some(&input),
            100,
        )
        .unwrap();
    assert!(
        !progress
            .ready_nodes(&plan)
            .unwrap()
            .contains(&instances[2].entry)
    );
    assert!(matches!(
        progress.claim(&plan, progress.revision(), &instances[2].entry, 100),
        Err(Error::WorkspaceBusy)
    ));
    drop(graph);
    finish(&mut progress, &plan, &input, 101);
    assert_eq!(output(&progress, "loop"), json!([1, 2, 3]));
}

#[test]
fn replay_uses_the_original_producer_result_instead_of_an_edited_rehashed_manifest() {
    let mut source = program();
    let budgets = source.workflow.budgets.clone();
    let profile = profile();
    source.workflow.dataflow.as_mut().unwrap().input = WorkflowDataType::Object {
        fields: BTreeMap::new(),
    };
    source.workflow.roles.push(serde_json::from_value(json!({"id":"producer","instructions":"Produce a typed list","provider_id":"model","provider_revision":1,"rights":{"tools":[],"folders":[]},"budgets":budgets,"delegate_to":[]})).unwrap());
    source.workflow.steps.push(WorkflowStep {
        id: "produce".into(),
        role_id: "producer".into(),
        goal: "Produce the input list".into(),
        dependencies: vec![],
        rights: WorkflowRights::default(),
        budgets: budgets.clone(),
        execution: None,
        zero_context: false,
    });
    let list = WorkflowDataType::List {
        item: Box::new(WorkflowDataType::Integer),
        max_items: 1000,
    };
    source.workflow.dataflow.as_mut().unwrap().steps.insert(
        "produce".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: BTreeMap::new(),
            output: WorkflowDataType::Object {
                fields: BTreeMap::from([("items".into(), list)]),
            },
        },
    );
    let iteration = source.iterations.get_mut("loop").unwrap();
    iteration.dependencies = vec!["produce".into()];
    iteration.inputs.get_mut("items").unwrap().value = WorkflowDataSource::Step {
        step_id: "produce".into(),
        pointer: "/items".into(),
    };
    let plan = WorkflowPlan::compile_iterations(&source, &scope(&source), &[profile]).unwrap();
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    assert_eq!(progress.ready_nodes(&plan).unwrap(), vec!["produce"]);
    let claim = progress.claim(&plan, 0, "produce", 100).unwrap();
    progress
        .admit(
            &plan,
            progress.revision(),
            &claim,
            "model-send",
            WorkflowOperation::Model {
                request_sha256: "a".repeat(64),
                reserved_tokens: None,
            },
            100,
        )
        .unwrap();
    progress
        .settle(
            &plan,
            progress.revision(),
            &claim,
            "model-send",
            WorkflowOutcome::Measured {
                input_tokens: 10,
                output_tokens: 20,
            },
            100,
        )
        .unwrap();
    progress
        .complete(
            &plan,
            progress.revision(),
            &claim,
            "{\"items\":[5,2,5]}",
            100,
        )
        .unwrap();
    native(
        &mut progress,
        &plan,
        &plan.iterations["loop"].capture_node,
        &json!({}),
        101,
    );
    let mut edited = serde_json::to_value(&progress).unwrap();
    let manifest = WorkflowIterationManifest::freeze(
        &source.iterations["loop"].source,
        "loop",
        &json!({"items":[9,9,9]}),
    )
    .unwrap();
    edited["iterations"]["loop"] = serde_json::to_value(manifest).unwrap();
    assert!(
        serde_json::from_value::<WorkflowProgress>(edited)
            .unwrap()
            .validate(&plan)
            .is_err()
    );
    finish(&mut progress, &plan, &json!({}), 101);
    assert_eq!(output(&progress, "loop"), json!([5, 2, 5]));
    assert_eq!(progress.charged_tokens(), 30);
    assert_eq!(
        progress
            .attempts()
            .values()
            .filter(|a| matches!(a.operation, WorkflowOperation::Model { .. }))
            .count(),
        1
    );
}

#[test]
fn a_waiting_item_holds_its_item_slot_but_releases_the_worker_and_keeps_its_wake() {
    let mut source = program();
    source
        .iterations
        .get_mut("loop")
        .unwrap()
        .source
        .max_parallel = 1;
    let body = &mut source.iterations.get_mut("loop").unwrap().source.body;
    body.controls.get_mut("echo").unwrap().operation = WorkflowControlOperation::Wait {
        seconds_input: "amount".into(),
    };
    body.dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("echo")
        .unwrap()
        .output = wait_output_type();
    source
        .iterations
        .get_mut("loop")
        .unwrap()
        .source
        .result
        .pointer = "/wake_at".into();
    let plan = plan(&source);
    let input = json!({"values":[2,3]});
    plan.validate_wait_inputs(Some(&input)).unwrap();
    assert!(
        plan.validate_wait_inputs(Some(&json!({"values":[2,0]})))
            .is_err()
    );
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    native(
        &mut progress,
        &plan,
        &plan.iterations["loop"].capture_node,
        &input,
        100,
    );
    let graph = progress.execution_graph(&plan).unwrap();
    let instances = &graph.iteration_instances["loop"];
    native(&mut progress, &plan, &instances[0].entry, &input, 100);
    let wait = graph
        .definition
        .controls
        .iter()
        .find(|(id, step)| {
            matches!(step.operation, WorkflowControlOperation::Wait { .. })
                && graph.nodes[*id].parent.as_deref() == Some(&instances[0].exit)
        })
        .unwrap()
        .0
        .clone();
    let claim = progress
        .claim(&plan, progress.revision(), &wait, 100)
        .unwrap();
    progress
        .complete_timer(
            &plan,
            progress.revision(),
            &claim,
            "timer-1",
            Some(&input),
            100_000,
        )
        .unwrap();
    assert_eq!(progress.next_wake_at(), Some(102));
    assert!(progress.can_park_timer(&plan).unwrap());
    assert!(
        !progress
            .ready_nodes(&plan)
            .unwrap()
            .contains(&instances[1].entry)
    );
    let serialized = serde_json::to_vec(&progress).unwrap();
    let mut reopened: WorkflowProgress = serde_json::from_slice(&serialized).unwrap();
    reopened.validate(&plan).unwrap();
    assert_eq!(reopened.next_wake_at(), Some(102));
    assert!(
        !reopened
            .advance_timers(&plan, reopened.revision(), 101)
            .unwrap()
    );
    assert!(
        reopened
            .advance_timers(&plan, reopened.revision(), 102)
            .unwrap()
    );
    native(&mut reopened, &plan, &instances[0].exit, &input, 102);
    assert!(
        reopened
            .ready_nodes(&plan)
            .unwrap()
            .contains(&instances[1].entry)
    );
}

#[test]
fn model_roles_are_frozen_once_and_uncertain_item_io_cannot_be_replayed() {
    let mut source = program();
    let iteration = source.iterations.get_mut("loop").unwrap();
    let body = &mut iteration.source.body;
    body.controls.clear();
    let budgets = body.budgets.clone();
    body.roles.push(serde_json::from_value(json!({"id":"worker","instructions":"Use only the bound amount","provider_id":"model","provider_revision":1,"rights":{"tools":[],"folders":[]},"budgets":budgets,"delegate_to":[]})).unwrap());
    body.steps.push(WorkflowStep {
        id: "echo".into(),
        role_id: "worker".into(),
        goal: "Process one amount".into(),
        dependencies: vec![],
        rights: WorkflowRights::default(),
        budgets,
        execution: None,
        zero_context: false,
    });
    let profile = profile();
    let plan = WorkflowPlan::compile_iterations(&source, &scope(&source), &[profile]).unwrap();
    assert_eq!(plan.roles.len(), 1);
    assert_eq!(plan.profiles.len(), 1);
    let input = json!({"values":[2,4,6]});
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    native(
        &mut progress,
        &plan,
        &plan.iterations["loop"].capture_node,
        &input,
        100,
    );
    let graph = progress.execution_graph(&plan).unwrap();
    let items = &graph.iteration_instances["loop"];
    let models: Vec<_> = graph
        .nodes
        .values()
        .filter(|node| node.role_id.is_some())
        .collect();
    assert_eq!(models.len(), 3);
    assert!(models.iter().all(|node| node.role_id == models[0].role_id));
    native(&mut progress, &plan, &items[0].entry, &input, 100);
    let node = models
        .iter()
        .find(|node| node.parent.as_deref() == Some(&items[0].exit))
        .unwrap();
    let claim = progress
        .claim(&plan, progress.revision(), &node.id, 100)
        .unwrap();
    let operation = WorkflowOperation::Model {
        request_sha256: "a".repeat(64),
        reserved_tokens: None,
    };
    progress
        .admit(
            &plan,
            progress.revision(),
            &claim,
            "uncertain-send",
            operation.clone(),
            100,
        )
        .unwrap();
    progress
        .settle(
            &plan,
            progress.revision(),
            &claim,
            "uncertain-send",
            WorkflowOutcome::Uncertain,
            101,
        )
        .unwrap();
    assert_eq!(progress.state(), WorkflowState::Interrupted);
    assert!(progress.ready_nodes(&plan).unwrap().is_empty());
    let reopened: WorkflowProgress =
        serde_json::from_slice(&serde_json::to_vec(&progress).unwrap()).unwrap();
    let mut reopened = reopened;
    reopened.validate(&plan).unwrap();
    assert_eq!(reopened.charged_tokens(), 2048);
    assert!(matches!(
        reopened
            .admit(
                &plan,
                reopened.revision(),
                &claim,
                "uncertain-send",
                operation,
                102
            )
            .unwrap(),
        WorkflowAdmission::Existing(_)
    ));
    assert!(
        reopened
            .resume_settled(&plan, reopened.revision(), BTreeMap::new(), 102)
            .is_err()
    );
    assert_eq!(reopened.attempts().len(), 1);
}

#[test]
fn root_and_loop_budgets_do_not_multiply_with_item_count() {
    let mut source = program();
    source
        .iterations
        .get_mut("loop")
        .unwrap()
        .source
        .budgets
        .max_steps = 3;
    let plan = plan(&source);
    let input = json!({"values":[1,2,3,4]});
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let mut denied = false;
    for _ in 0..50 {
        let ready = progress.ready_nodes(&plan).unwrap();
        if ready.is_empty() {
            break;
        }
        let node = ready.last().unwrap();
        let claim = progress
            .claim(&plan, progress.revision(), node, 100)
            .unwrap();
        match progress.complete_control(
            &plan,
            progress.revision(),
            &claim,
            &format!("control-{}", progress.revision()),
            Some(&input),
            100,
        ) {
            Ok(()) => {}
            Err(Error::BudgetExceeded) => {
                denied = true;
                progress
                    .fail(&plan, progress.revision(), &claim, "budget_exceeded", 100)
                    .unwrap();
                break;
            }
            Err(error) => panic!("{error:?}"),
        }
    }
    assert!(denied);
    assert_eq!(progress.attempts().len(), 3);
    assert_eq!(progress.state(), WorkflowState::Failed);
    assert!(plan.budgets.max_steps > 3);
    progress.validate(&plan).unwrap();
}

#[test]
fn an_invalid_known_body_wait_rolls_back_capture_and_all_proposed_nodes_atomically() {
    let mut source = program();
    let body = &mut source.iterations.get_mut("loop").unwrap().source.body;
    body.controls.get_mut("echo").unwrap().operation = WorkflowControlOperation::Wait {
        seconds_input: "amount".into(),
    };
    body.dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("echo")
        .unwrap()
        .output = wait_output_type();
    source
        .iterations
        .get_mut("loop")
        .unwrap()
        .source
        .result
        .pointer = "/wake_at".into();
    let plan = plan(&source);
    let input = json!({"values":[2,0,3]});
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let claim = progress
        .claim(&plan, 0, &plan.iterations["loop"].capture_node, 100)
        .unwrap();
    let before = canonical_hash(&progress);
    assert!(
        progress
            .complete_control(
                &plan,
                progress.revision(),
                &claim,
                "invalid-batch",
                Some(&input),
                100
            )
            .is_err()
    );
    assert_eq!(canonical_hash(&progress), before);
    assert_eq!(progress.nodes().len(), 3);
    assert!(progress.iteration_manifests().is_empty());
    assert!(progress.attempts().is_empty());
}

#[test]
fn altered_dynamic_nodes_and_template_profiles_do_not_validate_after_rehashing() {
    let plan = plan(&program());
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    native(
        &mut progress,
        &plan,
        &plan.iterations["loop"].capture_node,
        &json!({"values":[2,3]}),
        100,
    );
    let graph = progress.execution_graph(&plan).unwrap();
    let entry = &graph.iteration_instances["loop"][0].entry;
    let mut value = serde_json::to_value(&progress).unwrap();
    value["nodes"][entry]["plan"]["budgets"]["max_steps"] = json!(4096);
    assert!(
        serde_json::from_value::<WorkflowProgress>(value)
            .unwrap()
            .validate(&plan)
            .is_err()
    );
    let mut value = serde_json::to_value(&plan).unwrap();
    value["iterations"]["loop"]["source"]["max_parallel"] = json!(32);
    value["sha256"] = json!("");
    value["sha256"] = json!(canonical_hash(&value));
    assert!(
        serde_json::from_value::<WorkflowPlan>(value)
            .unwrap()
            .validate()
            .is_err()
    );
}

#[test]
fn a_false_loop_gate_skips_the_capture_and_return_without_materializing_items() {
    let mut source = program();
    source.workflow.controls.insert(
        "condition".into(),
        WorkflowControlStep {
            name: "Select loop".into(),
            dependencies: vec![],
            operation: WorkflowControlOperation::Condition {
                predicate: WorkflowPredicate::IsTrue {
                    input: "enabled".into(),
                },
            },
        },
    );
    source.workflow.dataflow.as_mut().unwrap().steps.insert(
        "condition".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: BTreeMap::from([(
                "enabled".into(),
                WorkflowValueBinding {
                    value_type: WorkflowDataType::Boolean,
                    value: WorkflowDataSource::Literal {
                        value: json!(false),
                    },
                },
            )]),
            output: WorkflowDataType::Boolean,
        },
    );
    source.iterations.get_mut("loop").unwrap().dependencies = vec!["condition".into()];
    source.workflow.gates.insert(
        "loop".into(),
        WorkflowGate {
            step_id: "condition".into(),
            when: true,
        },
    );
    let plan = plan(&source);
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    finish(&mut progress, &plan, &json!({"values":[1,2,3]}), 100);
    assert_eq!(progress.state(), WorkflowState::Completed);
    assert!(progress.iteration_manifests().is_empty());
    assert!(matches!(
        progress.nodes()["loop"].state,
        WorkflowNodeState::Skipped { .. }
    ));
    assert!(matches!(
        progress.nodes()["after"].state,
        WorkflowNodeState::Skipped { .. }
    ));
    assert_eq!(progress.attempts().len(), 1);
}

#[test]
fn chained_loops_keep_both_captures_and_the_original_output_order_on_replay() {
    let mut source = program();
    let mut second = source.iterations["loop"].clone();
    second.dependencies = vec!["loop".into()];
    second.inputs.get_mut("items").unwrap().value = WorkflowDataSource::Step {
        step_id: "loop".into(),
        pointer: String::new(),
    };
    // Sort the later capture first to exercise reconstruction independently of map order.
    source.iterations.insert("again".into(), second);
    source
        .workflow
        .controls
        .get_mut("after")
        .unwrap()
        .dependencies = vec!["again".into()];
    source
        .workflow
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("after")
        .unwrap()
        .inputs
        .get_mut("results")
        .unwrap()
        .value = WorkflowDataSource::Step {
        step_id: "again".into(),
        pointer: String::new(),
    };
    let plan = plan(&source);
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    finish(&mut progress, &plan, &json!({"values":[7,-3,7]}), 100);
    assert_eq!(progress.state(), WorkflowState::Completed);
    assert_eq!(progress.iteration_manifests().len(), 2);
    assert_eq!(output(&progress, "after"), json!({"results":[7,-3,7]}));
    let reopened: WorkflowProgress =
        serde_json::from_slice(&serde_json::to_vec(&progress).unwrap()).unwrap();
    reopened.validate(&plan).unwrap();
    assert_eq!(canonical_hash(&reopened), canonical_hash(&progress));
}

#[test]
fn a_skipped_item_result_cannot_be_silently_removed_from_the_collected_list() {
    let mut source = program();
    let body = &mut source.iterations.get_mut("loop").unwrap().source.body;
    body.controls.insert(
        "choose".into(),
        serde_json::from_value(json!({
            "name":"Skip the selected body result","dependencies":[],
            "operation":{"kind":"condition","predicate":{"operator":"is_true","input":"flag"}}
        }))
        .unwrap(),
    );
    body.dataflow.as_mut().unwrap().steps.insert("choose".into(),serde_json::from_value(json!({
        "inputs":{"flag":{"value_type":{"type":"boolean"},"value":{"source":"literal","value":false}}},"output":{"type":"boolean"}
    })).unwrap());
    body.controls.get_mut("echo").unwrap().dependencies = vec!["choose".into()];
    body.gates.insert(
        "echo".into(),
        WorkflowGate {
            step_id: "choose".into(),
            when: true,
        },
    );
    let plan = plan(&source);
    let input = json!({"values":[7]});
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    loop {
        let ready = progress.ready_nodes(&plan).unwrap();
        if ready == vec!["loop"] {
            break;
        }
        assert!(!ready.is_empty());
        native(&mut progress, &plan, &ready[0], &input, 100);
    }
    let claim = progress
        .claim(&plan, progress.revision(), "loop", 100)
        .unwrap();
    let before = canonical_hash(&progress);
    assert!(
        progress
            .complete_control(
                &plan,
                progress.revision(),
                &claim,
                "collect",
                Some(&input),
                100
            )
            .is_err()
    );
    assert_eq!(before, canonical_hash(&progress));
    progress
        .fail(
            &plan,
            progress.revision(),
            &claim,
            "workflow_iteration_result_missing",
            100,
        )
        .unwrap();
    assert_eq!(progress.state(), WorkflowState::Failed);
    progress.validate(&plan).unwrap();
}

#[test]
fn node_limit_rejection_rolls_back_the_whole_capture() {
    let mut source = program();
    let body = &mut source.iterations.get_mut("loop").unwrap().source.body;
    let control = body.controls["echo"].clone();
    let data = body.dataflow.as_ref().unwrap().steps["echo"].clone();
    for id in ["second", "third"] {
        body.controls.insert(id.into(), control.clone());
        body.dataflow
            .as_mut()
            .unwrap()
            .steps
            .insert(id.into(), data.clone());
    }
    let plan = plan(&source);
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let capture = &plan.iterations["loop"].capture_node;
    let claim = progress.claim(&plan, 0, capture, 100).unwrap();
    let before = canonical_hash(&progress);
    let input = json!({"values":vec![1;1000]});
    assert!(matches!(
        progress.complete_control(
            &plan,
            progress.revision(),
            &claim,
            "capture",
            Some(&input),
            100
        ),
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(canonical_hash(&progress), before);
    assert!(progress.iteration_manifests().is_empty());
    assert_eq!(progress.nodes().len(), 3);
}

#[test]
fn a_thousand_instances_share_the_plan_and_have_bounded_replayable_graph_metadata() {
    let source = program();
    let plan = plan(&source);
    let frozen = serde_json::to_vec(&plan).unwrap();
    let input = json!({"values":vec![7;1000]});
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let start = std::time::Instant::now();
    native(
        &mut progress,
        &plan,
        &plan.iterations["loop"].capture_node,
        &input,
        100,
    );
    let reopened: WorkflowProgress =
        serde_json::from_slice(&serde_json::to_vec(&progress).unwrap()).unwrap();
    let graph = reopened.execution_graph(&plan).unwrap();
    assert_eq!(graph.iteration_instances["loop"].len(), 1000);
    assert_eq!(graph.nodes.len(), 3003);
    assert_eq!(reopened.nodes().len(), 3003);
    assert!(reopened.attempts().is_empty());
    assert_eq!(serde_json::to_vec(&plan).unwrap(), frozen);
    eprintln!(
        "ITERATION_KERNEL_1000_CAPTURE_REOPEN_MS={}",
        start.elapsed().as_millis()
    );
    // This qualifies capture/reopen, not 1000 model requests or completed items.
}

#[test]
fn expanded_template_bytes_and_oversized_argument_frames_are_rejected_before_item_io() {
    for (count, padding, budget_error) in [(1000, 9000, true), (1, 16_384, false)] {
        let mut source = program();
        let iteration = &mut source.iterations.get_mut("loop").unwrap().source;
        let WorkflowDataType::Object { fields } =
            &mut iteration.body.dataflow.as_mut().unwrap().input
        else {
            panic!()
        };
        fields.insert("padding".into(), WorkflowDataType::Text);
        iteration.arguments.insert(
            "padding".into(),
            WorkflowIterationArgument::Literal {
                value: json!("🧪".repeat(padding)),
            },
        );
        let plan = plan(&source);
        let mut progress = WorkflowProgress::new(&plan).unwrap();
        let claim = progress
            .claim(&plan, 0, &plan.iterations["loop"].capture_node, 100)
            .unwrap();
        let before = canonical_hash(&progress);
        let error = progress
            .complete_control(
                &plan,
                progress.revision(),
                &claim,
                "capture",
                Some(&json!({"values":vec![7;count]})),
                100,
            )
            .unwrap_err();
        assert!(
            if budget_error {
                matches!(error, Error::BudgetExceeded)
            } else {
                matches!(error, Error::ContextLimit)
            },
            "{error:?}"
        );
        assert_eq!(before, canonical_hash(&progress));
        assert!(progress.iteration_manifests().is_empty());
    }
}

#[test]
fn shared_argument_text_is_referenced_from_one_capture_in_the_derived_graph() {
    let mut source = program();
    let WorkflowDataType::Object { fields } = &mut source.workflow.dataflow.as_mut().unwrap().input
    else {
        panic!()
    };
    fields.insert("context".into(), WorkflowDataType::Text);
    let iteration = source.iterations.get_mut("loop").unwrap();
    let WorkflowDataType::Object { fields } = &mut iteration.source.input else {
        panic!()
    };
    fields.insert("context".into(), WorkflowDataType::Text);
    iteration.inputs.insert(
        "context".into(),
        WorkflowValueBinding {
            value_type: WorkflowDataType::Text,
            value: WorkflowDataSource::Input {
                pointer: "/context".into(),
            },
        },
    );
    let WorkflowDataType::Object { fields } =
        &mut iteration.source.body.dataflow.as_mut().unwrap().input
    else {
        panic!()
    };
    fields.insert("context".into(), WorkflowDataType::Text);
    iteration.source.arguments.insert(
        "context".into(),
        WorkflowIterationArgument::Input {
            pointer: "/context".into(),
        },
    );
    let plan = plan(&source);
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let marker = "SHARED_CONTEXT_".repeat(512);
    let input = json!({"values":vec![7;20],"context":marker});
    native(
        &mut progress,
        &plan,
        &plan.iterations["loop"].capture_node,
        &input,
        100,
    );
    let graph = progress.execution_graph(&plan).unwrap();
    assert!(
        !serde_json::to_string(graph.definition.as_ref())
            .unwrap()
            .contains(&marker)
    );
    for item in &graph.iteration_instances["loop"] {
        assert!(
            matches!(&graph.definition.dataflow.as_ref().unwrap().steps[&item.entry].inputs["context"].value,
            WorkflowDataSource::Step{step_id,pointer} if step_id==&plan.iterations["loop"].capture_node&&pointer=="/context")
        );
    }
    assert_eq!(
        progress.iteration_manifests()["loop"].input["context"],
        input["context"]
    );
}

#[test]
fn a_nested_frozen_subflow_uses_each_item_frame_and_the_same_parent_ledger() {
    let mut source = program();
    let mut body: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-subflow.json"
    ))
    .unwrap();
    let WorkflowDataType::Object { fields } = &mut body.dataflow.as_mut().unwrap().input else {
        panic!()
    };
    fields.remove("private_note");
    let iteration = &mut source.iterations.get_mut("loop").unwrap().source;
    *iteration.body = body;
    iteration.result = WorkflowOutputRef {
        step_id: "after".into(),
        pointer: "/final".into(),
    };
    let plan = plan(&source);
    let frozen = canonical_hash(&plan);
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    finish(&mut progress, &plan, &json!({"values":[7,-3]}), 100);
    assert_eq!(progress.state(), WorkflowState::Completed);
    assert_eq!(output(&progress, "after"), json!({"results":[7,-3]}));
    assert_eq!(progress.attempts().len(), 10);
    assert_eq!(progress.charged_tokens(), 0);
    assert_eq!(
        progress
            .execution_graph(&plan)
            .unwrap()
            .composition_scopes
            .len(),
        5
    );
    let reopened: WorkflowProgress =
        serde_json::from_slice(&serde_json::to_vec(&progress).unwrap()).unwrap();
    reopened.validate(&plan).unwrap();
    assert_eq!(frozen, canonical_hash(&plan));
}

#[test]
fn a_later_item_cannot_restart_the_loop_deadline_after_a_wait() {
    let mut source = program();
    let iteration = &mut source.iterations.get_mut("loop").unwrap().source;
    iteration.max_parallel = 1;
    iteration.budgets.max_seconds = 5;
    iteration.body.controls.get_mut("echo").unwrap().operation = WorkflowControlOperation::Wait {
        seconds_input: "amount".into(),
    };
    iteration
        .body
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("echo")
        .unwrap()
        .output = wait_output_type();
    iteration.result.pointer = "/wake_at".into();
    let plan = plan(&source);
    let input = json!({"values":[2,3]});
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    native(
        &mut progress,
        &plan,
        &plan.iterations["loop"].capture_node,
        &input,
        100,
    );
    let graph = progress.execution_graph(&plan).unwrap();
    let items = &graph.iteration_instances["loop"];
    let wait = |index: usize| {
        graph
            .nodes
            .values()
            .find(|node| {
                node.parent.as_deref() == Some(&items[index].exit)
                    && matches!(
                        graph
                            .definition
                            .controls
                            .get(&node.id)
                            .map(|step| &step.operation),
                        Some(WorkflowControlOperation::Wait { .. })
                    )
            })
            .unwrap()
            .id
            .clone()
    };
    native(&mut progress, &plan, &items[0].entry, &input, 100);
    let first = progress
        .claim(&plan, progress.revision(), &wait(0), 100)
        .unwrap();
    progress
        .complete_timer(
            &plan,
            progress.revision(),
            &first,
            "first-wait",
            Some(&input),
            100_000,
        )
        .unwrap();
    progress
        .advance_timers(&plan, progress.revision(), 102)
        .unwrap();
    native(&mut progress, &plan, &items[0].exit, &input, 102);
    native(&mut progress, &plan, &items[1].entry, &input, 102);
    let second = progress
        .claim(&plan, progress.revision(), &wait(1), 102)
        .unwrap();
    let before = canonical_hash(&progress);
    assert!(matches!(
        progress.complete_timer(
            &plan,
            progress.revision(),
            &second,
            "second-wait",
            Some(&input),
            102_000
        ),
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(before, canonical_hash(&progress));
    assert_eq!(progress.nodes()["loop"].started_at, Some(100));
    assert!(progress.next_wake_at().is_none());
    let reopened: WorkflowProgress =
        serde_json::from_slice(&serde_json::to_vec(&progress).unwrap()).unwrap();
    reopened.validate(&plan).unwrap();
}
