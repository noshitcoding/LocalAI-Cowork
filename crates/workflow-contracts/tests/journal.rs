use cowork_workflow_contracts::{PrepareWorkflowOperation, SaveWorkflowDraft};
use serde_json::json;

fn draft() -> serde_json::Value {
    json!({"id":"draft","base_revision":0,"idempotency_key":"save-1","content":{
        "editor_format":"cowork.workflow-editor.v1","data":{"name":"","budgets":{"max_steps":-1},"roles":[{"provider_id":""}]},
        "origin":null,"archived":false}})
}
fn start() -> serde_json::Value {
    json!({"intent_id":"intent","draft_snapshot":null,"request":{"kind":"start","request":{
        "kind":"workflow","order_id":"order","base_revision":1,"idempotency_key":"start-1",
        "prompt":"Review","workflow_id":"workflow","version_id":"version"}}})
}
#[test]
fn partial_editor_data_does_not_become_an_execution_definition() {
    let request: SaveWorkflowDraft = serde_json::from_value(draft()).unwrap();
    assert_eq!(request.content.data["budgets"]["max_steps"], -1);
    assert!(serde_json::from_value::<PrepareWorkflowOperation>(json!({
        "intent_id":"intent","request":{"kind":"create","request":{"idempotency_key":"create","definition":request.content.data}},"draft_snapshot":null
    })).is_err());
}
#[test]
fn journal_commands_reject_authority_in_every_envelope() {
    let request: PrepareWorkflowOperation = serde_json::from_value(start()).unwrap();
    assert!(matches!(
        request.request,
        cowork_workflow_contracts::WorkflowOperationRequest::Start { .. }
    ));
    for field in [
        "spec",
        "scope",
        "policy_revision",
        "grant",
        "state",
        "receipt",
    ] {
        let mut body = start();
        body[field] = json!({});
        assert!(serde_json::from_value::<PrepareWorkflowOperation>(body).is_err());
        let mut body = start();
        body["request"][field] = json!({});
        assert!(serde_json::from_value::<PrepareWorkflowOperation>(body).is_err());
        let mut body = start();
        body["request"]["request"][field] = json!({});
        assert!(serde_json::from_value::<PrepareWorkflowOperation>(body).is_err());
    }
    let mut body = start();
    body["draft_snapshot"] =
        json!({"draft_id":"d","snapshot_id":"s","sha256":"a".repeat(64),"rights":["all"]});
    assert!(serde_json::from_value::<PrepareWorkflowOperation>(body).is_err());
}
#[test]
fn save_authority_and_unknown_operation_kinds_are_rejected() {
    for field in ["project_id", "revision", "snapshot_id", "policy", "rights"] {
        let mut body = draft();
        body[field] = json!("forged");
        assert!(serde_json::from_value::<SaveWorkflowDraft>(body).is_err());
    }
    let mut body = start();
    body["request"]["kind"] = json!("execute_spec");
    assert!(serde_json::from_value::<PrepareWorkflowOperation>(body).is_err());
}
