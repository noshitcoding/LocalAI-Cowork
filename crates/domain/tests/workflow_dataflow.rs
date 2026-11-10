use cowork_contracts::*;
use cowork_domain::{canonical_hash, workflows::*};
use serde_json::json;
use std::collections::BTreeMap;

fn definition() -> WorkflowDefinition {
    let limits = Budgets::default();
    serde_json::from_value(json!({
        "format_version":2,"name":"Typed invoice","mode":{"mode":"parallel","max_parallel":2},"failure":"stop",
        "rights":{"tools":[],"folders":[]},"budgets":limits,
        "roles":[{"id":"reader","instructions":"Read the declared input fields and return the declared result.","provider_id":"test","provider_revision":1,"rights":{"tools":[],"folders":[]},"budgets":limits,"delegate_to":[]}],
        "steps":[
            {"id":"extract","role_id":"reader","goal":"Extract invoice details","dependencies":[],"rights":{"tools":[],"folders":[]},"budgets":limits},
            {"id":"report","role_id":"reader","goal":"Report the checked total","dependencies":["extract"],"rights":{"tools":[],"folders":[]},"budgets":limits}
        ],
        "dataflow":{
            "input":{"type":"object","fields":{"file":{"type":"text"},"tags":{"type":"list","item":{"type":"text"},"max_items":5}}},
            "steps":{
                "extract":{"inputs":{
                    "file":{"value_type":{"type":"text"},"value":{"source":"input","pointer":"/file"}},
                    "country":{"value_type":{"type":"text"},"value":{"source":"literal","value":"DE"}}
                },"output":{"type":"object","fields":{"total":{"type":"number"},"approved":{"type":"boolean"}}}},
                "report":{"inputs":{
                    "amount":{"value_type":{"type":"number"},"value":{"source":"step","step_id":"extract","pointer":"/total"}}
                },"output":{"type":"object","fields":{"summary":{"type":"text"}}}}
            }
        }
    })).unwrap()
}
fn compile(definition: &WorkflowDefinition) -> WorkflowPlan {
    let profile: ProviderProfile=serde_json::from_value(json!({"id":"test","name":"test","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_ready":true,"credential_required":false})).unwrap();
    WorkflowPlan::compile(
        definition,
        &WorkflowScope {
            workspace_folder: None,
            rights: WorkflowRights::default(),
            budgets: Budgets::default(),
            max_parallel: 2,
        },
        &[profile],
    )
    .unwrap()
}

#[test]
fn published_types_resolve_only_explicit_fields_without_coercion() {
    let definition = definition();
    compile(&definition).validate().unwrap();
    let input = json!({"file":"invoice.pdf","tags":["pending"]});
    assert_eq!(
        definition
            .resolve_step_data("extract", Some(&input), &BTreeMap::new())
            .unwrap(),
        json!({"file":"invoice.pdf","country":"DE"})
    );
    let completed = BTreeMap::from([(
        "extract".into(),
        json!({"total":42.5,"approved":true}).to_string(),
    )]);
    assert_eq!(
        definition
            .resolve_step_data("report", Some(&input), &completed)
            .unwrap(),
        json!({"amount":42.5})
    );
    assert!(
        definition
            .validate_data_input(Some(&json!({"file":42,"tags":[]})))
            .is_err()
    );
    assert!(
        definition
            .validate_data_input(Some(
                &json!({"file":"invoice.pdf","tags":[],"extra":"undeclared"})
            ))
            .is_err()
    );
    assert!(definition.validate_data_input(None).is_err());
}

#[test]
fn publication_rejects_unavailable_or_wrongly_typed_sources() {
    let source = serde_json::to_value(definition()).unwrap();
    for (pointer, value) in [
        (
            "/dataflow/steps/report/inputs/amount/value/pointer",
            json!("/missing"),
        ),
        (
            "/dataflow/steps/report/inputs/amount/value/step_id",
            json!("report"),
        ),
        (
            "/dataflow/steps/report/inputs/amount/value_type",
            json!({"type":"text"}),
        ),
        ("/steps/1/dependencies", json!([])),
        (
            "/dataflow/steps/extract/inputs/country/value/value",
            json!(false),
        ),
        ("/dataflow/input/fields/tags/max_items", json!(0)),
    ] {
        let mut bad = source.clone();
        *bad.pointer_mut(pointer).unwrap() = value;
        let bad: WorkflowDefinition = serde_json::from_value(bad).unwrap();
        assert!(bad.validate_definition().is_err(), "{pointer}");
    }
}

#[test]
fn nullable_lists_and_safe_integer_limits_match_generated_schemas() {
    let kind:WorkflowDataType=serde_json::from_value(json!({"type":"object","fields":{
        "count":{"type":"integer"},"items":{"type":"list","item":{"type":"nullable","value":{"type":"text"}},"max_items":2}
    }})).unwrap();
    let output = StructuredOutput {
        name: "result".into(),
        schema: kind.schema(),
    };
    for value in [
        json!({"count":1.0,"items":["text",null]}),
        json!({"count":0,"items":[]}),
    ] {
        kind.validate_value(&value).unwrap();
        cowork_domain::structured_output::validate_result(&output, &value.to_string()).unwrap();
    }
    for value in [
        json!({"count":1.5,"items":[]}),
        json!({"count":9007199254740992u64,"items":[]}),
        json!({"count":0,"items":[null,null,null]}),
        json!({"count":0,"items":["\0"]}),
    ] {
        assert!(kind.validate_value(&value).is_err());
        assert!(
            cowork_domain::structured_output::validate_result(&output, &value.to_string()).is_err()
        );
    }
}

#[test]
fn list_paths_are_checked_against_actual_data_before_dispatch() {
    let mut definition = definition();
    let binding = definition
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("extract")
        .unwrap()
        .inputs
        .get_mut("file")
        .unwrap();
    binding.value = WorkflowDataSource::Input {
        pointer: "/tags/1".into(),
    };
    definition.validate_definition().unwrap();
    assert!(
        definition
            .resolve_step_data(
                "extract",
                Some(&json!({"file":"file.pdf","tags":["short"]})),
                &BTreeMap::new()
            )
            .is_err()
    );
    for pointer in [
        "/tags/01", "/tags/+1", "/tags/-", "/tags/5", "/tags/~2", "tags/1",
    ] {
        assert!(
            definition
                .dataflow
                .as_ref()
                .unwrap()
                .input
                .at_pointer(pointer)
                .is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn type_size_and_payload_bounds_are_independent_of_model_context() {
    let kind = WorkflowDataType::Text;
    assert!(kind.validate_value(&json!("x".repeat(16385))).is_err());
    let mut deep = kind;
    for _ in 0..10 {
        deep = WorkflowDataType::List {
            item: Box::new(deep),
            max_items: 1000,
        };
    }
    assert!(deep.validate_type().is_err());
    let mut definition = definition();
    definition.dataflow.as_mut().unwrap().input = WorkflowDataType::Object {
        fields: BTreeMap::from([("__proto__".into(), WorkflowDataType::Text)]),
    };
    assert!(definition.validate_definition().is_err());
}

#[test]
fn malformed_model_results_cannot_complete_even_through_domain_commands() {
    let plan = compile(&definition());
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let claim = progress
        .claim(&plan, progress.revision(), "extract", 1)
        .unwrap();
    for invalid in [
        "plain text",
        r#"{"total":"42.5","approved":true}"#,
        r#"{"total":42.5}"#,
    ] {
        assert!(
            progress
                .complete(&plan, progress.revision(), &claim, invalid, 2)
                .is_err()
        );
    }
    progress
        .complete(
            &plan,
            progress.revision(),
            &claim,
            r#"{"total":42.5,"approved":true}"#,
            2,
        )
        .unwrap();
    progress.validate(&plan).unwrap();
}

#[test]
fn legacy_serialization_stays_unchanged_and_new_format_requires_declarations() {
    let mut legacy = definition();
    legacy.format_version = 1;
    legacy.dataflow = None;
    legacy.validate_definition().unwrap();
    let wire = serde_json::to_value(&legacy).unwrap();
    assert!(wire.get("dataflow").is_none());
    let mut old_wire = wire.clone();
    old_wire.as_object_mut().unwrap().remove("dataflow");
    assert_eq!(canonical_hash(&legacy), canonical_hash(&old_wire));
    let binding = WorkflowBinding {
        inputs: None,
        workflow_id: "w".into(),
        version_id: "v".into(),
        definition_sha256: "d".into(),
        plan_sha256: "p".into(),
    };
    assert_eq!(
        serde_json::to_value(&binding).unwrap(),
        json!({"workflow_id":"w","version_id":"v","definition_sha256":"d","plan_sha256":"p"})
    );
    legacy.format_version = 2;
    assert!(legacy.validate_definition().is_err());
    let mut bad = definition();
    bad.format_version = 1;
    assert!(bad.validate_definition().is_err());
}

#[test]
fn native_preparation_is_one_replayable_zero_token_decision() {
    let mut native = definition();
    native.format_version = 1;
    native.dataflow = None;
    native.steps.truncate(1);
    native.rights.tools.insert("browser".into());
    native.roles[0].rights.tools.insert("browser".into());
    native.steps[0].rights.tools.insert("browser".into());
    native.steps[0].execution = Some(WorkflowStepExecution::WebFetch {
        url: "https://example.invalid/fixture".into(),
    });
    let plan = compile(&native);
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let claim = progress.claim(&plan, 0, "extract", 1).unwrap();
    let before = progress.revision();
    progress
        .prepare_step(
            &plan,
            before,
            &claim,
            "native-proposal",
            &"a".repeat(64),
            &"b".repeat(64),
            2,
        )
        .unwrap();
    assert_eq!(progress.revision(), before + 1);
    assert_eq!(progress.charged_tokens(), 0);
    assert_eq!(progress.attempts().len(), 1);
    assert_eq!(
        progress.attempts()["native-proposal"].outcome,
        Some(WorkflowOutcome::Succeeded)
    );
    let wire = serde_json::to_value(&progress).unwrap();
    assert_eq!(wire["format_version"], 6);
    serde_json::from_value::<WorkflowProgress>(wire)
        .unwrap()
        .validate(&plan)
        .unwrap();
}

#[test]
fn native_preparation_keeps_compatible_digests_without_granting_model_context() {
    let profile:ProviderProfile=serde_json::from_value(json!({"id":"test","name":"test","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_ready":true,"credential_required":false})).unwrap();
    let mut spec:RunSpec=serde_json::from_value(json!({"order_id":"o","project_id":"p","prompt":"fixture","provider":profile,"access_mode":"read_only","executor":"local-v2","policy_revision":1,"allowed_tools":[],"budgets":{"max_steps":8,"max_seconds":60,"max_output_tokens":256,"max_total_tokens":65536},"folders":[]})).unwrap();
    let messages = vec![json!({"role":"user","content":"short"})];
    let tools = json!([]);
    assert_eq!(
        cowork_domain::model_request_fingerprint(&spec, &messages, &tools).unwrap(),
        cowork_domain::workflow_preparation_fingerprint(&spec, &messages, &tools).unwrap()
    );
    spec.provider.as_mut().unwrap().context_tokens = 1024;
    let messages = vec![json!({"role":"user","content":"x".repeat(4096)})];
    assert!(cowork_domain::model_request_fingerprint(&spec, &messages, &tools).is_err());
    assert!(cowork_domain::workflow_preparation_fingerprint(&spec, &messages, &tools).is_ok());
    let oversized = vec![json!({"role":"user","content":"x".repeat(2*1024*1024)})];
    assert!(cowork_domain::workflow_preparation_fingerprint(&spec, &oversized, &tools).is_err());
}
