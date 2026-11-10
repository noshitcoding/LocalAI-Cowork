use cowork_domain::workflows::{WorkflowDataContext, WorkflowDefinition};
use serde_json::{Value, json};

fn source() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-dataflow.json"
    ))
    .unwrap()
}

#[test]
fn legacy_context_remains_inherited_without_adding_serialized_keys() {
    let original = source();
    let definition: WorkflowDefinition = serde_json::from_value(original).unwrap();
    definition.validate_definition().unwrap();
    assert_eq!(
        definition.dataflow.as_ref().unwrap().steps["extract"].context_policy,
        WorkflowDataContext::Inherit
    );
    let serialized = serde_json::to_value(definition).unwrap();
    assert!(
        serialized["dataflow"]["steps"]["extract"]
            .get("context_policy")
            .is_none()
    );
}

#[test]
fn model_context_boundary_is_explicit_and_survives_serialization() {
    let mut raw = source();
    raw["dataflow"]["steps"]["extract"]["context_policy"] = json!("bound_inputs_only");
    let definition: WorkflowDefinition = serde_json::from_value(raw).unwrap();
    definition.validate_definition().unwrap();
    assert_eq!(
        definition.dataflow.as_ref().unwrap().steps["extract"].context_policy,
        WorkflowDataContext::BoundInputsOnly
    );
    assert_eq!(
        serde_json::to_value(definition).unwrap()["dataflow"]["steps"]["extract"]["context_policy"],
        "bound_inputs_only"
    );
}

#[test]
fn invalid_policies_and_context_policies_on_native_reviews_are_rejected() {
    let mut raw = source();
    raw["dataflow"]["steps"]["extract"]["context_policy"] = json!("unknown");
    assert!(serde_json::from_value::<WorkflowDefinition>(raw).is_err());
    let mut raw: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-native-review.json"
    ))
    .unwrap();
    raw["dataflow"]["steps"]["review"]["context_policy"] = json!("bound_inputs_only");
    assert!(
        serde_json::from_value::<WorkflowDefinition>(raw)
            .unwrap()
            .validate_definition()
            .is_err()
    );
}
