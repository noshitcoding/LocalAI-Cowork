//! Validate the saved engine table evidence against its original typed proposal.
use super::*;
use cowork_domain::documents::DocumentCall;

fn bounds(value: &Value) -> Result<(u64, u64, u64, u64)> {
    fn cell(value: &str) -> Result<(u64, u64)> {
        let split = value
            .bytes()
            .position(|b| !b.is_ascii_uppercase())
            .ok_or_else(invalid)?;
        let (column, row) = value.split_at(split);
        if column.is_empty()
            || column.len() > 3
            || row.starts_with('0')
            || !row.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(invalid());
        }
        let column = column
            .bytes()
            .fold(0u64, |n, b| n * 26 + u64::from(b - b'A' + 1));
        let row = row.parse::<u64>().map_err(|_| invalid())?;
        if column > 16384 || row == 0 || row > 1_048_576 {
            return Err(invalid());
        }
        Ok((column, row))
    }
    let (first, last) = text(value)?.split_once(':').ok_or_else(invalid)?;
    let (left, top) = cell(first)?;
    let (right, bottom) = cell(last)?;
    if left > right || top > bottom {
        return Err(invalid());
    }
    Ok((left, top, right, bottom))
}

pub(super) fn project(call: &DocumentCall, engine: &Value) -> Result<Value> {
    let patches = call.spec["patches"]
        .as_array()
        .filter(|p| p.len() == 1)
        .ok_or_else(invalid)?;
    let patch = &patches[0];
    let rows = patch["rows"]
        .as_array()
        .filter(|rows| !rows.is_empty() && rows.len() <= 100)
        .ok_or_else(invalid)?;
    let columns = patch["columns"]
        .as_array()
        .filter(|c| !c.is_empty() && c.len() <= 32)
        .ok_or_else(invalid)?;
    let keys = patch["key_columns"]
        .as_array()
        .filter(|k| !k.is_empty() && k.len() <= 4)
        .ok_or_else(invalid)?;
    let keys = keys.iter().map(integer).collect::<Result<Vec<_>>>()?;
    let input = call
        .inputs
        .first()
        .filter(|_| call.inputs.len() == 1)
        .ok_or_else(invalid)?;
    let expected = input.expected_sha256.as_deref().ok_or_else(invalid)?;
    sha(&json!(expected))?;
    let output = call.output.as_ref().ok_or_else(invalid)?;
    let raw = &engine["result"]["table_append"];
    let qa = &engine["qa"]["table_append"];
    if call.operation != "edit"
        || patch["op"] != "append_table_rows"
        || !input.path.to_ascii_lowercase().ends_with(".xlsx")
        || !output.path.to_ascii_lowercase().ends_with(".xlsx")
        || input.path == output.path
        || output.folder_id.as_deref() != Some(input.folder_id.as_str())
        || call.spec["source_sha256"] != expected
        || raw["sheet"] != patch["sheet"]
        || raw["table"] != patch["table"]
        || text(&raw["sheet"])?.is_empty()
        || text(&raw["sheet"])?.chars().count() > 31
        || text(&raw["table"])?.is_empty()
        || text(&raw["table"])?.chars().count() > 255
        || integer(&raw["rows_added"])? != rows.len() as u64
        || raw["key_columns"] != json!(keys)
        || keys.iter().any(|k| *k >= columns.len() as u64)
        || keys.iter().collect::<std::collections::BTreeSet<_>>().len() != keys.len()
        || qa["status"] != "verified"
        || integer(&qa["rows_checked"])? != rows.len() as u64
        || rows
            .iter()
            .any(|row| row.as_array().is_none_or(|r| r.len() != columns.len()))
    {
        return Err(invalid());
    }
    let (left, top, right, bottom) = bounds(&raw["previous_range"])?;
    let (new_left, new_top, new_right, new_bottom) = bounds(&raw["range"])?;
    let first = integer(&raw["first_row"])?;
    let last = integer(&raw["last_row"])?;
    if left != new_left
        || top != new_top
        || right != new_right
        || right - left + 1 != columns.len() as u64
        || first <= top
        || first > bottom + 1
        || last != first + rows.len() as u64 - 1
        || new_bottom != bottom.max(last)
        || bottom - top > 100_000
        || (new_bottom - top + 1) * (right - left + 1) > 1_000_000
    {
        return Err(invalid());
    }
    // Project only the bounded public fields; no engine-local stage paths or raw rows.
    let table = json!({"sheet":raw["sheet"],"table":raw["table"],"rows_added":raw["rows_added"],
        "first_row":first,"last_row":last,"previous_range":raw["previous_range"],"range":raw["range"],"key_columns":keys});
    Ok(
        json!({"source":{"folder_id":input.folder_id,"path":input.path,"sha256":expected},"table":table}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn original() -> (DocumentCall, Value) {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../../tests/fixtures/workflow-table-append-result.json"
        ))
        .unwrap();
        let tool = serde_json::from_value(fixture["tool"].clone()).unwrap();
        (
            parse_document_call(&tool).unwrap(),
            fixture["engine"].clone(),
        )
    }
    #[test]
    fn actual_saved_table_receipt_projects_only_verified_source_and_row_coordinates() {
        let (call, engine) = original();
        let result = project(&call, &engine).unwrap();
        assert_eq!(result["table"]["rows_added"], 1);
        assert_eq!(result["table"]["range"], "A1:E3");
        assert_eq!(result["source"]["sha256"], call.spec["source_sha256"]);
        assert!(result["table"].get("rows").is_none());
    }
    #[test]
    fn altered_table_names_counts_ranges_key_columns_or_readback_evidence_are_rejected() {
        for (pointer, value) in [
            ("/result/table_append/sheet", json!("Other")),
            ("/result/table_append/table", json!("Other")),
            ("/result/table_append/rows_added", json!(2)),
            ("/result/table_append/first_row", json!(2)),
            ("/result/table_append/last_row", json!(4)),
            ("/result/table_append/range", json!("A1:F3")),
            ("/result/table_append/previous_range", json!("A1:E1")),
            ("/result/table_append/range", json!("A1:E1048577")),
            ("/result/table_append/key_columns", json!([1])),
            ("/qa/table_append/status", json!("unverified")),
            ("/qa/table_append/rows_checked", json!(0)),
        ] {
            let (call, mut engine) = original();
            *engine.pointer_mut(pointer).unwrap() = value;
            assert!(project(&call, &engine).is_err(), "{pointer}");
        }
    }
    #[test]
    fn another_source_folder_format_patch_or_missing_source_hash_is_rejected() {
        let (call, engine) = original();
        let mut cases = vec![];
        let mut changed = call.clone();
        changed.inputs[0].expected_sha256 = None;
        cases.push(changed);
        let mut changed = call.clone();
        changed.output.as_mut().unwrap().folder_id = Some("foreign".into());
        cases.push(changed);
        let mut changed = call.clone();
        changed.inputs[0].path = "invoices.docx".into();
        cases.push(changed);
        let mut changed = call.clone();
        changed.spec["patches"][0]["op"] = json!("append_rows");
        cases.push(changed);
        for changed in cases {
            assert!(project(&changed, &engine).is_err());
        }
    }
}
