use cowork_workflow_contracts::WorkflowRunApproval;
use serde_json::{Value, json};
fn approval() -> Value {
    json!({"approval":{"id":"approval","run_id":"run","tool":{"id":"call","name":"document_write","arguments":{}},"target":"local-v2","policy_revision":1,"fingerprint":"original","decision":null},"node_id":"node"})
}
#[test]
fn existing_model_role_approval_keeps_its_original_wire_shape() {
    let mut original = approval();
    original["role_id"] = json!("writer");
    let decoded: WorkflowRunApproval = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(decoded.role_id.as_deref(), Some("writer"));
    assert_eq!(serde_json::to_value(decoded).unwrap(), original);
}
#[test]
fn native_action_approval_omits_a_model_role() {
    let original = approval();
    let decoded: WorkflowRunApproval = serde_json::from_value(original.clone()).unwrap();
    assert!(decoded.role_id.is_none());
    assert_eq!(serde_json::to_value(decoded).unwrap(), original);
}
#[test]
fn optional_role_metadata_remains_typed_and_closed() {
    for (name, value) in [("role_id", json!(7)), ("grant", json!(true))] {
        let mut changed = approval();
        changed[name] = value;
        assert!(serde_json::from_value::<WorkflowRunApproval>(changed).is_err());
    }
}
