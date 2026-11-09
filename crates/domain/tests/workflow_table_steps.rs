//! Native proposal/type tests. Actual XLSX effects are tested in the engine suite.
use cowork_domain::{documents::parse_document_call, workflows::*};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn operation() -> WorkflowDocumentOperation {
    serde_json::from_value(json!({"kind":"append_table_rows","path_input":"source","sha256_input":"sha",
        "output_path_input":"output","rows_input":"rows","sheet":"Invoices","table":"InvoiceTable",
        "columns":[{"field":"invoice","header":"Invoice","kind":"text","nullable":false},
                   {"field":"gross","header":"Gross","kind":"money_minor","nullable":false},
                   {"field":"issued","header":"Issued","kind":"date","nullable":false},
                   {"field":"note","header":"Note","kind":"text","nullable":true}],"key_fields":["invoice"]})).unwrap()
}
fn frame() -> Value {
    json!({"source":"invoices.xlsx","sha":"a".repeat(64),"output":"updated.xlsx",
        "rows":[{"invoice":"INV-002","gross":11900,"issued":"2026-11-01","note":"=1+1"}]})
}
fn definition(operation: WorkflowDocumentOperation, frame: Value) -> WorkflowDefinition {
    let rights = json!({"tools":["document_write"],"folders":["files"]});
    let budgets =
        json!({"max_steps":8,"max_seconds":180,"max_output_tokens":1024,"max_total_tokens":4096});
    let inputs: BTreeMap<_, _> = operation
        .input_types()
        .into_iter()
        .map(|(name, kind)| {
            let value = frame[&name].clone();
            (
                name,
                json!({"value_type":kind,"value":{"source":"literal","value":value}}),
            )
        })
        .collect();
    serde_json::from_value(json!({"format_version":3,"name":"Book invoice","mode":{"mode":"parallel","max_parallel":1},
        "failure":"stop","rights":rights,"budgets":budgets,"roles":[],"steps":[],
        "documents":{"book":{"name":"Append checked invoice","dependencies":[],"folder":{"kind":"selected_folder","folder_id":"files"},
            "operation":operation,"rights":rights,"budgets":budgets}},
        "dataflow":{"input":{"type":"object","fields":{}},"steps":{"book":{"inputs":inputs,"output":operation.output_type()}}}})).unwrap()
}

#[test]
fn typed_table_proposal_uses_existing_document_edit_and_preserves_minor_units() {
    let definition = definition(operation(), frame());
    definition.validate_definition().unwrap();
    let step = &definition.documents["book"];
    let call = step.tool_call(&frame()).unwrap();
    assert_eq!(call.name, "document_write");
    let parsed = parse_document_call(&call).unwrap();
    assert_eq!(parsed.operation, "edit");
    assert_eq!(parsed.inputs[0].expected_sha256, Some("a".repeat(64)));
    assert_eq!(parsed.output.unwrap().path, "updated.xlsx");
    let patch = &parsed.spec["patches"][0];
    assert_eq!(
        patch["rows"][0],
        json!(["INV-002", 11900, "2026-11-01", "=1+1"])
    );
    assert_eq!(
        patch["columns"][1],
        json!({"header":"Gross","kind":"money_minor","nullable":false})
    );
    assert_eq!(patch["key_columns"], json!([0]));
    assert!(
        !patch["columns"][0]
            .as_object()
            .unwrap()
            .contains_key("field")
    );
    assert_eq!(step.operation.input_names().len(), 4);
    assert_eq!(definition.roles.len(), 0);
}

#[test]
fn native_rows_are_objects_with_exact_required_fields_not_json_text() {
    let step = definition(operation(), frame())
        .documents
        .remove("book")
        .unwrap();
    for rows in [
        json!("[{\"invoice\":\"INV\"}]"),
        json!([]),
        json!([{"invoice":"INV"}]),
        json!([{"invoice":"INV","gross":11900,"issued":"2026-11-01","note":null,"secret":"extra"}]),
        json!([{"invoice":"INV","gross":"11900","issued":"2026-11-01","note":null}]),
    ] {
        let mut value = frame();
        value["rows"] = rows;
        assert!(step.tool_call(&value).is_err());
    }
    let mut value = frame();
    value["rows"][0]["note"] = Value::Null;
    assert!(step.tool_call(&value).is_ok());
    value["extra"] = json!(true);
    assert!(step.tool_call(&value).is_err());
}

#[test]
fn invalid_dates_money_and_blank_keys_are_rejected_before_a_proposal() {
    let step = definition(operation(), frame())
        .documents
        .remove("book")
        .unwrap();
    for (field, value) in [
        ("issued", json!("2026-02-30")),
        ("issued", json!("2026-+1-01")),
        ("issued", json!("1900-02-29")),
        ("issued", json!("1899-12-31")),
        ("gross", json!(10_000_000_000_000i64)),
        ("gross", json!(119.5)),
        ("gross", json!(true)),
        ("invoice", json!("   ")),
        ("invoice", json!("")),
    ] {
        let mut value_frame = frame();
        value_frame["rows"][0][field] = value;
        assert!(step.tool_call(&value_frame).is_err(), "{field}");
    }
    let mut value = frame();
    value["rows"][0]["issued"] = json!("2024-02-29");
    value["rows"][0]["gross"] = json!(11900.0); // JSON integers may have a float spelling.
    assert_eq!(
        step.tool_call(&value).unwrap().arguments["spec"]["patches"][0]["rows"][0][1],
        11900
    );
}

#[test]
fn duplicate_keys_schema_ambiguity_and_changed_authority_are_rejected() {
    let mut value = frame();
    let row = value["rows"][0].clone();
    value["rows"].as_array_mut().unwrap().push(row);
    assert!(
        definition(operation(), value)
            .validate_definition()
            .is_err()
    );
    for changed in [
        json!({"key_fields":[]}),
        json!({"key_fields":["invoice","invoice"]}),
        json!({"key_fields":["note"]}),
        json!({"rows_input":"source"}),
    ] {
        let mut raw = serde_json::to_value(operation()).unwrap();
        raw.as_object_mut()
            .unwrap()
            .extend(changed.as_object().unwrap().clone());
        let operation = serde_json::from_value(raw).unwrap();
        assert!(
            definition(operation, frame())
                .validate_definition()
                .is_err()
        );
    }
    let mut definition = definition(operation(), frame());
    definition
        .documents
        .get_mut("book")
        .unwrap()
        .rights
        .tools
        .clear();
    assert!(definition.validate_definition().is_err());
}

#[test]
fn source_identity_fresh_output_and_declared_row_type_are_required() {
    let step = definition(operation(), frame())
        .documents
        .remove("book")
        .unwrap();
    for (name, value) in [
        ("sha", "forged"),
        ("source", "invoice.docx"),
        ("output", "invoices.xlsx"),
        ("output", "../escape.xlsx"),
        ("output", "updated.csv"),
    ] {
        let mut data = frame();
        data[name] = json!(value);
        assert!(step.tool_call(&data).is_err());
    }
    let mut definition = definition(operation(), frame());
    definition
        .dataflow
        .as_mut()
        .unwrap()
        .steps
        .get_mut("book")
        .unwrap()
        .inputs
        .get_mut("rows")
        .unwrap()
        .value_type = WorkflowDataType::Text;
    assert!(definition.validate_definition().is_err());
}
