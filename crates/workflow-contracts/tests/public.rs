use cowork_contracts::AccessMode;
use cowork_workflow_contracts::{ApiContractV2, RunRequest};
use serde_json::json;

fn workflow() -> serde_json::Value {
    json!({"kind":"workflow","order_id":"o","base_revision":1,"idempotency_key":"key",
        "prompt":"Review the document","workflow_id":"w","version_id":"v"})
}
#[test]
fn legacy_start_keeps_its_wire_shape_and_restrictive_default() {
    let request: RunRequest = serde_json::from_value(json!({"order_id":"o","base_revision":1,
        "idempotency_key":"key","prompt":"Read","provider_id":"p","provider_revision":1}))
    .unwrap();
    let RunRequest::Single(single) = request else {
        panic!("legacy request changed")
    };
    assert_eq!(single.access_mode, AccessMode::ConfirmMutations);
}
#[test]
fn workflow_selection_is_explicit_and_never_supplies_authority() {
    let request: RunRequest = serde_json::from_value(workflow()).unwrap();
    let RunRequest::Workflow(request) = request else {
        panic!("workflow request misclassified")
    };
    assert_eq!(request.access_mode, AccessMode::ConfirmMutations);
    let mut empty_selection = workflow();
    empty_selection["folders"] = json!([]);
    empty_selection["mcp_tools"] = json!([]);
    assert!(serde_json::from_value::<RunRequest>(empty_selection).is_ok());
    for (name, value) in [
        ("max_parallel", json!(999)),
        (
            "folders",
            json!([{"host_path":"C:\\private","grant_all":true}]),
        ),
        ("provider_id", json!("replacement")),
        (
            "mcp_tools",
            json!([{"server_id":"server","server_revision":1,"tool_name":"read","runtime_secret":"forged"}]),
        ),
        ("scope", json!({})),
        ("grant", json!("forged")),
        ("plan", json!({})),
        ("spec", json!({})),
    ] {
        let mut body = workflow();
        body[name] = value;
        assert!(
            serde_json::from_value::<RunRequest>(body).is_err(),
            "accepted {name}"
        );
    }
    let mut unknown = workflow();
    unknown["kind"] = json!("delegated");
    assert!(serde_json::from_value::<RunRequest>(unknown).is_err());
}
#[test]
fn aggregate_schema_exposes_definitions_without_private_execution_capabilities() {
    let schema = serde_json::to_value(schemars::schema_for!(ApiContractV2)).unwrap();
    let definitions = schema["$defs"].as_object().unwrap();
    for name in [
        "CreateRun",
        "RunRequest",
        "WorkflowDefinition",
        "WorkflowRunView",
        "WorkflowPage",
    ] {
        assert!(definitions.contains_key(name), "missing {name}");
    }
    for name in [
        "WorkflowScope",
        "WorkflowPlan",
        "WorkflowClaim",
        "WorkflowPermit",
        "WorkflowProgress",
        "WorkflowDispatchPermit",
    ] {
        assert!(!definitions.contains_key(name), "exposed {name}");
    }
    assert_eq!(
        definitions["WorkflowRunRequest"]["additionalProperties"],
        false
    );
}
