//! Compiler/journal tests only. No engine or external IO is claimed here.
use cowork_contracts::Budgets;
use cowork_domain::{Error, canonical_hash, documents::parse_document_call, workflows::*};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn read() -> WorkflowDocumentOperation {
    WorkflowDocumentOperation::ReadText {
        path_input: "path".into(),
        sha256_input: "sha".into(),
        offset: 0,
    }
}
fn ocr() -> WorkflowDocumentOperation {
    WorkflowDocumentOperation::Ocr {
        path_input: "path".into(),
        sha256_input: "sha".into(),
        languages: vec!["deu".into(), "eng".into()],
        pages: vec![2, 1],
        dpi: 200,
        rotation: 0,
        offset: 0,
        max_words: 100,
    }
}
fn create(format: WorkflowDocumentFormat) -> WorkflowDocumentOperation {
    WorkflowDocumentOperation::Create {
        format,
        path_input: "path".into(),
        content_input: "content".into(),
    }
}
fn rights() -> WorkflowRights {
    WorkflowRights {
        current_workspace: false,
        tools: BTreeSet::from(["document_read".into(), "document_write".into()]),
        folders: BTreeSet::from(["copy-files".into()]),
    }
}
fn budgets() -> Budgets {
    Budgets {
        max_steps: 128,
        max_seconds: 180,
        max_output_tokens: 1024,
        max_total_tokens: 4096,
    }
}
fn definition(operation: WorkflowDocumentOperation) -> WorkflowDefinition {
    let path = match operation {
        WorkflowDocumentOperation::Create {
            format: WorkflowDocumentFormat::Pdf,
            ..
        } => "report.pdf",
        WorkflowDocumentOperation::Create { .. } => "report.docx",
        WorkflowDocumentOperation::Ocr { .. } => "source.pdf",
        _ => "source.docx",
    };
    let second = if matches!(operation, WorkflowDocumentOperation::Create { .. }) {
        "# A report".to_owned()
    } else {
        "a".repeat(64)
    };
    let inputs: BTreeMap<_, _> = operation
        .input_names()
        .iter()
        .zip([path.to_owned(), second])
        .map(|(name, value)| {
            (
                (*name).to_owned(),
                WorkflowValueBinding {
                    value_type: WorkflowDataType::Text,
                    value: WorkflowDataSource::Literal {
                        value: json!(value),
                    },
                },
            )
        })
        .collect();
    serde_json::from_value(json!({
        "format_version":3,"name":"Native documents","mode":{"mode":"parallel","max_parallel":2},"failure":"stop",
        "rights":rights(),"budgets":budgets(),"roles":[],"steps":[],
        "documents":{"document":{"name":"Read document","dependencies":[],"folder":{"kind":"selected_folder","folder_id":"copy-files"},"operation":operation,"rights":rights(),"budgets":budgets()}},
        "dataflow":{"input":{"type":"object","fields":{}},"steps":{"document":{"inputs":inputs,"output":operation.output_type()}}}
    })).unwrap()
}
fn scope() -> WorkflowScope {
    WorkflowScope {
        workspace_folder: None,
        rights: rights(),
        budgets: budgets(),
        max_parallel: 2,
    }
}
fn compile(d: &WorkflowDefinition) -> WorkflowPlan {
    WorkflowPlan::compile(d, &scope(), &[]).unwrap()
}
fn read_output() -> String {
    json!({"source":{"folder_id":"copy-files","path":"source.docx","sha256":"a".repeat(64)},"text":"Verified source","truncated":false,"offset":0,"next_offset":null,"diagnostics":[],"diagnostics_truncated":false}).to_string()
}

#[test]
fn documents_compile_without_a_model_account_and_never_admit_a_model_request() {
    let d = definition(read());
    d.validate_definition().unwrap();
    let plan = compile(&d);
    assert!(plan.profiles.is_empty() && plan.roles.is_empty());
    assert!(plan.nodes["document"].role_id.is_none());
    assert_eq!(
        plan.nodes["document"].rights.tools,
        BTreeSet::from(["document_read".into()])
    );
    let mut p = WorkflowProgress::new(&plan).unwrap();
    let claim = p.claim(&plan, p.revision(), "document", 100).unwrap();
    let before = canonical_hash(&p);
    assert!(matches!(
        p.admit(
            &plan,
            p.revision(),
            &claim,
            "model",
            WorkflowOperation::Model {
                request_sha256: "b".repeat(64),
                reserved_tokens: None
            },
            100
        ),
        Err(Error::Forbidden)
    ));
    assert_eq!(canonical_hash(&p), before);
    assert_eq!(p.charged_tokens(), 0);
}

#[test]
fn ocr_proposals_preserve_explicit_languages_pages_copy_and_source_hash_without_a_model() {
    let definition = definition(ocr());
    let plan = compile(&definition);
    assert!(plan.profiles.is_empty() && plan.roles.is_empty());
    assert!(plan.nodes["document"].role_id.is_none());
    assert_eq!(
        plan.nodes["document"].rights.tools,
        BTreeSet::from(["document_read".into()])
    );
    let tool = plan
        .definition
        .document_tool_call("document", Some(&json!({})), &BTreeMap::new())
        .unwrap();
    let parsed = parse_document_call(&tool).unwrap();
    assert_eq!(parsed.operation, "ocr");
    assert!(parsed.output.is_none());
    assert_eq!(parsed.inputs[0].folder_id, "copy-files");
    assert_eq!(
        parsed.inputs[0].expected_sha256.as_deref(),
        Some("a".repeat(64).as_str())
    );
    assert_eq!(parsed.options["languages"], json!(["deu", "eng"]));
    assert_eq!(parsed.options["pages"], json!([2, 1]));
    assert_eq!(parsed.options["limit"], 100);
    assert_eq!(parsed.options["dpi"], 200);
}

#[test]
fn ocr_configuration_rejects_unknown_languages_and_unbounded_or_ambiguous_values() {
    for (key, value) in [
        ("languages", json!([])),
        ("languages", json!(["fra"])),
        ("languages", json!(["eng", "eng"])),
        ("pages", json!([0])),
        ("pages", json!([1, 1])),
        ("pages", json!((1..=17).collect::<Vec<u32>>())),
        ("dpi", json!(99)),
        ("dpi", json!(401)),
        ("rotation", json!(45)),
        ("offset", json!(1_000_000_001)),
        ("max_words", json!(0)),
        ("max_words", json!(101)),
    ] {
        let mut value_definition = serde_json::to_value(definition(ocr())).unwrap();
        value_definition["documents"]["document"]["operation"][key] = value;
        let invalid: WorkflowDefinition = serde_json::from_value(value_definition).unwrap();
        assert!(
            invalid.validate_definition().is_err(),
            "accepted invalid OCR field {key}"
        );
    }
}

#[test]
fn actual_ocr_result_has_a_fixed_type_with_word_positions_and_explicit_continuations() {
    let output = ocr().output_type();
    output.validate_type().unwrap();
    let value: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-native-ocr-output.json"
    ))
    .unwrap();
    output.validate_value(&value).unwrap();
    assert!(value["text"].as_str().unwrap().contains("119,00"));
    for (pointer, bad) in [
        ("/words/0/score", json!("uncertain")),
        ("/pages/0/width", json!("unknown")),
        ("/next_offset", json!("later")),
        ("/engine/languages", json!(["eng", "deu", "fra"])),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() = bad;
        assert!(
            output.validate_value(&changed).is_err(),
            "accepted wrong OCR result field {pointer}"
        );
    }
}

#[test]
fn model_ocr_arguments_cannot_choose_programs_models_or_unrelated_read_options() {
    let base = json!({"operation":"ocr","folder_id":"copy-files","path":"source.png","expected_sha256":"a".repeat(64)});
    for (key, value) in [
        ("languages", json!(["eng --config injected"])),
        ("dpi", json!(500)),
        ("rotation", json!(1)),
        ("sheet", json!("Sheet1")),
        ("tessdata_path", json!("/host")),
        ("program", json!("foreign.exe")),
    ] {
        let mut arguments = base.clone();
        arguments[key] = value;
        assert!(
            parse_document_call(&cowork_contracts::ToolCall {
                id: "ocr".into(),
                name: "document_read".into(),
                arguments
            })
            .is_err()
        );
    }
    let mut non_ocr = base.clone();
    non_ocr["operation"] = json!("text");
    non_ocr["languages"] = json!(["eng"]);
    assert!(
        parse_document_call(&cowork_contracts::ToolCall {
            id: "read".into(),
            name: "document_read".into(),
            arguments: non_ocr
        })
        .is_err()
    );
    let mut wrong_file = base;
    wrong_file["path"] = json!("source.docx");
    assert!(
        parse_document_call(&cowork_contracts::ToolCall {
            id: "ocr".into(),
            name: "document_read".into(),
            arguments: wrong_file
        })
        .is_err()
    );
}

#[test]
fn an_unused_declared_role_does_not_require_its_missing_provider_in_a_document_graph() {
    let mut d = definition(read());
    d.roles.push(serde_json::from_value(json!({"id":"unused","instructions":"Unused saved role","provider_id":"missing-model","provider_revision":1,"rights":rights(),"budgets":budgets(),"delegate_to":[]})).unwrap());
    let plan = compile(&d);
    assert!(plan.profiles.is_empty() && plan.roles.is_empty());
    assert_eq!(plan.definition.roles.len(), 1);
    plan.validate().unwrap();
}

#[test]
fn deterministic_preparation_reserves_no_tokens_and_uses_the_existing_tool_journal() {
    let plan = compile(&definition(read()));
    let mut p = WorkflowProgress::new(&plan).unwrap();
    let claim = p.claim(&plan, p.revision(), "document", 100).unwrap();
    p.prepare_step(
        &plan,
        p.revision(),
        &claim,
        "proposal",
        &"a".repeat(64),
        &"b".repeat(64),
        100,
    )
    .unwrap();
    assert_eq!(p.attempts()["proposal"].reserved_tokens, 0);
    let admission = p
        .admit(
            &plan,
            p.revision(),
            &claim,
            "read",
            WorkflowOperation::Tool {
                tool_id: "document_read".into(),
                arguments_sha256: "c".repeat(64),
            },
            100,
        )
        .unwrap();
    drop(admission);
    p.settle(
        &plan,
        p.revision(),
        &claim,
        "read",
        WorkflowOutcome::Succeeded,
        100,
    )
    .unwrap();
    p.complete(&plan, p.revision(), &claim, &read_output(), 100)
        .unwrap();
    assert_eq!(p.state(), WorkflowState::Completed);
    assert_eq!(p.charged_tokens(), 0);
    let restored: WorkflowProgress =
        serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
    restored.validate(&plan).unwrap();
}

#[test]
fn an_unsettled_document_effect_cannot_complete_and_replay_does_not_replace_it() {
    let plan = compile(&definition(read()));
    let mut p = WorkflowProgress::new(&plan).unwrap();
    let claim = p.claim(&plan, p.revision(), "document", 100).unwrap();
    drop(
        p.admit(
            &plan,
            p.revision(),
            &claim,
            "read",
            WorkflowOperation::Tool {
                tool_id: "document_read".into(),
                arguments_sha256: "c".repeat(64),
            },
            100,
        )
        .unwrap(),
    );
    assert!(
        p.complete(&plan, p.revision(), &claim, &read_output(), 100)
            .is_err()
    );
    p.settle(
        &plan,
        p.revision(),
        &claim,
        "read",
        WorkflowOutcome::Uncertain,
        100,
    )
    .unwrap();
    let restored: WorkflowProgress =
        serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
    restored.validate(&plan).unwrap();
    assert_eq!(restored.state(), WorkflowState::Interrupted);
    assert_eq!(restored.attempts().len(), 1);
}

#[test]
fn exact_native_operations_form_valid_engine_calls_without_a_model_response() {
    for op in [
        read(),
        WorkflowDocumentOperation::Preview {
            path_input: "path".into(),
            sha256_input: "sha".into(),
            pages: vec![1, 3],
        },
        create(WorkflowDocumentFormat::Docx),
        create(WorkflowDocumentFormat::Pdf),
    ] {
        let plan = compile(&definition(op.clone()));
        let tool = plan
            .definition
            .document_tool_call("document", Some(&json!({})), &BTreeMap::new())
            .unwrap();
        let parsed = parse_document_call(&tool).unwrap();
        assert_eq!(tool.name, op.tool_name());
        assert!(
            parsed
                .inputs
                .iter()
                .all(|source| source.folder_id == "copy-files"
                    && source.expected_sha256.as_ref().unwrap().len() == 64)
        );
        if let Some(output) = parsed.output {
            assert_eq!(output.folder_id.as_deref(), Some("copy-files"));
        }
    }
}

#[test]
fn unbound_workspace_selection_is_only_data_and_cannot_form_a_dispatch_proposal() {
    let mut d = definition(read());
    let step = d.documents.get_mut("document").unwrap();
    step.folder = WorkflowDocumentFolder::CurrentWorkspace;
    step.rights.current_workspace = true;
    assert!(matches!(
        step.tool_call(&json!({"path":"source.docx","sha":"a".repeat(64)})),
        Err(Error::Forbidden)
    ));
}

#[test]
fn source_hash_is_portable_but_each_compiled_folder_is_explicitly_bound() {
    let mut d = definition(read());
    d.rights.current_workspace = true;
    let step = d.documents.get_mut("document").unwrap();
    step.folder = WorkflowDocumentFolder::CurrentWorkspace;
    step.rights.current_workspace = true;
    step.rights.folders.clear();
    let original = serde_json::to_vec(&d).unwrap();
    let mut hashes = vec![];
    for folder in ["copy-a", "copy-b"] {
        let mut local = scope();
        local.rights.folders.insert(folder.into());
        local.workspace_folder = Some(WorkflowWorkspaceBinding {
            folder_id: folder.into(),
            project_id: "project".into(),
            order_id: folder.into(),
        });
        let plan = WorkflowPlan::compile(&d, &local, &[]).unwrap();
        assert_eq!(plan.source_sha256, canonical_hash(&d));
        assert_eq!(
            plan.definition
                .document_tool_call("document", Some(&json!({})), &BTreeMap::new())
                .unwrap()
                .arguments["folder_id"],
            folder
        );
        hashes.push(plan.sha256);
    }
    assert_ne!(hashes[0], hashes[1]);
    assert_eq!(serde_json::to_vec(&d).unwrap(), original);
    assert!(WorkflowPlan::compile(&d, &scope(), &[]).is_err());
}

#[test]
fn final_tool_and_folder_intersections_cannot_be_widened_by_a_document_node() {
    let d = definition(read());
    for field in ["tool", "folder"] {
        let mut selected = scope();
        if field == "tool" {
            selected.rights.tools.clear();
        } else {
            selected.rights.folders.clear();
        }
        assert!(matches!(
            WorkflowPlan::compile(&d, &selected, &[]),
            Err(Error::Forbidden)
        ));
    }
    let mut wrong = d.clone();
    wrong.documents.get_mut("document").unwrap().folder = WorkflowDocumentFolder::SelectedFolder {
        folder_id: "foreign-folder".into(),
    };
    assert!(wrong.validate_definition().is_err());
}

#[test]
fn malformed_hashes_paths_frame_keys_and_native_options_are_rejected() {
    let d = definition(read());
    let step = &d.documents["document"];
    for frame in [
        json!({"path":"../secret.docx","sha":"a".repeat(64)}),
        json!({"path":"source.docx","sha":"wrong"}),
        json!({"path":"source.docx","sha":"A".repeat(64)}),
        json!({"path":"source.docx","sha":"a".repeat(64),"folder_id":"foreign"}),
        json!({"path":"source.docx","sha":1}),
    ] {
        assert!(step.tool_call(&frame).is_err(), "{frame}");
    }
    for op in [
        WorkflowDocumentOperation::ReadText {
            path_input: "path".into(),
            sha256_input: "sha".into(),
            offset: 1_000_000_001,
        },
        WorkflowDocumentOperation::ReadText {
            path_input: "path".into(),
            sha256_input: "sha".into(),
            offset: u32::MAX,
        },
        WorkflowDocumentOperation::Preview {
            path_input: "path".into(),
            sha256_input: "sha".into(),
            pages: vec![1, 1],
        },
        WorkflowDocumentOperation::Preview {
            path_input: "path".into(),
            sha256_input: "sha".into(),
            pages: vec![0],
        },
        WorkflowDocumentOperation::Preview {
            path_input: "path".into(),
            sha256_input: "sha".into(),
            pages: (1..=17).collect(),
        },
    ] {
        assert!(definition(op).validate_definition().is_err());
    }
}

#[test]
fn a_pdf_action_cannot_silently_write_a_docx_extension() {
    let d = definition(create(WorkflowDocumentFormat::Pdf));
    d.documents["document"]
        .tool_call(&json!({"path":"REPORT.PDF","content":"# Text"}))
        .unwrap();
    assert!(
        d.documents["document"]
            .tool_call(&json!({"path":"report.docx","content":"# Text"}))
            .is_err()
    );
}

#[test]
fn schema_and_dag_limits_apply_to_document_nodes() {
    for change in [
        "input-type",
        "output",
        "dependency",
        "collision",
        "mode",
        "old-format",
        "aliased-input",
    ] {
        let mut d = definition(read());
        match change {
            "input-type" => {
                d.dataflow
                    .as_mut()
                    .unwrap()
                    .steps
                    .get_mut("document")
                    .unwrap()
                    .inputs
                    .get_mut("path")
                    .unwrap()
                    .value_type = WorkflowDataType::Integer
            }
            "output" => {
                d.dataflow
                    .as_mut()
                    .unwrap()
                    .steps
                    .get_mut("document")
                    .unwrap()
                    .output = WorkflowDataType::Text
            }
            "dependency" => d
                .documents
                .get_mut("document")
                .unwrap()
                .dependencies
                .push("document".into()),
            "collision" => {
                d.controls.insert(
                    "document".into(),
                    WorkflowControlStep {
                        name: "Other".into(),
                        dependencies: vec![],
                        operation: WorkflowControlOperation::Compose,
                    },
                );
            }
            "mode" => d.mode = WorkflowMode::Sequential,
            "old-format" => d.format_version = 2,
            _ => {
                d.documents.get_mut("document").unwrap().operation =
                    WorkflowDocumentOperation::ReadText {
                        path_input: "path".into(),
                        sha256_input: "path".into(),
                        offset: 0,
                    }
            }
        }
        assert!(d.validate_definition().is_err(), "{change}");
    }
    let mut d = definition(read());
    let step = d.documents.remove("document").unwrap();
    let frame = d
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .remove("document")
        .unwrap();
    for index in 0..65 {
        let id = format!("doc-{index}");
        d.documents.insert(id.clone(), step.clone());
        d.dataflow.as_mut().unwrap().steps.insert(id, frame.clone());
    }
    assert!(d.validate_definition().is_err());
}

#[test]
fn explicit_typed_workflow_inputs_reach_only_the_native_operation_fields() {
    let mut d = definition(read());
    let data = d.dataflow.as_mut().unwrap();
    data.input = WorkflowDataType::Object {
        fields: BTreeMap::from([
            ("file".into(), WorkflowDataType::Text),
            ("hash".into(), WorkflowDataType::Text),
            ("unbound".into(), WorkflowDataType::Text),
        ]),
    };
    data.steps
        .get_mut("document")
        .unwrap()
        .inputs
        .get_mut("path")
        .unwrap()
        .value = WorkflowDataSource::Input {
        pointer: "/file".into(),
    };
    data.steps
        .get_mut("document")
        .unwrap()
        .inputs
        .get_mut("sha")
        .unwrap()
        .value = WorkflowDataSource::Input {
        pointer: "/hash".into(),
    };
    let plan = compile(&d);
    let tool = plan
        .definition
        .document_tool_call(
            "document",
            Some(&json!({"file":"fresh.docx","hash":"b".repeat(64),"unbound":"DO NOT PROPAGATE"})),
            &BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(tool.arguments["path"], "fresh.docx");
    assert_eq!(tool.arguments["expected_sha256"], "b".repeat(64));
    assert!(!tool.arguments.to_string().contains("DO NOT PROPAGATE"));
}

#[test]
fn original_workflow_json_and_plan_hashes_stay_unchanged_without_document_nodes() {
    let original: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-control.json"
    ))
    .unwrap();
    let d: WorkflowDefinition = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(&d).unwrap(), original);
    let original: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-plan-before-composition.json"
    ))
    .unwrap();
    let plan: WorkflowPlan = serde_json::from_value(original.clone()).unwrap();
    plan.validate().unwrap();
    assert_eq!(serde_json::to_value(&plan).unwrap(), original);
    assert_eq!(
        plan.sha256,
        "843bb38c743928c7b5d24804bd53251114620784b7955fd275495cf95ef0a800"
    );
}

#[test]
fn frozen_subflows_keep_document_operations_and_intersect_their_budget() {
    let child = definition(read());
    let mut outer = child.clone();
    outer.name = "Subflow owner".into();
    outer.format_version = 4;
    outer.documents.clear();
    outer.dataflow.as_mut().unwrap().steps.clear();
    outer.dataflow.as_mut().unwrap().steps.insert(
        "child".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: BTreeMap::new(),
            output: read().output_type(),
        },
    );
    let mut limits = budgets();
    limits.max_steps = 12;
    outer.subflows.insert(
        "child".into(),
        WorkflowEmbeddedSubflow {
            name: "Frozen document reader".into(),
            dependencies: vec![],
            definition: Box::new(child),
            result: WorkflowOutputRef {
                step_id: "document".into(),
                pointer: "".into(),
            },
            rights: rights(),
            budgets: limits,
        },
    );
    let plan = compile(&outer);
    plan.validate().unwrap();
    assert!(plan.roles.is_empty() && plan.profiles.is_empty());
    assert_eq!(plan.definition.documents.len(), 1);
    let (id, _) = plan.definition.documents.iter().next().unwrap();
    assert!(id.starts_with("composed-"));
    assert_eq!(plan.nodes[id].budgets.max_steps, 12);
    assert_eq!(
        plan.definition
            .document_tool_call(id, Some(&json!({})), &BTreeMap::new())
            .unwrap()
            .arguments["path"],
        "source.docx"
    );
}

#[test]
fn a_root_document_and_a_subflow_with_the_same_local_name_are_both_kept() {
    let mut outer = definition(read());
    let child = definition(read());
    outer.format_version = 4;
    outer.dataflow.as_mut().unwrap().steps.insert(
        "child".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: BTreeMap::new(),
            output: read().output_type(),
        },
    );
    outer.subflows.insert(
        "child".into(),
        WorkflowEmbeddedSubflow {
            name: "After root document".into(),
            dependencies: vec!["document".into()],
            definition: Box::new(child),
            result: WorkflowOutputRef {
                step_id: "document".into(),
                pointer: "".into(),
            },
            rights: rights(),
            budgets: budgets(),
        },
    );
    let plan = compile(&outer);
    assert_eq!(plan.definition.documents.len(), 2);
    assert!(plan.definition.documents.contains_key("document"));
    assert_eq!(plan.source_sha256, canonical_hash(&outer));
    plan.validate().unwrap();
}

#[test]
fn list_instances_keep_separate_bound_document_inputs_and_no_model_roles() {
    let mut body = definition(create(WorkflowDocumentFormat::Docx));
    body.roles.push(serde_json::from_value(json!({"id":"unused","instructions":"Unused model configuration","provider_id":"not-configured","provider_revision":1,"rights":rights(),"budgets":budgets(),"delegate_to":[]})).unwrap());
    let input = WorkflowDataType::Object {
        fields: BTreeMap::from([
            ("path".into(), WorkflowDataType::Text),
            ("content".into(), WorkflowDataType::Text),
        ]),
    };
    body.dataflow.as_mut().unwrap().input = input.clone();
    for name in ["path", "content"] {
        body.dataflow
            .as_mut()
            .unwrap()
            .steps
            .get_mut("document")
            .unwrap()
            .inputs
            .get_mut(name)
            .unwrap()
            .value = WorkflowDataSource::Input {
            pointer: format!("/{name}"),
        };
    }
    let collection = WorkflowDataType::List {
        item: Box::new(input),
        max_items: 2,
    };
    let mut outer = body.clone();
    outer.format_version = 5;
    outer.documents.clear();
    outer.dataflow.as_mut().unwrap().input = WorkflowDataType::Object {
        fields: BTreeMap::from([("values".into(), collection.clone())]),
    };
    outer.dataflow.as_mut().unwrap().steps = BTreeMap::from([(
        "loop".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: BTreeMap::from([(
                "items".into(),
                WorkflowValueBinding {
                    value_type: collection,
                    value: WorkflowDataSource::Input {
                        pointer: "/values".into(),
                    },
                },
            )]),
            output: WorkflowDataType::List {
                item: Box::new(create(WorkflowDocumentFormat::Docx).output_type()),
                max_items: 2,
            },
        },
    )]);
    outer.iterations.insert(
        "loop".into(),
        WorkflowEmbeddedIteration {
            name: "Reports".into(),
            dependencies: vec![],
            definition: Box::new(body),
            collection_input: "items".into(),
            arguments: BTreeMap::from([
                (
                    "path".into(),
                    WorkflowIterationArgument::Item {
                        pointer: "/path".into(),
                    },
                ),
                (
                    "content".into(),
                    WorkflowIterationArgument::Item {
                        pointer: "/content".into(),
                    },
                ),
            ]),
            result: WorkflowOutputRef {
                step_id: "document".into(),
                pointer: "".into(),
            },
            rights: rights(),
            budgets: budgets(),
            max_parallel: 2,
        },
    );
    let plan = compile(&outer);
    let input = json!({"values":[{"path":"one.docx","content":"First bound report"},{"path":"two.docx","content":"Second bound report"}]});
    let mut p = WorkflowProgress::new(&plan).unwrap();
    for attempt in 0..8 {
        let ready = p.ready_nodes(&plan).unwrap();
        let mut advanced = false;
        for id in ready {
            if p.requires_native(&plan, &id).unwrap() {
                let claim = p.claim(&plan, p.revision(), &id, 100).unwrap();
                p.complete_control(
                    &plan,
                    p.revision(),
                    &claim,
                    &format!("control-{attempt}-{id}"),
                    Some(&input),
                    100,
                )
                .unwrap();
                advanced = true;
            }
        }
        if !advanced {
            break;
        }
    }
    let graph = p.execution_graph(&plan).unwrap();
    assert_eq!(graph.definition.documents.len(), 2);
    assert!(plan.profiles.is_empty() && plan.roles.is_empty());
    let completed = p
        .nodes()
        .iter()
        .filter_map(|(id, node)| match &node.state {
            WorkflowNodeState::Completed { output, .. } => Some((id.clone(), output.clone())),
            _ => None,
        })
        .collect();
    let mut proposals: Vec<_> = graph
        .definition
        .documents
        .keys()
        .map(|id| {
            assert!(graph.nodes[id].role_id.is_none());
            graph
                .definition
                .document_tool_call(id, Some(&input), &completed)
                .unwrap()
                .arguments
        })
        .collect();
    proposals.sort_by_key(|args| args["output"]["path"].as_str().unwrap().to_owned());
    assert_eq!(proposals[0]["output"]["path"], "one.docx");
    assert_eq!(proposals[0]["spec"]["markdown"], "First bound report");
    assert_eq!(proposals[1]["output"]["path"], "two.docx");
    assert_eq!(proposals[1]["spec"]["markdown"], "Second bound report");
    assert_eq!(p.charged_tokens(), 0);
}
