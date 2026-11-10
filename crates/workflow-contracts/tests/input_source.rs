use cowork_contracts::Budgets;
use cowork_workflow_contracts::{WorkflowPreflight, WorkflowRunRequest};
use serde_json::json;

#[test]
fn old_workflow_request_and_preflight_bytes_omit_new_source_fields() {
    let request = json!({"kind":"workflow","order_id":"order","base_revision":1,"idempotency_key":"key","prompt":"prompt",
        "workflow_id":"workflow","version_id":"version","access_mode":"confirm_mutations","budgets":Budgets::default()});
    let parsed: WorkflowRunRequest = serde_json::from_value(request.clone()).unwrap();
    assert!(parsed.input_source.is_none());
    assert_eq!(serde_json::to_value(parsed).unwrap(), request);
    let review = json!({"ready":true,"workflow":{"workflow_id":"workflow","version_id":"version","definition_sha256":"a".repeat(64),"plan_sha256":"b".repeat(64)},
        "name":"Old immutable version","version_revision":1,"executor":"local-v2","policy_revision":1,"access_mode":"confirm_mutations",
        "budgets":Budgets::default(),"allowed_tools":[],"roles":[],"max_parallel":4});
    let parsed: WorkflowPreflight = serde_json::from_value(review.clone()).unwrap();
    assert!(parsed.input_source.is_none());
    assert_eq!(serde_json::to_value(parsed).unwrap(), review);
}
