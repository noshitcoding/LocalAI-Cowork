use cowork_contracts::Budgets;
use cowork_domain::{canonical_hash, workflows::*};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn form() -> WorkflowReviewForm {
    serde_json::from_value(json!({"question":"Check the invoice fields before booking.","fields":{
        "invoice":{"label":"Invoice number","help":"Match the original invoice.","value_type":{"type":"text"},"format":"value","required":true,"minimum":null,"maximum":null},
        "date":{"label":"Issued on","help":"","value_type":{"type":"text"},"format":"date","required":true,"minimum":null,"maximum":null},
        "net":{"label":"Net","help":"","value_type":{"type":"integer"},"format":"money_minor","required":true,"minimum":0,"maximum":9007199254740991_i64},
        "tax":{"label":"Tax","help":"","value_type":{"type":"integer"},"format":"money_minor","required":true,"minimum":0,"maximum":null},
        "gross":{"label":"Gross","help":"","value_type":{"type":"integer"},"format":"money_minor","required":true,"minimum":0,"maximum":null}
    },"checks":[{"kind":"sum","terms":["net","tax"],"total":"gross"}]})).unwrap()
}
fn values() -> Value {
    json!({"invoice":"INV-42","date":"2026-11-01","net":10000,"tax":1900,"gross":11900})
}
fn definition() -> WorkflowDefinition {
    let form = form();
    let inputs: BTreeMap<_, _> = form
        .fields
        .iter()
        .map(|(name, f)| {
            (
                name.clone(),
                WorkflowValueBinding {
                    value_type: f.value_type.clone(),
                    value: WorkflowDataSource::Input {
                        pointer: format!("/{name}"),
                    },
                },
            )
        })
        .collect();
    serde_json::from_value(json!({"format_version":3,"name":"Review invoice","mode":{"mode":"parallel","max_parallel":1},"failure":"stop",
        "rights":{"tools":["request_input"],"folders":[]},"budgets":Budgets::default(),"roles":[],"steps":[],
        "controls":{"review":{"name":"Check invoice","dependencies":[],"operation":{"kind":"review","form":form,"rights":{"tools":["request_input"],"folders":[]}}}},
        "dataflow":{"input":form.output_type(),"steps":{"review":{"inputs":inputs,"output":form.output_type()}}}})).unwrap()
}
fn scope(d: &WorkflowDefinition) -> WorkflowScope {
    WorkflowScope {
        workspace_folder: None,
        rights: d.rights.clone(),
        budgets: d.budgets.clone(),
        max_parallel: 1,
    }
}

#[test]
fn model_free_review_compiles_to_only_original_human_input_rights_and_cannot_auto_complete() {
    let d = definition();
    d.validate_definition().unwrap();
    let p = WorkflowPlan::compile(&d, &scope(&d), &[]).unwrap();
    assert!(p.roles.is_empty() && p.profiles.is_empty());
    assert_eq!(p.nodes["review"].role_id, None);
    assert_eq!(
        p.nodes["review"].rights.tools,
        std::collections::BTreeSet::from(["request_input".into()])
    );
    assert!(p.nodes["review"].rights.folders.is_empty());
    let mut progress = WorkflowProgress::new(&p).unwrap();
    let c = progress
        .claim(&p, progress.revision(), "review", 100)
        .unwrap();
    assert!(!progress.requires_native(&p, "review").unwrap());
    assert!(
        progress
            .complete_control(
                &p,
                progress.revision(),
                &c,
                "forbidden",
                Some(&values()),
                100
            )
            .is_err()
    );
    assert!(
        d.evaluate_control("review", Some(&values()), &BTreeMap::new())
            .is_err()
    );
    let mut revoked = scope(&d);
    revoked.rights.tools.clear();
    assert!(WorkflowPlan::compile(&d, &revoked, &[]).is_err());
}
#[test]
fn prefilled_invalid_values_require_human_correction_and_extra_fields_are_rejected() {
    let d = definition();
    let mut initial = values();
    initial["invoice"] = json!("");
    initial["gross"] = json!(1);
    let request = d
        .review_request("review", Some(&initial), &BTreeMap::new())
        .unwrap();
    assert_eq!(request.values, initial);
    assert!(request.form.validate_answer(&initial).is_err());
    request.form.validate_answer(&values()).unwrap();
    let mut extra = values();
    extra["new_permission"] = json!(true);
    assert!(request.form.validate_answer(&extra).is_err());
    assert!(
        d.decode_step_output("review", &initial.to_string())
            .is_err()
    );
    assert_eq!(
        d.decode_step_output("review", &values().to_string())
            .unwrap(),
        values()
    );
}
#[test]
fn required_fields_real_calendar_dates_ranges_and_integer_money_are_enforced() {
    for (field, value) in [
        ("invoice", json!("   ")),
        ("date", json!("2026-02-29")),
        ("date", json!("1900-02-29")),
        ("date", json!("0000-01-01")),
        ("date", json!("2026-1-01")),
        ("net", json!(-1)),
        ("tax", json!(19.01)),
        ("gross", json!(9007199254740992_i64)),
        ("gross", Value::Null),
    ] {
        let mut bad = values();
        bad[field] = value;
        assert!(form().validate_answer(&bad).is_err(), "{field}: {bad}");
    }
    let mut leap = values();
    leap["date"] = json!("2000-02-29");
    form().validate_answer(&leap).unwrap();
    let mut encoded = values();
    encoded["net"] = json!(10000.0);
    encoded["tax"] = json!(1900.0);
    encoded["gross"] = json!(11900.0);
    form().validate_answer(&encoded).unwrap();
}
#[test]
fn malformed_unbounded_or_ambiguous_review_contracts_never_validate() {
    let original = serde_json::to_value(form()).unwrap();
    for (pointer, value) in [
        ("/fields/date/value_type/type", json!("integer")),
        ("/fields/net/minimum", json!(9007199254740992_i64)),
        ("/fields/net/maximum", json!(-1)),
        ("/checks/0/terms", json!(["net", "net"])),
        ("/checks/0/total", json!("absent")),
        ("/checks/0/terms", json!(["net", "gross"])),
    ] {
        let mut invalid = original.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        let f: WorkflowReviewForm = serde_json::from_value(invalid).unwrap();
        assert!(f.validate().is_err(), "{pointer}");
    }
    let mut invalid = form();
    invalid.fields.clear();
    assert!(invalid.validate().is_err());
    let mut d = definition();
    d.rights.tools.clear();
    assert!(d.validate_definition().is_err());
    let mut d = definition();
    d.dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("review")
        .unwrap()
        .inputs
        .remove("net");
    assert!(d.validate_definition().is_err());
}
#[test]
fn legacy_controls_roundtrip_without_review_metadata() {
    let d: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-control.json"
    ))
    .unwrap();
    let serialized = serde_json::to_value(&d).unwrap();
    assert!(!serialized.to_string().contains("review_form"));
    let first = WorkflowPlan::compile(
        &d,
        &WorkflowScope {
            rights: WorkflowRights::default(),
            budgets: Budgets::default(),
            workspace_folder: None,
            max_parallel: 2,
        },
        &[],
    )
    .unwrap();
    let roundtrip: WorkflowDefinition = serde_json::from_value(serialized).unwrap();
    let second = WorkflowPlan::compile(&roundtrip, &first.scope, &[]).unwrap();
    assert_eq!(first.sha256, second.sha256);
    assert_eq!(canonical_hash(&d), canonical_hash(&roundtrip));
}

#[test]
fn embedded_review_cannot_inherit_a_right_removed_by_its_call_scope() {
    let child = definition();
    let data = child.dataflow.as_ref().unwrap();
    let mut outer = child.clone();
    outer.format_version = 4;
    outer.controls.clear();
    outer.subflows.insert(
        "call".into(),
        WorkflowEmbeddedSubflow {
            name: "Invoice review".into(),
            dependencies: vec![],
            definition: Box::new(child.clone()),
            result: WorkflowOutputRef {
                step_id: "review".into(),
                pointer: String::new(),
            },
            rights: child.rights.clone(),
            budgets: child.budgets.clone(),
        },
    );
    outer.dataflow.as_mut().unwrap().steps =
        BTreeMap::from([("call".into(), data.steps["review"].clone())]);
    let plan = WorkflowPlan::compile(&outer, &scope(&outer), &[]).unwrap();
    assert!(plan.roles.is_empty());
    assert!(
        plan.nodes
            .values()
            .any(|node| node.rights.tools.contains("request_input"))
    );
    outer.subflows.get_mut("call").unwrap().rights.tools.clear();
    assert!(outer.validate_definition().is_err());
    assert!(WorkflowPlan::compile(&outer, &scope(&outer), &[]).is_err());
}

#[test]
fn missing_extracted_fields_are_reviewable_and_required_outputs_are_nonnullable_for_downstream_actions()
 {
    let mut d = definition();
    let WorkflowControlOperation::Review { form, .. } =
        &mut d.controls.get_mut("review").unwrap().operation
    else {
        panic!()
    };
    for field in form.fields.values_mut() {
        field.value_type = WorkflowDataType::Nullable {
            value: Box::new(field.value_type.clone()),
        };
    }
    let form = form.clone();
    let data = d.dataflow.as_mut().unwrap();
    data.input = form.prefill_type();
    let config = data.steps.get_mut("review").unwrap();
    for (name, field) in &form.fields {
        config.inputs.get_mut(name).unwrap().value_type = field.value_type.clone();
    }
    config.output = form.output_type();
    let missing = json!({"invoice":null,"date":null,"net":null,"tax":null,"gross":null});
    d.validate_definition().unwrap();
    WorkflowPlan::compile(&d, &scope(&d), &[]).unwrap();
    assert_eq!(
        d.review_request("review", Some(&missing), &BTreeMap::new())
            .unwrap()
            .values,
        missing
    );
    assert!(form.validate_answer(&missing).is_err());
    form.validate_answer(&values()).unwrap();
    assert_eq!(config_type(&d, "gross"), WorkflowDataType::Integer);
    assert_eq!(config_type(&d, "date"), WorkflowDataType::Text);
    assert!(
        d.decode_step_output("review", &missing.to_string())
            .is_err()
    );
    assert_eq!(
        d.decode_step_output("review", &values().to_string())
            .unwrap(),
        values()
    );
}
fn config_type(d: &WorkflowDefinition, key: &str) -> WorkflowDataType {
    d.dataflow.as_ref().unwrap().steps["review"]
        .output
        .at_pointer(&format!("/{key}"))
        .unwrap()
        .clone()
}
