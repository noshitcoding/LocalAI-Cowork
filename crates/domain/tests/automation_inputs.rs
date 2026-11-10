use cowork_contracts::*;
use cowork_domain::{automation::*, workflows::*};
use serde_json::{Value, json};

fn definition() -> WorkflowDefinition {
    let mut definition: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-dataflow.json"
    ))
    .unwrap();
    definition.dataflow.as_mut().unwrap().input = serde_json::from_value(json!({"type":"object","fields":{
        "invoice":{"type":"object","fields":{"total":{"type":"number"},"vendor":{"type":"text"}}},
        "currency":{"type":"text"},"note":{"type":"nullable","value":{"type":"text"}},
        "items":{"type":"list","max_items":5,"item":{"type":"object","fields":{"sku":{"type":"text"}}}}
    }})).unwrap();
    definition
}
fn mapping(bindings: Value) -> AutomationWorkflowInputs {
    serde_json::from_value(json!({"format_version":1,"bindings":bindings})).unwrap()
}
fn saved() -> Value {
    json!({"invoice":{"total":0,"vendor":"Fixed"},"currency":"EUR","note":null,"items":[{"sku":"old"}]})
}

#[test]
fn event_projection_preserves_types_fixed_values_and_explicit_null() {
    let map = mapping(
        json!({"/invoice/total":"/amount","/note":"/optional","/invoice/vendor":"/weird~1key~0"}),
    );
    let result = resolve_workflow_inputs(&map, &definition(), Some(&saved()), &json!({"amount":42.5,"optional":null,"weird/key~":"Acme","currency":"USD","native_tools":["execute_program"]})).unwrap();
    assert_eq!(
        result,
        json!({"invoice":{"total":42.5,"vendor":"Acme"},"currency":"EUR","note":null,"items":[{"sku":"old"}]})
    );
    assert!(
        resolve_workflow_inputs(
            &map,
            &definition(),
            Some(&saved()),
            &json!({"amount":"42.5","optional":null,"weird/key~":"Acme"})
        )
        .is_err()
    );
    assert!(
        resolve_workflow_inputs(
            &map,
            &definition(),
            Some(&saved()),
            &json!({"amount":42.5,"weird/key~":"Acme"})
        )
        .is_err()
    );
}
#[test]
fn nested_objects_can_be_assembled_without_inventing_unmapped_required_values() {
    let map = mapping(
        json!({"/invoice/total":"/amount","/invoice/vendor":"/vendor","/currency":"/currency","/note":"/note","/items":"/items"}),
    );
    let event =
        json!({"amount":42.5,"vendor":"Acme","currency":"EUR","note":null,"items":[{"sku":"A"} ]});
    let result = resolve_workflow_inputs(&map, &definition(), None, &event).unwrap();
    assert_eq!(result["invoice"]["total"], 42.5);
    let incomplete = mapping(json!({"/invoice/total":"/amount"}));
    assert!(resolve_workflow_inputs(&incomplete, &definition(), None, &event).is_err());
}
#[test]
fn mapping_rejects_overlap_unknown_targets_malformed_pointers_and_future_formats() {
    for bindings in [
        json!({"":"","/currency":"/currency"}),
        json!({"/invoice":"/invoice","/invoice/total":"/amount"}),
        json!({"/missing":"/amount"}),
        json!({"/currency":"/bad~2escape"}),
        json!({"/currency":"relative"}),
        json!({"/items/00/sku":"/sku"}),
    ] {
        assert!(validate_input_bindings(&mapping(bindings), &definition()).is_err());
    }
    let mut map = mapping(json!({}));
    map.format_version = 2;
    assert!(validate_input_bindings(&map, &definition()).is_err());
    let mut old = definition();
    old.dataflow = None;
    old.format_version = 1;
    assert!(validate_input_bindings(&mapping(json!({})), &old).is_err());
}
#[test]
fn whole_lists_and_existing_positions_work_without_implicit_list_growth() {
    let map = mapping(json!({"/items/0/sku":"/sku"}));
    assert_eq!(
        resolve_workflow_inputs(&map, &definition(), Some(&saved()), &json!({"sku":"new"}))
            .unwrap()["items"][0]["sku"],
        "new"
    );
    let mut empty = saved();
    empty["items"] = json!([]);
    assert!(
        resolve_workflow_inputs(&map, &definition(), Some(&empty), &json!({"sku":"new"})).is_err()
    );
    let map = mapping(json!({"/items":"/list"}));
    assert_eq!(
        resolve_workflow_inputs(
            &map,
            &definition(),
            Some(&empty),
            &json!({"list":[{"sku":"A"},{"sku":"B"}]})
        )
        .unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        resolve_workflow_inputs(
            &map,
            &definition(),
            Some(&empty),
            &json!({"list":[{"sku":4}]})
        )
        .is_err()
    );
    let root = mapping(json!({"":""}));
    assert_eq!(
        resolve_workflow_inputs(&root, &definition(), None, &saved()).unwrap(),
        saved()
    );
    assert_eq!(
        resolve_workflow_inputs(
            &mapping(json!({})),
            &definition(),
            Some(&saved()),
            &json!({"currency":"USD"})
        )
        .unwrap(),
        saved()
    );
}
#[test]
fn legacy_event_trigger_preview_bytes_omit_new_optional_fields() {
    let trigger = json!({"id":"t","project_id":"p","name":"Name","revision":1,"task_id":"task","task_revision":1,"source":"manual","predicates":[],"enabled":true,"created_at":1,"updated_at":1});
    let typed: AutomationTrigger = serde_json::from_value(trigger.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), trigger);
    let event = json!({"id":"e","project_id":"p","trigger_id":"t","trigger_revision":1,"task_id":"task","task_revision":1,"event_id":"source","source":"manual","data":{},"state":"queued","run_id":null,"reason":null,"received_at":1,"updated_at":1});
    let typed: AutomationEvent = serde_json::from_value(event.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), event);
    let preview = json!({"matches":true,"would_queue":true,"reason":null});
    let typed: AutomationEventPreview = serde_json::from_value(preview.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), preview);
}
