use cowork_contracts::Budgets;
use cowork_domain::workflows::*;
use serde_json::{Value, json};

fn definition() -> WorkflowDefinition {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-control.json"
    ))
    .unwrap()
}
fn compile(d: &WorkflowDefinition) -> WorkflowPlan {
    WorkflowPlan::compile(
        d,
        &WorkflowScope {
            workspace_folder: None,
            rights: WorkflowRights::default(),
            budgets: Budgets::default(),
            max_parallel: 2,
        },
        &[],
    )
    .unwrap()
}
fn run(plan: &WorkflowPlan, input: &Value) -> WorkflowProgress {
    let mut p = WorkflowProgress::new(plan).unwrap();
    for _ in 0..64 {
        let ready = p.ready_nodes(plan).unwrap();
        if ready.is_empty() {
            break;
        }
        for node in ready {
            let claim = p.claim(plan, p.revision(), &node, 100).unwrap();
            p.complete_control(
                plan,
                p.revision(),
                &claim,
                &format!("native-{node}"),
                Some(input),
                100,
            )
            .unwrap();
        }
    }
    p
}
#[test]
fn both_routes_join_without_a_provider_or_tokens_and_survive_checkpoint_replay() {
    let plan = compile(&definition());
    assert!(plan.profiles.is_empty());
    assert!(
        plan.nodes
            .values()
            .all(|n| n.role_id.is_none() && n.rights == WorkflowRights::default())
    );
    for (amount, route, skipped) in [(150, "review", "automatic"), (25, "automatic", "review")] {
        let input = json!({"amount": amount});
        let p = run(&plan, &input);
        assert_eq!(p.state(), WorkflowState::Completed);
        assert_eq!(p.charged_tokens(), 0);
        assert_eq!(p.attempts().len(), 4);
        assert!(matches!(
            p.nodes()[skipped].state,
            WorkflowNodeState::Skipped { .. }
        ));
        for (id, expected) in [
            ("choice", json!({"amount":amount,"route":route})),
            ("results", json!([{"amount":amount,"route":route}])),
        ] {
            let WorkflowNodeState::Completed { output, .. } = &p.nodes()[id].state else {
                panic!("{id}");
            };
            assert_eq!(
                plan.definition.decode_step_output(id, output).unwrap(),
                expected
            );
        }
        let restored: WorkflowProgress =
            serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        restored.validate(&plan).unwrap();
        assert!(restored.ready_nodes(&plan).unwrap().is_empty());
    }
}
#[test]
fn invalid_gates_types_cycles_and_unbounded_predicates_never_compile() {
    let base = serde_json::to_value(definition()).unwrap();
    for (pointer, value) in [
        ("/format_version", json!(2)),
        ("/mode/mode", json!("sequential")),
        ("/gates/review/step_id", json!("review")),
        ("/controls/large/dependencies", json!(["results"])),
        ("/dataflow/steps/large/output", json!({"type":"text"})),
        ("/controls/large/operation/predicate/value", json!("100")),
        (
            "/controls/choice/operation/candidates/0/pointer",
            json!("/missing"),
        ),
        ("/dataflow/steps/results/output/max_items", json!(1)),
    ] {
        let mut v = base.clone();
        *v.pointer_mut(pointer).unwrap() = value;
        assert!(
            serde_json::from_value::<WorkflowDefinition>(v)
                .unwrap()
                .validate_definition()
                .is_err(),
            "{pointer}"
        );
    }
    let mut d = definition();
    if let WorkflowControlOperation::Condition { predicate } =
        &mut d.controls.get_mut("large").unwrap().operation
    {
        for _ in 0..5 {
            *predicate = WorkflowPredicate::Not {
                predicate: Box::new(predicate.clone()),
            };
        }
    }
    assert!(d.validate_definition().is_err());
}
#[test]
fn selecting_two_active_results_fails_without_committing_a_partial_decision() {
    let mut d = definition();
    d.gates.clear();
    let plan = compile(&d);
    let input = json!({"amount":25});
    let mut p = WorkflowProgress::new(&plan).unwrap();
    for id in ["large", "review", "automatic"] {
        let claim = p.claim(&plan, p.revision(), id, 100).unwrap();
        p.complete_control(
            &plan,
            p.revision(),
            &claim,
            &format!("native-{id}"),
            Some(&input),
            100,
        )
        .unwrap();
    }
    let claim = p.claim(&plan, p.revision(), "choice", 100).unwrap();
    let before = serde_json::to_value(&p).unwrap();
    assert!(
        p.complete_control(&plan, p.revision(), &claim, "ambiguous", Some(&input), 100)
            .is_err()
    );
    assert_eq!(serde_json::to_value(&p).unwrap(), before);
    p.validate(&plan).unwrap();
}
#[test]
fn native_nodes_cannot_be_admitted_as_models_or_completed_with_forged_outputs() {
    let plan = compile(&definition());
    let mut p = WorkflowProgress::new(&plan).unwrap();
    let claim = p.claim(&plan, 0, "large", 100).unwrap();
    assert!(
        p.admit(
            &plan,
            p.revision(),
            &claim,
            "model",
            WorkflowOperation::Model {
                request_sha256: "a".repeat(64),
                reserved_tokens: None
            },
            100
        )
        .is_err()
    );
    assert!(
        p.complete(&plan, p.revision(), &claim, "true", 100)
            .is_err()
    );
    let input = json!({"amount":25});
    p.complete_control(&plan, p.revision(), &claim, "control", Some(&input), 100)
        .unwrap();
    let claim = p.claim(&plan, p.revision(), "review", 100).unwrap();
    let changed = json!({"amount":150});
    assert!(
        p.complete_control(&plan, p.revision(), &claim, "changed", Some(&changed), 100)
            .is_err()
    );
    p.complete_control(&plan, p.revision(), &claim, "skip", Some(&input), 100)
        .unwrap();
    assert!(matches!(
        p.nodes()["review"].state,
        WorkflowNodeState::Skipped { .. }
    ));
    let mut v = serde_json::to_value(&p).unwrap();
    v["nodes"]["review"]["state"] =
        json!({"state":"completed","output":"{}","sha256":cowork_domain::hash(b"{}")});
    assert!(
        serde_json::from_value::<WorkflowProgress>(v)
            .unwrap()
            .validate(&plan)
            .is_err()
    );
}
#[test]
fn old_definitions_and_plans_keep_their_original_wire_shape() {
    let original: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-dataflow.json"
    ))
    .unwrap();
    let d: WorkflowDefinition = serde_json::from_value(original.clone()).unwrap();
    let encoded = serde_json::to_value(d).unwrap();
    assert!(encoded.get("controls").is_none() && encoded.get("gates").is_none());
    assert_eq!(encoded, original);
}

#[test]
fn control_work_obeys_step_budgets_and_original_elapsed_time() {
    let mut d = definition();
    d.budgets.max_steps = 1;
    let plan = compile(&d);
    let input = json!({"amount":150});
    let mut p = WorkflowProgress::new(&plan).unwrap();
    let claim = p.claim(&plan, 0, "large", 100).unwrap();
    p.complete_control(&plan, p.revision(), &claim, "first", Some(&input), 100)
        .unwrap();
    let claim = p.claim(&plan, p.revision(), "review", 100).unwrap();
    assert!(
        p.complete_control(
            &plan,
            p.revision(),
            &claim,
            "over-budget",
            Some(&input),
            100
        )
        .is_err()
    );
    assert!(
        p.complete_control(&plan, p.revision(), &claim, "expired", Some(&input), 1000)
            .is_err()
    );
    assert_eq!(p.attempts().len(), 1);
    p.validate(&plan).unwrap();
}

#[test]
fn empty_selected_branches_produce_a_durable_skip_and_an_empty_collection() {
    let mut d = definition();
    d.gates.get_mut("automatic").unwrap().when = true;
    let plan = compile(&d);
    let p = run(&plan, &json!({"amount":25}));
    assert_eq!(p.state(), WorkflowState::Completed);
    assert!(matches!(
        p.nodes()["choice"].state,
        WorkflowNodeState::Skipped { .. }
    ));
    let WorkflowNodeState::Completed { output, .. } = &p.nodes()["results"].state else {
        panic!()
    };
    assert_eq!(output, "[]");
}

#[test]
fn native_claim_resume_does_not_accept_fabricated_completion_evidence() {
    let plan = compile(&definition());
    let mut p = WorkflowProgress::new(&plan).unwrap();
    p.claim(&plan, 0, "large", 100).unwrap();
    p.recover(&plan, p.revision(), 101).unwrap();
    assert!(
        p.resume_settled(
            &plan,
            p.revision(),
            std::collections::BTreeMap::from([("large".into(), "true".into())]),
            102
        )
        .is_err()
    );
    p.resume_settled(&plan, p.revision(), Default::default(), 102)
        .unwrap();
    assert_eq!(p.nodes()["large"].state, WorkflowNodeState::Pending);
    p.validate(&plan).unwrap();
}

#[test]
fn combined_conditions_use_typed_text_boolean_null_and_numeric_values() {
    let mut d = definition();
    let config = &mut d
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("large")
        .unwrap()
        .inputs;
    for (key, ty, value) in [
        ("flag", WorkflowDataType::Boolean, json!(false)),
        ("word", WorkflowDataType::Text, json!("Rechnung ✓")),
        (
            "optional",
            WorkflowDataType::Nullable {
                value: Box::new(WorkflowDataType::Text),
            },
            Value::Null,
        ),
    ] {
        config.insert(
            key.into(),
            WorkflowValueBinding {
                value_type: ty,
                value: WorkflowDataSource::Literal { value },
            },
        );
    }
    d.controls.get_mut("large").unwrap().operation = WorkflowControlOperation::Condition {
        predicate: WorkflowPredicate::All {
            predicates: vec![
                WorkflowPredicate::Not {
                    predicate: Box::new(WorkflowPredicate::IsTrue {
                        input: "flag".into(),
                    }),
                },
                WorkflowPredicate::Any {
                    predicates: vec![
                        WorkflowPredicate::Contains {
                            input: "word".into(),
                            value: "✓".into(),
                        },
                        WorkflowPredicate::IsTrue {
                            input: "flag".into(),
                        },
                    ],
                },
                WorkflowPredicate::IsNull {
                    input: "optional".into(),
                },
                WorkflowPredicate::Less {
                    input: "amount".into(),
                    value: Box::new(json!(100)),
                },
                WorkflowPredicate::NotEqual {
                    input: "amount".into(),
                    value: Box::new(json!(100)),
                },
            ],
        },
    };
    let plan = compile(&d);
    let p = run(&plan, &json!({"amount":25}));
    assert!(matches!(
        p.nodes()["review"].state,
        WorkflowNodeState::Completed { .. }
    ));
    let p = run(&plan, &json!({"amount":150}));
    assert!(matches!(
        p.nodes()["review"].state,
        WorkflowNodeState::Skipped { .. }
    ));
    d.dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("large")
        .unwrap()
        .inputs
        .get_mut("flag")
        .unwrap()
        .value = WorkflowDataSource::Literal {
        value: json!("false"),
    };
    assert!(
        d.validate_definition().is_err(),
        "Text is never coerced into a boolean"
    );
}

#[test]
fn numeric_equality_ignores_integer_or_float_json_encoding_in_nested_values() {
    let mut d = definition();
    let ty = WorkflowDataType::Object {
        fields: std::collections::BTreeMap::from([(
            "amounts".into(),
            WorkflowDataType::List {
                item: Box::new(WorkflowDataType::Number),
                max_items: 2,
            },
        )]),
    };
    d.dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("large")
        .unwrap()
        .inputs
        .insert(
            "record".into(),
            WorkflowValueBinding {
                value_type: ty,
                value: WorkflowDataSource::Literal {
                    value: json!({"amounts":[25.0,100]}),
                },
            },
        );
    d.controls.get_mut("large").unwrap().operation = WorkflowControlOperation::Condition {
        predicate: WorkflowPredicate::Equal {
            input: "record".into(),
            value: Box::new(json!({"amounts":[25,100.0]})),
        },
    };
    let plan = compile(&d);
    let p = run(&plan, &json!({"amount":25}));
    assert!(matches!(
        p.nodes()["review"].state,
        WorkflowNodeState::Completed { .. }
    ));
    if let WorkflowControlOperation::Condition {
        predicate: WorkflowPredicate::Equal { value, .. },
    } = &mut d.controls.get_mut("large").unwrap().operation
    {
        **value = json!({"amounts":[25,"100"]});
    }
    assert!(
        d.validate_definition().is_err(),
        "Numeric strings remain invalid"
    );
}
