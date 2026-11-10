use cowork_contracts::Budgets;
use cowork_domain::{canonical_hash, workflows::*};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn source() -> WorkflowIterationSource {
    let mut body: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-subflow.json"
    ))
    .unwrap();
    body = *body.subflows.remove("invoke").unwrap().definition;
    body.rights = WorkflowRights::default();
    let input = json!({"type":"object","fields":{
        "amount":{"type":"integer"},"position":{"type":"integer"},"currency":{"type":"text"},"factor":{"type":"number"}
    }});
    body.dataflow.as_mut().unwrap().input = serde_json::from_value(input.clone()).unwrap();
    let fields = ["amount", "position", "currency", "factor"];
    body.dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("echo")
        .unwrap()
        .inputs = fields
        .into_iter()
        .map(|field| {
            (
                field.into(),
                WorkflowValueBinding {
                    value_type: body
                        .dataflow
                        .as_ref()
                        .unwrap()
                        .input
                        .at_pointer(&format!("/{field}"))
                        .unwrap()
                        .clone(),
                    value: WorkflowDataSource::Input {
                        pointer: format!("/{field}"),
                    },
                },
            )
        })
        .collect();
    body.dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("echo")
        .unwrap()
        .output = serde_json::from_value(input).unwrap();
    WorkflowIterationSource {
        format_version: 1,
        name: "Invoice values".into(),
        collection_input: "entries".into(),
        input: serde_json::from_value(json!({"type":"object","fields":{
            "entries":{"type":"list","item":{"type":"integer"},"max_items":1000},
            "currency":{"type":"text"},"private_note":{"type":"text"}
        }}))
        .unwrap(),
        arguments: BTreeMap::from([
            (
                "amount".into(),
                WorkflowIterationArgument::Item {
                    pointer: String::new(),
                },
            ),
            ("position".into(), WorkflowIterationArgument::Index),
            (
                "currency".into(),
                WorkflowIterationArgument::Input {
                    pointer: "/currency".into(),
                },
            ),
            (
                "factor".into(),
                WorkflowIterationArgument::Literal {
                    value: json!(-1.25),
                },
            ),
        ]),
        result: WorkflowOutputRef {
            step_id: "echo".into(),
            pointer: "/amount".into(),
        },
        rights: WorkflowRights::default(),
        budgets: body.budgets.clone(),
        body: Box::new(body),
        max_parallel: 2,
    }
}
fn input() -> Value {
    json!({"entries":[7,-3,7],"currency":"EUR","private_note":"UNBOUND_PRIVATE_MARKER"})
}
fn freeze(source: &WorkflowIterationSource, input: &Value) -> WorkflowIterationManifest {
    WorkflowIterationManifest::freeze(source, "invoices", input).unwrap()
}
fn rehash(manifest: &mut WorkflowIterationManifest) {
    manifest.sha256.clear();
    manifest.sha256 = canonical_hash(manifest);
}

#[test]
fn duplicate_values_keep_distinct_stable_indexes_and_only_explicit_arguments() {
    let source = source();
    source.validate().unwrap();
    let original = input();
    let manifest = freeze(&source, &original);
    eprintln!(
        "iteration_source_sha256={} manifest_sha256={}",
        canonical_hash(&source),
        manifest.sha256
    );
    let verified = manifest
        .verify(&source, "invoices", &canonical_hash(&original))
        .unwrap();
    assert_eq!(verified.len(), 3);
    assert!(!verified.is_empty());
    assert_eq!(
        verified.item(0).unwrap().input.as_ref(),
        &json!({"amount":7,"position":0,"currency":"EUR","factor":-1.25})
    );
    assert_eq!(verified.item(1).unwrap().input["amount"], -3);
    assert_ne!(manifest.items[0].id, manifest.items[2].id);
    assert_eq!(manifest.items[0].item_sha256, manifest.items[2].item_sha256);
    assert!(
        !serde_json::to_string(&verified.item(0).unwrap().input)
            .unwrap()
            .contains("UNBOUND_PRIVATE_MARKER")
    );
    let reopened: WorkflowIterationManifest =
        serde_json::from_slice(&serde_json::to_vec(&manifest).unwrap()).unwrap();
    let frames = reopened
        .verify(&source, "invoices", &canonical_hash(&original))
        .unwrap();
    assert_eq!(frames.item(2).unwrap(), verified.item(2).unwrap());
    assert_eq!(
        canonical_hash(&manifest),
        canonical_hash(&freeze(&source, &original))
    );
    assert!(verified.item(3).is_err());
}

#[test]
fn input_is_frozen_and_an_edited_or_reordered_list_is_a_different_execution() {
    let source = source();
    let mut values = input();
    let manifest = freeze(&source, &values);
    let original_hash = manifest.input_sha256.clone();
    values["currency"] = json!("USD");
    values["entries"] = json!([7, 7, -3]);
    assert_eq!(manifest.input["currency"], "EUR");
    assert!(
        manifest
            .verify(&source, "invoices", &canonical_hash(&values))
            .is_err()
    );
    let mut forged = freeze(&source, &values);
    rehash(&mut forged);
    assert!(forged.verify(&source, "invoices", &original_hash).is_err());
    assert_ne!(manifest.items[0].id, forged.items[0].id);
    assert!(
        manifest
            .verify(&source, "another-loop", &original_hash)
            .is_err()
    );
}

#[test]
fn rehashing_indexes_ids_or_bound_frames_does_not_create_original_evidence() {
    let source = source();
    let original = input();
    let manifest = freeze(&source, &original);
    for change in [
        "index",
        "id",
        "input_hash",
        "item_hash",
        "duplicate",
        "order",
    ] {
        let mut forged = manifest.clone();
        match change {
            "index" => forged.items[0].index = 1,
            "id" => forged.items[0].id = "iteration-".to_owned() + &"a".repeat(40),
            "input_hash" => forged.items[0].input_sha256 = "a".repeat(64),
            "item_hash" => forged.items[0].item_sha256 = "a".repeat(64),
            "duplicate" => forged.items[1] = forged.items[0].clone(),
            _ => forged.items.swap(0, 1),
        }
        rehash(&mut forged);
        assert!(
            forged
                .verify(&source, "invoices", &canonical_hash(&original))
                .is_err(),
            "{change}"
        );
    }
}

#[test]
fn changed_body_result_rights_budget_or_parallelism_invalidates_the_original_manifest() {
    let original = source();
    let manifest = freeze(&original, &input());
    for change in [
        "body",
        "result",
        "rights",
        "budget",
        "parallelism",
        "binding",
    ] {
        let mut source = original.clone();
        match change {
            "body" => source.body.controls.get_mut("echo").unwrap().name = "New body".into(),
            "result" => source.result.pointer = "/factor".into(),
            "rights" => {
                source.rights.tools.insert("write_text_file".into());
            }
            "budget" => source.budgets.max_steps -= 1,
            "parallelism" => source.max_parallel = 1,
            _ => {
                source.arguments.insert(
                    "factor".into(),
                    WorkflowIterationArgument::Literal { value: json!(2.5) },
                );
            }
        }
        assert!(
            manifest
                .verify(&source, "invoices", &canonical_hash(&input()))
                .is_err(),
            "{change}"
        );
    }
}

#[test]
fn missing_wrong_or_implicit_argument_types_are_rejected_before_materialization() {
    for change in [
        "missing",
        "extra",
        "item",
        "index",
        "pointer",
        "literal",
        "parallelism",
        "collection",
        "result",
        "hierarchy",
    ] {
        let mut source = source();
        match change {
            "missing" => {
                source.arguments.remove("currency");
            }
            "extra" => {
                source.arguments.insert(
                    "private_note".into(),
                    WorkflowIterationArgument::Input {
                        pointer: "/private_note".into(),
                    },
                );
            }
            "item" => {
                source.arguments.insert(
                    "amount".into(),
                    WorkflowIterationArgument::Input {
                        pointer: "/currency".into(),
                    },
                );
            }
            "index" => {
                source
                    .arguments
                    .insert("currency".into(), WorkflowIterationArgument::Index);
            }
            "pointer" => {
                source.arguments.insert(
                    "currency".into(),
                    WorkflowIterationArgument::Input {
                        pointer: "/constructor".into(),
                    },
                );
            }
            "literal" => {
                source.arguments.insert(
                    "factor".into(),
                    WorkflowIterationArgument::Literal {
                        value: json!("1.25"),
                    },
                );
            }
            "parallelism" => source.max_parallel = 33,
            "collection" => source.collection_input = "currency".into(),
            "hierarchy" => {
                source.body.mode = WorkflowMode::Hierarchical {
                    max_parallel: 2,
                    max_depth: 1,
                    max_delegations: 1,
                    manager_step_id: None,
                }
            }
            _ => source.result.pointer = "/missing".into(),
        }
        assert!(
            WorkflowIterationManifest::freeze(&source, "invoices", &input()).is_err(),
            "{change}"
        );
    }
    let mut wrong = input();
    wrong["entries"] = json!([7, "3"]);
    assert!(WorkflowIterationManifest::freeze(&source(), "invoices", &wrong).is_err());
}

#[test]
fn results_are_collected_by_original_index_and_missing_extra_or_wrong_values_fail() {
    let source = source();
    let manifest = freeze(&source, &input());
    let frames = manifest
        .verify(&source, "invoices", &manifest.input_sha256)
        .unwrap();
    let outputs = manifest
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| (item.id.clone(), json!(10 + index).to_string()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(frames.collect(&outputs).unwrap(), json!([10, 11, 12]));
    let mut missing = outputs.clone();
    missing.remove(&manifest.items[0].id);
    assert!(frames.collect(&missing).is_err());
    let mut extra = outputs.clone();
    extra.insert("foreign".into(), "1".into());
    assert!(frames.collect(&extra).is_err());
    let mut wrong = outputs.clone();
    wrong.insert(manifest.items[0].id.clone(), "\"10\"".into());
    assert!(frames.collect(&wrong).is_err());
    let mut invalid = outputs;
    invalid.insert(manifest.items[0].id.clone(), "not json".into());
    assert!(frames.collect(&invalid).is_err());
}

#[test]
fn empty_lists_have_no_instances_and_collect_an_exact_empty_result() {
    let source = source();
    let mut value = input();
    value["entries"] = json!([]);
    let manifest = freeze(&source, &value);
    let frames = manifest
        .verify(&source, "invoices", &manifest.input_sha256)
        .unwrap();
    assert!(frames.is_empty());
    assert_eq!(frames.collect(&BTreeMap::new()).unwrap(), json!([]));
    assert!(frames.item(0).is_err());
}

#[test]
fn fields_of_each_item_are_selected_explicitly_without_inheriting_same_named_parent_fields() {
    let mut source = source();
    let WorkflowDataType::Object { fields } = &mut source.input else {
        panic!()
    };
    let WorkflowDataType::List { item, .. } = fields.get_mut("entries").unwrap() else {
        panic!()
    };
    **item = serde_json::from_value(json!({"type":"object","fields":{
        "amount":{"type":"integer"},"invoice":{"type":"object","fields":{"currency":{"type":"text"}}}
    }})).unwrap();
    source.arguments.insert(
        "amount".into(),
        WorkflowIterationArgument::Item {
            pointer: "/amount".into(),
        },
    );
    source.arguments.insert(
        "currency".into(),
        WorkflowIterationArgument::Item {
            pointer: "/invoice/currency".into(),
        },
    );
    let value = json!({"entries":[{"amount":13,"invoice":{"currency":"JPY"}},{"amount":7,"invoice":{"currency":"USD"}}],"currency":"EUR","private_note":"UNBOUND_PRIVATE_MARKER"});
    let manifest = freeze(&source, &value);
    let frames = manifest
        .verify(&source, "invoices", &canonical_hash(&value))
        .unwrap();
    assert_eq!(
        frames.item(0).unwrap().input.as_ref(),
        &json!({"amount":13,"currency":"JPY","factor":-1.25,"position":0})
    );
    assert_eq!(frames.item(1).unwrap().input["currency"], "USD");
    for pointer in [
        "/invoice/missing",
        "/constructor",
        "/invoice/currency/nested",
    ] {
        let mut invalid = source.clone();
        invalid.arguments.insert(
            "currency".into(),
            WorkflowIterationArgument::Item {
                pointer: pointer.into(),
            },
        );
        assert!(WorkflowIterationManifest::freeze(&invalid, "invoices", &value).is_err());
    }
}

#[test]
fn optional_array_positions_missing_from_actual_input_stop_the_whole_materialization() {
    let mut source = source();
    let WorkflowDataType::Object { fields } = &mut source.input else {
        panic!()
    };
    let WorkflowDataType::List { item, .. } = fields.get_mut("entries").unwrap() else {
        panic!()
    };
    **item = WorkflowDataType::List {
        item: Box::new(WorkflowDataType::Integer),
        max_items: 3,
    };
    source.arguments.insert(
        "amount".into(),
        WorkflowIterationArgument::Item {
            pointer: "/0".into(),
        },
    );
    source.validate().unwrap();
    let value = json!({"entries":[[7],[]],"currency":"EUR","private_note":"not bound"});
    assert!(
        matches!(WorkflowIterationManifest::freeze(&source,"invoices",&value),Err(cowork_domain::Error::Validation(code)) if code=="workflow_iteration_item_pointer_missing")
    );
    let mut source = crate::source();
    source.arguments.insert(
        "amount".into(),
        WorkflowIterationArgument::Input {
            pointer: "/entries/9".into(),
        },
    );
    source.validate().unwrap();
    assert!(
        matches!(WorkflowIterationManifest::freeze(&source,"invoices",&input()),Err(cowork_domain::Error::Validation(code)) if code=="workflow_iteration_input_pointer_missing")
    );
}

#[test]
fn one_thousand_items_retain_shared_context_once_and_respect_the_declared_boundary() {
    let source = source();
    let mut value = input();
    value["entries"] = json!((0..1000).collect::<Vec<_>>());
    value["currency"] = json!("SHARED_CONTEXT_MARKER".repeat(100));
    let started = std::time::Instant::now();
    let manifest = freeze(&source, &value);
    let serialized = serde_json::to_string(&manifest).unwrap();
    assert_eq!(manifest.items.len(), 1000);
    assert_eq!(serialized.matches("SHARED_CONTEXT_MARKER").count(), 100);
    assert!(serialized.len() < 512 * 1024);
    let verified = manifest
        .verify(&source, "invoices", &canonical_hash(&value))
        .unwrap();
    assert_eq!(verified.item(999).unwrap().input["position"], 999);
    eprintln!(
        "materialize_and_verify_1000_items_ms={}",
        started.elapsed().as_millis()
    );
    value["entries"] = json!((0..1001).collect::<Vec<_>>());
    assert!(WorkflowIterationManifest::freeze(&source, "invoices", &value).is_err());
    let mut small = source;
    let WorkflowDataType::Object { fields } = &mut small.input else {
        panic!()
    };
    let WorkflowDataType::List { max_items, .. } = fields.get_mut("entries").unwrap() else {
        panic!()
    };
    *max_items = 2;
    assert!(WorkflowIterationManifest::freeze(&small, "invoices", &input()).is_err());
}

// This bridge is deliberately test-only: product integration must add instances
// to the existing parent's graph, never start an independent Run for each item.
fn parent_graph(
    source: &WorkflowIterationSource,
    manifest: &WorkflowIterationManifest,
    budgets: Budgets,
) -> WorkflowPlan {
    let frames = manifest
        .verify(source, "invoices", &manifest.input_sha256)
        .unwrap();
    let mut workflow = source.body.as_ref().clone();
    workflow.name = "One parent iteration graph".into();
    workflow.roles.clear();
    workflow.steps.clear();
    workflow.controls.clear();
    workflow.gates.clear();
    workflow.subflows.clear();
    workflow.mode = WorkflowMode::Parallel {
        max_parallel: source.max_parallel,
    };
    workflow.budgets = budgets.clone();
    let data = workflow.dataflow.as_mut().unwrap();
    data.input = source.input.clone();
    data.steps.clear();
    let mut calls = BTreeMap::new();
    let mut results = vec![];
    for (index, item) in manifest.items.iter().enumerate() {
        let frame = frames.item(index).unwrap();
        let WorkflowDataType::Object { fields } = &source.body.dataflow.as_ref().unwrap().input
        else {
            panic!()
        };
        data.steps.insert(
            item.id.clone(),
            WorkflowStepData {
                context_policy: Default::default(),
                inputs: fields
                    .iter()
                    .map(|(name, kind)| {
                        (
                            name.clone(),
                            WorkflowValueBinding {
                                value_type: kind.clone(),
                                value: WorkflowDataSource::Literal {
                                    value: frame.input[name].clone(),
                                },
                            },
                        )
                    })
                    .collect(),
                output: match frames.output_type() {
                    WorkflowDataType::List { item, .. } => item.as_ref().clone(),
                    _ => panic!(),
                },
            },
        );
        calls.insert(
            item.id.clone(),
            WorkflowSubflowCall {
                name: format!("Item {}", index + 1),
                dependencies: vec![],
                body: Box::new(WorkflowComposition {
                    format_version: 1,
                    workflow: source.body.as_ref().clone(),
                    calls: BTreeMap::new(),
                }),
                result: source.result.clone(),
                rights: source.rights.clone(),
                budgets: source.body.budgets.clone(),
            },
        );
        results.push(WorkflowOutputRef {
            step_id: item.id.clone(),
            pointer: String::new(),
        });
    }
    workflow.controls.insert(
        "collect".into(),
        WorkflowControlStep {
            name: "Collect original indexes".into(),
            dependencies: manifest.items.iter().map(|item| item.id.clone()).collect(),
            operation: WorkflowControlOperation::Collect {
                candidates: results,
            },
        },
    );
    workflow.dataflow.as_mut().unwrap().steps.insert(
        "collect".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: BTreeMap::new(),
            output: frames.output_type().clone(),
        },
    );
    let program = WorkflowComposition {
        format_version: 1,
        workflow,
        calls,
    };
    WorkflowPlan::compile_composition(
        &program,
        &WorkflowScope {
            workspace_folder: None,
            rights: source.rights.clone(),
            budgets,
            max_parallel: source.max_parallel,
        },
        &[],
    )
    .unwrap()
}

#[test]
fn materialized_frames_feed_one_existing_parent_ledger_and_preserve_result_order() {
    let source = source();
    let manifest = freeze(&source, &input());
    let plan = parent_graph(&source, &manifest, source.budgets.clone());
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let mut reopened_during_execution = false;
    for _ in 0..32 {
        let ready = progress.ready_nodes(&plan).unwrap();
        if ready.is_empty() {
            break;
        }
        // Reverse readiness deliberately; the result must still follow input indexes.
        let id = ready.last().unwrap();
        let claim = progress.claim(&plan, progress.revision(), id, 100).unwrap();
        progress
            .complete_control(
                &plan,
                progress.revision(),
                &claim,
                &format!("native-{}", progress.revision()),
                Some(&input()),
                100,
            )
            .unwrap();
        if progress.attempts().len() == 2 && !reopened_during_execution {
            let reopened: WorkflowProgress =
                serde_json::from_slice(&serde_json::to_vec(&progress).unwrap()).unwrap();
            reopened.validate(&plan).unwrap();
            assert_eq!(canonical_hash(&progress), canonical_hash(&reopened));
            progress = reopened;
            reopened_during_execution = true;
        }
    }
    assert_eq!(progress.state(), WorkflowState::Completed);
    assert_eq!(progress.charged_tokens(), 0);
    assert_eq!(progress.attempts().len(), 7);
    assert!(reopened_during_execution);
    let WorkflowNodeState::Completed { output, .. } = &progress.nodes()["collect"].state else {
        panic!()
    };
    assert_eq!(
        serde_json::from_str::<Value>(output).unwrap(),
        json!([7, -3, 7])
    );
    progress.validate(&plan).unwrap();
    let reopened: WorkflowProgress =
        serde_json::from_slice(&serde_json::to_vec(&progress).unwrap()).unwrap();
    reopened.validate(&plan).unwrap();
    assert_eq!(canonical_hash(&progress), canonical_hash(&reopened));
}

#[test]
fn a_smaller_parent_budget_is_shared_by_all_materialized_items_without_new_attempt_capacity() {
    let source = source();
    let manifest = freeze(&source, &input());
    let mut limits = source.budgets.clone();
    limits.max_steps = 3;
    let plan = parent_graph(&source, &manifest, limits);
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let mut denied = false;
    for _ in 0..32 {
        let ready = progress.ready_nodes(&plan).unwrap();
        if ready.is_empty() {
            break;
        }
        let id = ready.last().unwrap();
        let claim = progress.claim(&plan, progress.revision(), id, 100).unwrap();
        match progress.complete_control(
            &plan,
            progress.revision(),
            &claim,
            &format!("native-{}", progress.revision()),
            Some(&input()),
            100,
        ) {
            Ok(()) => {}
            Err(cowork_domain::Error::BudgetExceeded) => {
                denied = true;
                assert_eq!(progress.attempts().len(), 3);
                progress
                    .fail(&plan, progress.revision(), &claim, "budget_exceeded", 100)
                    .unwrap();
                break;
            }
            Err(error) => panic!("{error:?}"),
        }
    }
    assert!(denied);
    assert_eq!(progress.state(), WorkflowState::Failed);
    assert_eq!(progress.attempts().len(), 3);
    assert_eq!(progress.charged_tokens(), 0);
    progress.validate(&plan).unwrap();
    let frames = manifest
        .verify(&source, "invoices", &manifest.input_sha256)
        .unwrap();
    assert!(frames.collect(&BTreeMap::new()).is_err());
}
