use cowork_contracts::Budgets;
use cowork_domain::{canonical_hash, workflows::*};
use serde_json::{Value, json};
fn definition() -> WorkflowDefinition {
    serde_json::from_value(json!({"format_version":3,"name":"Durable wait","mode":{"mode":"parallel","max_parallel":2},"failure":"stop","rights":{"tools":[],"folders":[]},"budgets":Budgets::default(),"roles":[],"steps":[],
        "controls":{"wait":{"name":"Wait","dependencies":[],"operation":{"kind":"wait","seconds_input":"seconds"}},"after":{"name":"After","dependencies":["wait"],"operation":{"kind":"compose"}}},
        "dataflow":{"input":{"type":"object","fields":{"delay":{"type":"integer"}}},"steps":{
            "wait":{"inputs":{"seconds":{"value_type":{"type":"integer"},"value":{"source":"input","pointer":"/delay"}}},"output":{"type":"object","fields":{"wake_at":{"type":"integer"}}}},
            "after":{"inputs":{"when":{"value_type":{"type":"integer"},"value":{"source":"step","step_id":"wait","pointer":"/wake_at"}}},"output":{"type":"object","fields":{"when":{"type":"integer"}}}}
        }}})).unwrap()
}
fn plan(d: &WorkflowDefinition) -> WorkflowPlan {
    WorkflowPlan::compile(
        d,
        &WorkflowScope {
            workspace_folder: None,
            rights: WorkflowRights::default(),
            budgets: d.budgets.clone(),
            max_parallel: 2,
        },
        &[],
    )
    .unwrap()
}
fn start(plan: &WorkflowPlan, delay: Value) -> WorkflowProgress {
    let mut p = WorkflowProgress::new(plan).unwrap();
    let claim = p.claim(plan, 0, "wait", 100).unwrap();
    p.complete_control(
        plan,
        p.revision(),
        &claim,
        "timer",
        Some(&json!({"delay":delay})),
        100,
    )
    .unwrap();
    p
}
#[test]
fn timer_uses_one_step_and_no_tokens_and_never_completes_early() {
    let plan = plan(&definition());
    let mut p = start(&plan, json!(5.0));
    assert_eq!(p.next_wake_at(), Some(105));
    assert!(p.can_park_timer(&plan).unwrap());
    assert_eq!(p.attempts().len(), 1);
    assert_eq!(p.charged_tokens(), 0);
    let revision = p.revision();
    assert!(!p.advance_timers(&plan, revision, 104).unwrap());
    assert_eq!(p.revision(), revision);
    assert!(p.ready_nodes(&plan).unwrap().is_empty());
    assert!(p.claim(&plan, p.revision(), "wait", 104).is_err());
    assert!(p.advance_timers(&plan, revision + 1, 105).is_err());
    assert!(p.advance_timers(&plan, revision, 105).unwrap());
    assert_eq!(p.ready_nodes(&plan).unwrap(), vec!["after"]);
    let claim = p.claim(&plan, p.revision(), "after", 106).unwrap();
    p.complete_control(
        &plan,
        p.revision(),
        &claim,
        "after",
        Some(&json!({"delay":5.0})),
        106,
    )
    .unwrap();
    assert_eq!(p.state(), WorkflowState::Completed);
    p.validate(&plan).unwrap();
    assert_eq!(p.attempts().len(), 2);
    let WorkflowNodeState::Completed { output, .. } = &p.nodes()["after"].state else {
        panic!()
    };
    assert_eq!(output, "{\"when\":105}");
}
#[test]
fn timer_survives_reopen_and_recovery_without_restarting_its_delay() {
    let plan = plan(&definition());
    let mut p = start(&plan, json!(10));
    p.recover(&plan, p.revision(), 103).unwrap();
    assert!(p.can_park_timer(&plan).unwrap());
    assert_eq!(p.next_wake_at(), Some(110));
    let mut saved: WorkflowProgress =
        serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
    saved.validate(&plan).unwrap();
    assert!(saved.advance_timers(&plan, saved.revision(), 110).unwrap());
    assert!(saved.next_wake_at().is_none());
    assert_eq!(saved.attempts().len(), 1);
}
#[test]
fn invalid_durations_and_wrong_types_never_schedule_a_timer() {
    let d = definition();
    let plan = plan(&d);
    for delay in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(86401),
        json!("5"),
        json!(null),
    ] {
        let mut p = WorkflowProgress::new(&plan).unwrap();
        let claim = p.claim(&plan, 0, "wait", 100).unwrap();
        let before = serde_json::to_value(&p).unwrap();
        assert!(
            p.complete_control(
                &plan,
                p.revision(),
                &claim,
                "bad",
                Some(&json!({"delay":delay})),
                100
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(&p).unwrap(), before);
    }
    let mut wrong = serde_json::to_value(&d).unwrap();
    wrong["dataflow"]["steps"]["wait"]["inputs"]["seconds"]["value_type"] =
        json!({"type":"number"});
    assert!(
        serde_json::from_value::<WorkflowDefinition>(wrong)
            .unwrap()
            .validate_definition()
            .is_err()
    );
}
#[test]
fn waiting_does_not_reset_or_extend_the_confirmed_time_budget() {
    let mut d = definition();
    d.budgets.max_seconds = 10;
    let plan = plan(&d);
    let mut p = start(&plan, json!(5));
    p.advance_timers(&plan, p.revision(), 110).unwrap();
    assert_eq!(p.state(), WorkflowState::Failed);
    assert!(
        matches!(p.nodes()["wait"].state,WorkflowNodeState::Failed{ref code} if code=="budget_exceeded")
    );
    let mut p = WorkflowProgress::new(&plan).unwrap();
    let claim = p.claim(&plan, 0, "wait", 100).unwrap();
    assert!(
        p.complete_control(
            &plan,
            p.revision(),
            &claim,
            "over-budget",
            Some(&json!({"delay":10})),
            100
        )
        .is_err()
    );
}
#[test]
fn rehashing_a_changed_timer_or_output_cannot_make_it_valid() {
    let plan = plan(&definition());
    let p = start(&plan, json!(5));
    let mut value = serde_json::to_value(&p).unwrap();
    value["nodes"]["wait"]["state"]["wake_at"] = json!(101);
    assert!(
        serde_json::from_value::<WorkflowProgress>(value)
            .unwrap()
            .validate(&plan)
            .is_err()
    );
}

#[test]
fn subsecond_start_never_shortens_the_requested_wait() {
    let plan = plan(&definition());
    let mut p = WorkflowProgress::new(&plan).unwrap();
    let claim = p.claim(&plan, 0, "wait", 100).unwrap();
    p.complete_timer(
        &plan,
        p.revision(),
        &claim,
        "precise",
        Some(&json!({"delay":5})),
        100999,
    )
    .unwrap();
    assert_eq!(p.next_wake_at(), Some(106));
    assert!(!p.advance_timers(&plan, p.revision(), 105).unwrap());
    assert!(p.advance_timers(&plan, p.revision(), 106).unwrap());
    p.validate(&plan).unwrap();
    assert_eq!(serde_json::to_value(&p).unwrap()["format_version"], 9);
}

#[test]
fn preflight_checks_known_native_bindings_without_advancing_virtual_timers() {
    let mut source = definition();
    source
        .controls
        .get_mut("wait")
        .unwrap()
        .dependencies
        .push("frame".into());
    source.controls.insert(
        "frame".into(),
        WorkflowControlStep {
            name: "Known duration".into(),
            dependencies: vec![],
            operation: WorkflowControlOperation::Compose,
        },
    );
    let data = source.dataflow.as_mut().unwrap();
    data.steps.insert(
        "frame".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: std::collections::BTreeMap::from([(
                "delay".into(),
                WorkflowValueBinding {
                    value_type: WorkflowDataType::Integer,
                    value: WorkflowDataSource::Input {
                        pointer: "/delay".into(),
                    },
                },
            )]),
            output: WorkflowDataType::Object {
                fields: std::collections::BTreeMap::from([(
                    "delay".into(),
                    WorkflowDataType::Integer,
                )]),
            },
        },
    );
    data.steps
        .get_mut("wait")
        .unwrap()
        .inputs
        .get_mut("seconds")
        .unwrap()
        .value = WorkflowDataSource::Step {
        step_id: "frame".into(),
        pointer: "/delay".into(),
    };
    let compiled = plan(&source);
    compiled
        .validate_wait_inputs(Some(&json!({"delay":5})))
        .unwrap();
    assert!(
        compiled
            .validate_wait_inputs(Some(&json!({"delay":0})))
            .is_err()
    );
    assert!(
        compiled
            .validate_wait_inputs(Some(&json!({"delay":86401})))
            .is_err()
    );
    // The downstream result contains a UTC timestamp. It remains unknown;
    // preflight must not treat virtual time zero as a real output value.
    assert_eq!(compiled.sha256, plan(&source).sha256);
}

#[test]
fn preflight_skips_invalid_durations_only_when_the_known_gate_is_false() {
    let mut source = definition();
    source.controls.insert(
        "enabled".into(),
        WorkflowControlStep {
            name: "Run this branch".into(),
            dependencies: vec![],
            operation: WorkflowControlOperation::Condition {
                predicate: WorkflowPredicate::IsTrue {
                    input: "run".into(),
                },
            },
        },
    );
    source
        .controls
        .get_mut("wait")
        .unwrap()
        .dependencies
        .push("enabled".into());
    source.gates.insert(
        "wait".into(),
        WorkflowGate {
            step_id: "enabled".into(),
            when: true,
        },
    );
    let data = source.dataflow.as_mut().unwrap();
    let WorkflowDataType::Object { fields } = &mut data.input else {
        panic!()
    };
    fields.insert("run".into(), WorkflowDataType::Boolean);
    data.steps.insert(
        "enabled".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: std::collections::BTreeMap::from([(
                "run".into(),
                WorkflowValueBinding {
                    value_type: WorkflowDataType::Boolean,
                    value: WorkflowDataSource::Input {
                        pointer: "/run".into(),
                    },
                },
            )]),
            output: WorkflowDataType::Boolean,
        },
    );
    let compiled = plan(&source);
    compiled
        .validate_wait_inputs(Some(&json!({"delay":0,"run":false})))
        .unwrap();
    assert!(
        compiled
            .validate_wait_inputs(Some(&json!({"delay":0,"run":true})))
            .is_err()
    );
}

#[test]
fn preflight_never_infers_an_epoch_dependent_branch_from_its_virtual_clock() {
    let mut source = definition();
    source.controls.insert(
        "epoch".into(),
        WorkflowControlStep {
            name: "Epoch-dependent condition".into(),
            dependencies: vec!["wait".into()],
            operation: WorkflowControlOperation::Condition {
                predicate: WorkflowPredicate::Less {
                    input: "wake".into(),
                    value: Box::new(json!(86400)),
                },
            },
        },
    );
    let after = source.controls.get_mut("after").unwrap();
    after.dependencies = vec!["epoch".into()];
    after.operation = WorkflowControlOperation::Wait {
        seconds_input: "seconds".into(),
    };
    source.gates.insert(
        "after".into(),
        WorkflowGate {
            step_id: "epoch".into(),
            when: true,
        },
    );
    let data = source.dataflow.as_mut().unwrap();
    let WorkflowDataType::Object { fields } = &mut data.input else {
        panic!()
    };
    fields.insert("unsafe".into(), WorkflowDataType::Integer);
    data.steps.insert(
        "epoch".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: std::collections::BTreeMap::from([(
                "wake".into(),
                WorkflowValueBinding {
                    value_type: WorkflowDataType::Integer,
                    value: WorkflowDataSource::Step {
                        step_id: "wait".into(),
                        pointer: "/wake_at".into(),
                    },
                },
            )]),
            output: WorkflowDataType::Boolean,
        },
    );
    data.steps.insert(
        "after".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: std::collections::BTreeMap::from([(
                "seconds".into(),
                WorkflowValueBinding {
                    value_type: WorkflowDataType::Integer,
                    value: WorkflowDataSource::Input {
                        pointer: "/unsafe".into(),
                    },
                },
            )]),
            output: wait_output_type(),
        },
    );
    // Completing the virtual first timer would invent wake_at=5, select this
    // branch, and reject unsafe=0. Its actual epoch-dependent result is unknown.
    plan(&source)
        .validate_wait_inputs(Some(&json!({"delay":5,"unsafe":0})))
        .unwrap();
}

#[test]
fn recovery_before_parking_can_resume_only_a_settled_timer_without_new_outputs() {
    let plan = plan(&definition());
    let mut p = start(&plan, json!(5));
    let wake = p.next_wake_at();
    let attempts = canonical_hash(p.attempts());
    p.recover(&plan, p.revision(), 101).unwrap();
    assert!(p.can_park_timer(&plan).unwrap());
    let unchanged = canonical_hash(&p);
    assert!(
        p.resume_settled(
            &plan,
            p.revision(),
            std::collections::BTreeMap::from([("wait".into(), "forged".into())]),
            102
        )
        .is_err()
    );
    assert_eq!(canonical_hash(&p), unchanged);
    p.resume_settled(&plan, p.revision(), Default::default(), 102)
        .unwrap();
    assert_eq!(p.next_wake_at(), wake);
    assert_eq!(canonical_hash(p.attempts()), attempts);
    p.validate(&plan).unwrap();
    let mut busy = WorkflowProgress::new(&plan).unwrap();
    busy.claim(&plan, 0, "wait", 100).unwrap();
    assert!(
        busy.resume_settled(&plan, busy.revision(), Default::default(), 101)
            .is_err()
    );
}
