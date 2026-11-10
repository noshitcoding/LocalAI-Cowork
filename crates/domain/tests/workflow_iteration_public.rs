use cowork_domain::{canonical_hash, workflows::*};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn definition() -> WorkflowDefinition {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-iteration.json"
    ))
    .unwrap()
}
fn scope(definition: &WorkflowDefinition) -> WorkflowScope {
    WorkflowScope {
        workspace_folder: None,
        rights: definition.rights.clone(),
        budgets: definition.budgets.clone(),
        max_parallel: 4,
    }
}
fn finish(plan: &WorkflowPlan, input: Value) -> WorkflowProgress {
    let mut p = WorkflowProgress::new(plan).unwrap();
    for _ in 0..100 {
        let ready = p.ready_nodes(plan).unwrap();
        let Some(node) = ready.last() else { break };
        let claim = p.claim(plan, p.revision(), node, 100).unwrap();
        p.complete_control(
            plan,
            p.revision(),
            &claim,
            &format!("native-{}", p.revision()),
            Some(&input),
            100,
        )
        .unwrap();
    }
    p.validate(plan).unwrap();
    p
}
fn output(p: &WorkflowProgress, id: &str) -> Value {
    let WorkflowNodeState::Completed { output, .. } = &p.nodes()[id].state else {
        panic!("{p:?}")
    };
    serde_json::from_str(output).unwrap()
}

#[test]
fn published_source_roundtrips_and_binds_the_original_definition_and_frozen_body() {
    let mut definition = definition();
    definition.validate_definition().unwrap();
    let source = definition.iteration_source().unwrap();
    assert_eq!(
        canonical_hash(&definition),
        canonical_hash(&source.source_definition().unwrap())
    );
    let plan = WorkflowPlan::compile(&definition, &scope(&definition), &[]).unwrap();
    assert!(plan.iteration_definition);
    assert_eq!(plan.source_sha256, canonical_hash(&definition));
    let frozen = serde_json::to_vec(&plan).unwrap();
    let p = finish(&plan, json!({"values":[7,-3,7]}));
    assert_eq!(p.state(), WorkflowState::Completed);
    assert_eq!(output(&p, "after"), json!({"results":[7,-3,7]}));
    definition
        .iterations
        .get_mut("loop")
        .unwrap()
        .definition
        .name = "Later template change".into();
    assert_ne!(plan.source_sha256, canonical_hash(&definition));
    assert_eq!(frozen, serde_json::to_vec(&plan).unwrap());
    let reopened: WorkflowPlan = serde_json::from_slice(&frozen).unwrap();
    reopened.validate().unwrap();
}

#[test]
fn empty_public_lists_have_no_body_instances() {
    let definition = definition();
    let plan = WorkflowPlan::compile(&definition, &scope(&definition), &[]).unwrap();
    let p = finish(&plan, json!({"values":[]}));
    assert_eq!(p.state(), WorkflowState::Completed);
    assert_eq!(p.nodes().len(), 3);
    assert_eq!(p.attempts().len(), 2);
    assert_eq!(output(&p, "after"), json!({"results":[]}));
}

#[test]
fn public_entries_start_in_input_order_under_the_configured_parallel_limit() {
    let definition = definition();
    let plan = WorkflowPlan::compile(&definition, &scope(&definition), &[]).unwrap();
    let mut p = WorkflowProgress::new(&plan).unwrap();
    let capture = plan.iterations["loop"].capture_node.clone();
    let claim = p.claim(&plan, 0, &capture, 100).unwrap();
    p.complete_control(
        &plan,
        p.revision(),
        &claim,
        "capture",
        Some(&json!({"values":[7,-3,7]})),
        100,
    )
    .unwrap();
    let graph = p.execution_graph(&plan).unwrap();
    let entries = &graph.iteration_instances["loop"];
    assert_eq!(
        p.ready_nodes(&plan).unwrap(),
        vec![entries[0].entry.clone()]
    );
    assert!(
        p.claim(&plan, p.revision(), &entries[1].entry, 100)
            .is_err()
    );
    p.claim(&plan, p.revision(), &entries[0].entry, 100)
        .unwrap();
    p.claim(&plan, p.revision(), &entries[1].entry, 100)
        .unwrap();
    assert!(
        p.claim(&plan, p.revision(), &entries[2].entry, 100)
            .is_err()
    );
    p.validate(&plan).unwrap();
}

#[test]
fn root_subflows_and_lists_share_one_typed_plan_and_ledger() {
    let mut definition = definition();
    let list = WorkflowDataType::List {
        item: Box::new(WorkflowDataType::Integer),
        max_items: 1000,
    };
    let mut body = definition.iterations["loop"].definition.as_ref().clone();
    let input = WorkflowDataType::Object {
        fields: BTreeMap::from([("results".into(), list.clone())]),
    };
    body.dataflow.as_mut().unwrap().input = input.clone();
    let data = body
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("echo")
        .unwrap();
    data.inputs = BTreeMap::from([(
        "results".into(),
        WorkflowValueBinding {
            value_type: list.clone(),
            value: WorkflowDataSource::Input {
                pointer: "/results".into(),
            },
        },
    )]);
    data.output = input;
    definition.subflows.insert(
        "consume".into(),
        WorkflowEmbeddedSubflow {
            name: "Use the ordered results".into(),
            dependencies: vec!["loop".into()],
            definition: Box::new(body),
            result: WorkflowOutputRef {
                step_id: "echo".into(),
                pointer: "/results".into(),
            },
            rights: WorkflowRights::default(),
            budgets: definition.budgets.clone(),
        },
    );
    definition.dataflow.as_mut().unwrap().steps.insert(
        "consume".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: BTreeMap::from([(
                "results".into(),
                WorkflowValueBinding {
                    value_type: list.clone(),
                    value: WorkflowDataSource::Step {
                        step_id: "loop".into(),
                        pointer: String::new(),
                    },
                },
            )]),
            output: list,
        },
    );
    definition.controls.get_mut("after").unwrap().dependencies = vec!["consume".into()];
    definition
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
        step_id: "consume".into(),
        pointer: String::new(),
    };
    definition.validate_definition().unwrap();
    let plan = WorkflowPlan::compile(&definition, &scope(&definition), &[]).unwrap();
    assert!(plan.composition.is_some());
    assert!(plan.iteration_definition);
    let p = finish(&plan, json!({"values":[7,-3,7]}));
    assert_eq!(p.state(), WorkflowState::Completed);
    assert_eq!(output(&p, "after"), json!({"results":[7,-3,7]}));
    assert_eq!(p.attempts().len(), 10);
    assert_eq!(p.charged_tokens(), 0);
    assert_eq!(
        p.execution_graph(&plan).unwrap().composition_scopes.len(),
        5
    );
}

#[test]
fn wrong_output_types_unbound_arguments_and_cycles_are_rejected_at_publication() {
    for kind in 0..4 {
        let mut definition = definition();
        match kind {
            0 => {
                definition
                    .dataflow
                    .as_mut()
                    .unwrap()
                    .steps
                    .get_mut("loop")
                    .unwrap()
                    .output = WorkflowDataType::Boolean
            }
            1 => {
                definition
                    .iterations
                    .get_mut("loop")
                    .unwrap()
                    .arguments
                    .clear();
            }
            2 => definition.iterations.get_mut("loop").unwrap().dependencies = vec!["after".into()],
            _ => {
                definition
                    .dataflow
                    .as_mut()
                    .unwrap()
                    .steps
                    .get_mut("loop")
                    .unwrap()
                    .inputs
                    .get_mut("items")
                    .unwrap()
                    .value = WorkflowDataSource::Input {
                    pointer: "/undeclared".into(),
                }
            }
        }
        assert!(definition.validate_definition().is_err(), "variant {kind}");
    }
}

#[test]
fn incompatible_versions_and_nested_loop_sources_are_not_silently_ignored() {
    for version in 1..=4 {
        let mut definition = definition();
        definition.format_version = version;
        assert!(definition.validate_definition().is_err());
    }
    let mut empty = definition();
    empty.iterations.clear();
    assert!(empty.validate_definition().is_err());
    let mut nested = definition();
    *nested.iterations.get_mut("loop").unwrap().definition = definition();
    assert!(nested.validate_definition().is_err());
}

#[test]
fn iteration_marker_and_source_binding_cannot_be_forged_by_rehashing_the_plan() {
    let definition = definition();
    let plan = WorkflowPlan::compile(&definition, &scope(&definition), &[]).unwrap();
    let mut edited = serde_json::to_value(&plan).unwrap();
    edited["iteration_definition"] = json!(false);
    edited["sha256"] = json!("");
    edited["sha256"] = json!(canonical_hash(&edited));
    assert!(
        serde_json::from_value::<WorkflowPlan>(edited)
            .unwrap()
            .validate()
            .is_err()
    );
    let old: WorkflowPlan = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-plan-before-composition.json"
    ))
    .unwrap();
    old.validate().unwrap();
    let bytes = serde_json::to_string(&old).unwrap();
    assert!(!bytes.contains("\"iterations\""));
    assert!(!bytes.contains("iteration_definition"));
    assert_eq!(
        old.sha256,
        "843bb38c743928c7b5d24804bd53251114620784b7955fd275495cf95ef0a800"
    );
}
