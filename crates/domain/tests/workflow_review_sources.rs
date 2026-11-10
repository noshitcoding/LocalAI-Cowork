use cowork_domain::workflows::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn definition() -> WorkflowDefinition {
    let mut d: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-native-review.json"
    ))
    .unwrap();
    let documents: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-native-documents.json"
    ))
    .unwrap();
    let mut source = documents["documents"]["read-source"].clone();
    source["budgets"] = d["budgets"].clone();
    source["rights"] = json!({"current_workspace":true,"tools":["document_read"],"folders":[]});
    d["rights"] =
        json!({"current_workspace":true,"tools":["document_read","request_input"],"folders":[]});
    d["documents"] = json!({"source":source});
    d["controls"]["review"]["dependencies"] = json!(["source"]);
    d["controls"]["review"]["operation"]["form"]["source_steps"] = json!(["source"]);
    let mut data = documents["dataflow"]["steps"]["read-source"].clone();
    data["inputs"]["path"]["value"] = json!({"source":"literal","value":"invoice.pdf"});
    data["inputs"]["sha"]["value"] = json!({"source":"literal","value":"a".repeat(64)});
    d["dataflow"]["steps"]["source"] = data;
    serde_json::from_value(d).unwrap()
}
fn missing() -> Value {
    json!({"invoice":null,"issued":null,"net_minor":null,"tax_minor":null,"gross_minor":null})
}
fn text() -> Value {
    json!({"source":{"folder_id":"original-copy","path":"invoice.pdf","sha256":"a".repeat(64)},"text":"Invoice INV-42. Total 119.00 EUR.","truncated":false,"offset":0,"next_offset":null,"diagnostics":[],"diagnostics_truncated":false})
}
fn sources(value: Value) -> BTreeMap<String, String> {
    BTreeMap::from([("source".into(), value.to_string())])
}
fn form(d: &mut WorkflowDefinition) -> &mut WorkflowReviewForm {
    match &mut d.controls.get_mut("review").unwrap().operation {
        WorkflowControlOperation::Review { form, .. } => form,
        _ => panic!("review fixture"),
    }
}

#[test]
fn selected_native_source_is_frozen_readonly_context_and_never_an_answer_or_execution_right() {
    let d = definition();
    d.validate_definition().unwrap();
    let mut completed = sources(text());
    let request = d
        .review_request("review", Some(&missing()), &completed)
        .unwrap();
    assert_eq!(request.values, missing());
    assert_eq!(request.evidence.len(), 1);
    assert_eq!(request.evidence[0].kind, WorkflowReviewEvidenceKind::Text);
    assert_eq!(request.evidence[0].output, text());
    completed.insert(
        "source".into(),
        json!({"text":"later mutation"}).to_string(),
    );
    assert_eq!(request.evidence[0].output, text());
    let mut answer = missing();
    answer["evidence"] = json!({"new_right":"write_text_file"});
    assert!(request.form.validate_answer(&answer).is_err());
    let mut granted = d.rights.clone();
    granted.current_workspace = false;
    granted.folders.insert("original-copy".into());
    let plan = WorkflowPlan::compile(
        &d,
        &WorkflowScope {
            workspace_folder: Some(WorkflowWorkspaceBinding {
                folder_id: "original-copy".into(),
                project_id: "project".into(),
                order_id: "order".into(),
            }),
            rights: granted,
            budgets: d.budgets.clone(),
            max_parallel: 1,
        },
        &[],
    )
    .unwrap();
    assert_eq!(
        plan.nodes["review"].rights.tools,
        std::collections::BTreeSet::from(["request_input".into()])
    );
    assert!(plan.nodes["review"].rights.folders.is_empty());
}

#[test]
fn missing_malformed_and_extended_document_outputs_do_not_create_a_review() {
    let d = definition();
    assert!(
        d.review_request("review", Some(&missing()), &BTreeMap::new())
            .is_err()
    );
    for bad in [
        json!({"text":"fake output"}),
        {
            let mut v = text();
            v["source"]["sha256"] = json!(1);
            v
        },
        {
            let mut v = text();
            v["extra_right"] = json!(true);
            v
        },
    ] {
        assert!(
            d.review_request("review", Some(&missing()), &sources(bad))
                .is_err()
        );
    }
}

#[test]
fn source_must_be_an_actual_document_ancestor_selected_in_the_published_definition() {
    for source in ["review", "missing"] {
        let mut d = definition();
        form(&mut d).source_steps = vec![source.into()];
        assert!(d.validate_definition().is_err());
    }
    let mut d = definition();
    d.controls.get_mut("review").unwrap().dependencies.clear();
    assert!(d.validate_definition().is_err());
    let mut d = definition();
    d.documents.remove("source");
    d.controls.get_mut("review").unwrap().dependencies.clear();
    d.dataflow.as_mut().unwrap().steps.remove("source");
    assert!(d.validate_definition().is_err());
}

#[test]
fn write_actions_and_arbitrary_json_steps_are_not_accepted_as_document_evidence() {
    let mut d = definition();
    let docs: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-native-documents.json"
    ))
    .unwrap();
    d.documents.insert(
        "source".into(),
        serde_json::from_value(docs["documents"]["write-report"].clone()).unwrap(),
    );
    d.documents.get_mut("source").unwrap().dependencies.clear();
    d.dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("source")
        .unwrap()
        .output = d.documents["source"].operation.output_type();
    assert!(d.validate_definition().is_err());
    let mut d = definition();
    d.documents.remove("source");
    d.controls.insert(
        "source".into(),
        WorkflowControlStep {
            name: "Untrusted JSON".into(),
            dependencies: vec![],
            operation: WorkflowControlOperation::Compose,
        },
    );
    assert!(d.validate_definition().is_err());
}

#[test]
fn source_lists_are_bounded_unique_and_do_not_change_legacy_serialization() {
    let mut d = definition();
    form(&mut d).source_steps = vec!["source".into(); 2];
    assert!(d.validate_definition().is_err());
    form(&mut d).source_steps = (0..5).map(|i| format!("source-{i}")).collect();
    assert!(d.validate_definition().is_err());
    let legacy: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-native-review.json"
    ))
    .unwrap();
    let request = legacy
        .review_request("review", Some(&missing()), &BTreeMap::new())
        .unwrap();
    assert!(request.evidence.is_empty());
    let serialized = serde_json::to_value(request).unwrap();
    assert!(serialized.get("evidence").is_none());
    assert!(serialized["form"].get("source_steps").is_none());
}

#[test]
fn embedded_review_maps_only_its_own_source_and_preserves_the_call_authority_boundary() {
    let child = definition();
    let mut outer = child.clone();
    outer.format_version = 4;
    outer.controls.clear();
    outer.documents.clear();
    outer.subflows.insert(
        "call".into(),
        WorkflowEmbeddedSubflow {
            name: "Invoice review with source".into(),
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
    outer.dataflow.as_mut().unwrap().steps = BTreeMap::from([(
        "call".into(),
        child.dataflow.as_ref().unwrap().steps["review"].clone(),
    )]);
    let mut granted = child.rights.clone();
    granted.current_workspace = false;
    granted.folders.insert("original-copy".into());
    let scope = WorkflowScope {
        workspace_folder: Some(WorkflowWorkspaceBinding {
            folder_id: "original-copy".into(),
            project_id: "project".into(),
            order_id: "order".into(),
        }),
        rights: granted,
        budgets: child.budgets.clone(),
        max_parallel: 1,
    };
    let plan = WorkflowPlan::compile(&outer, &scope, &[]).unwrap();
    let (review, form) = plan
        .definition
        .controls
        .iter()
        .find_map(|(id, step)| match &step.operation {
            WorkflowControlOperation::Review { form, .. } => Some((id, form)),
            _ => None,
        })
        .unwrap();
    let source = &form.source_steps[0];
    assert_ne!(source, "source");
    assert!(plan.definition.documents.contains_key(source));
    let mut completed = BTreeMap::from([(source.clone(), text().to_string())]);
    for binding in plan.definition.dataflow.as_ref().unwrap().steps[review]
        .inputs
        .values()
    {
        if let WorkflowDataSource::Step { step_id, .. } = &binding.value {
            completed.insert(step_id.clone(), missing().to_string());
        }
    }
    let request = plan
        .definition
        .review_request(review, Some(&missing()), &completed)
        .unwrap();
    assert_eq!(request.evidence[0].step_id, *source);
    assert_eq!(request.evidence[0].output, text());
    outer
        .subflows
        .get_mut("call")
        .unwrap()
        .rights
        .tools
        .remove("document_read");
    assert!(WorkflowPlan::compile(&outer, &scope, &[]).is_err());
}

#[test]
fn ocr_evidence_keeps_original_source_page_word_coordinates_and_raw_score() {
    let mut d = definition();
    d.documents.get_mut("source").unwrap().operation = WorkflowDocumentOperation::Ocr {
        path_input: "path".into(),
        sha256_input: "sha".into(),
        languages: vec!["deu".into(), "eng".into()],
        pages: vec![1],
        dpi: 200,
        rotation: 0,
        offset: 0,
        max_words: 100,
    };
    d.dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("source")
        .unwrap()
        .output = d.documents["source"].operation.output_type();
    let mut output: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-native-ocr-output.json"
    ))
    .unwrap();
    output["source"] = text()["source"].clone();
    output["diagnostics"] = json!([]);
    output["diagnostics_truncated"] = json!(false);
    let request = d
        .review_request("review", Some(&missing()), &sources(output.clone()))
        .unwrap();
    assert_eq!(request.evidence[0].kind, WorkflowReviewEvidenceKind::Ocr);
    assert_eq!(request.evidence[0].output, output);
    assert!(
        !request.evidence[0].output["words"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(request.evidence[0].output["pages"][0]["page"], 1);
}

#[test]
fn aggregate_evidence_has_a_separate_bound_and_is_never_silently_truncated() {
    let mut d = definition();
    let original = d.documents.remove("source").unwrap();
    let data = d.dataflow.as_mut().unwrap().steps.remove("source").unwrap();
    let mut completed = BTreeMap::new();
    let mut output = text();
    output["diagnostics"] = Value::Array(
        (0..64)
            .map(|_| json!({"severity":"info","message":"x".repeat(512),"file":null,"line":null}))
            .collect(),
    );
    output["text"] = json!("x".repeat(8192));
    let ids: Vec<_> = (0..4).map(|i| format!("source-{i}")).collect();
    for id in &ids {
        d.documents.insert(id.clone(), original.clone());
        d.dataflow
            .as_mut()
            .unwrap()
            .steps
            .insert(id.clone(), data.clone());
        completed.insert(id.clone(), output.to_string());
    }
    d.controls.get_mut("review").unwrap().dependencies = ids.clone();
    form(&mut d).source_steps = ids;
    d.validate_definition().unwrap();
    let error = d
        .review_request("review", Some(&missing()), &completed)
        .unwrap_err();
    assert!(
        error.to_string().contains("workflow_review_source_limit"),
        "{error}"
    );
}
