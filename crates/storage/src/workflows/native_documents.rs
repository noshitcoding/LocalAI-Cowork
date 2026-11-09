//! Provider-free document proposal data. Dispatch still requires the original
//! workflow lease, confirmed copy/engine binding and ordinary effect journal.
use super::*;
use cowork_domain::documents::parse_document_call;
use std::collections::BTreeMap;
mod ocr;
mod tables;

pub(super) fn completed(workflow: &StoredWorkflow) -> BTreeMap<String, String> {
    workflow
        .progress
        .nodes()
        .iter()
        .filter_map(|(id, node)| match &node.state {
            WorkflowNodeState::Completed { output, .. } => Some((id.clone(), output.clone())),
            _ => None,
        })
        .collect()
}

pub(super) fn spec(
    run: &RunRecord,
    node: &WorkflowNodePlan,
    step: &WorkflowDocumentStep,
) -> Result<RunSpec> {
    let WorkflowDocumentFolder::SelectedFolder { folder_id } = &step.folder else {
        return Err(Error::Forbidden);
    };
    if node.role_id.is_some()
        || run.spec.input_workspace.is_none()
        || !node.rights.tools.contains(step.operation.tool_name())
        || !node.rights.folders.contains(folder_id)
    {
        return Err(Error::Forbidden);
    }
    let mut spec = run.spec.clone();
    spec.provider = None;
    spec.model_binding = None;
    spec.output = None;
    spec.budgets = node.budgets.clone();
    spec.allowed_tools
        .retain(|tool| tool == step.operation.tool_name());
    spec.folders.retain(|folder| folder.id == *folder_id);
    spec.mcp_tools.clear();
    if spec.allowed_tools.len() != 1 || spec.folders.len() != 1 {
        return Err(Error::Forbidden);
    }
    if let Some(access) = spec
        .agent_config
        .as_ref()
        .and_then(|config| config.access_mode.as_ref())
    {
        spec.access_mode = cowork_domain::narrow_access_mode(&spec.access_mode, access);
    }
    spec.agent_config = None;
    if spec.environment.is_some() {
        return Err(Error::Forbidden);
    }
    if matches!(spec.access_mode, AccessMode::ReadOnly) {
        if step.operation.tool_name() != "document_read" {
            return Err(Error::Forbidden);
        }
        for folder in &mut spec.folders {
            folder.writable = false;
        }
    }
    Ok(spec)
}

pub(super) fn system(node: &str, step: &WorkflowDocumentStep) -> Value {
    json!({"role":"system","content":json!({"native_document_step":node,"operation":step.operation,"folder":step.folder}).to_string()})
}

pub(super) fn fingerprint(spec: &RunSpec, messages: &[Value], tools: &Value) -> Result<String> {
    if spec.provider.is_some()
        || spec.model_binding.is_some()
        || spec.workflow.is_none()
        || spec.input_workspace.is_none()
        || spec.allowed_tools.len() != 1
        || !matches!(
            spec.allowed_tools[0].as_str(),
            "document_read" | "document_write"
        )
    {
        return Err(Error::Forbidden);
    }
    let frame = json!({"format":"workflow-native-document-context/1","spec":spec,"messages":messages,"tools":tools});
    if serde_json::to_vec(&frame).map_err(storage_error)?.len() > 2 * 1024 * 1024 {
        return Err(Error::ContextLimit);
    }
    Ok(canonical_hash(&frame))
}

fn invalid() -> Error {
    Error::Validation("workflow_native_document_result_invalid".into())
}
fn text(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(invalid)
}
fn integer(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .filter(|number| *number <= 9_007_199_254_740_991)
        .ok_or_else(invalid)
}
fn sha(value: &Value) -> Result<&str> {
    let value = text(value)?;
    if !cowork_domain::workspace_transfer::valid_sha256(value) {
        return Err(invalid());
    }
    Ok(value)
}
fn clipped(value: &str, bytes: usize) -> (String, bool) {
    let mut end = value.len().min(bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_owned(), end < value.len())
}
fn diagnostics(engine: &Value) -> Result<(Value, bool)> {
    let raw = engine.get("diagnostics").ok_or_else(invalid)?;
    let raw = raw.as_array().ok_or_else(invalid)?;
    let mut result = vec![];
    let mut bytes = 0;
    let mut truncated = false;
    for entry in raw {
        if result.len() == 64 {
            truncated = true;
            break;
        }
        let (severity, cut_severity) = clipped(text(&entry["severity"])?, 32);
        let (message, cut_message) = clipped(text(&entry["message"])?, 512);
        let (file, cut_file) = match entry.get("file") {
            Some(Value::String(file)) => {
                let (file, cut) = clipped(file, 512);
                (json!(file), cut)
            }
            None | Some(Value::Null) => (Value::Null, false),
            _ => return Err(invalid()),
        };
        let line = match entry.get("line") {
            None | Some(Value::Null) => Value::Null,
            Some(value) => json!(integer(value)?),
        };
        let item = json!({"severity":severity,"message":message,"file":file,"line":line});
        let size = serde_json::to_vec(&item).map_err(storage_error)?.len();
        if bytes + size > 8 * 1024 {
            truncated = true;
            break;
        }
        bytes += size;
        truncated |= cut_severity || cut_message || cut_file;
        result.push(item);
    }
    Ok((json!(result), truncated))
}

fn preview(source: Value, raw: &Value, images: &Value, options: &Value) -> Result<Value> {
    let total = match raw.get("pages") {
        Some(value) => integer(value)?,
        None if raw["format"] == "image" => 1,
        _ => return Err(invalid()),
    };
    if total == 0 {
        return Err(invalid());
    }
    let images = images.as_array().ok_or_else(invalid)?;
    if images.is_empty() || images.len() > 16 {
        return Err(invalid());
    }
    let mut rendered = std::collections::BTreeSet::new();
    let projected = images
        .iter()
        .map(|image| {
            let page = integer(&image["page"])?;
            let width = integer(&image["width"])?;
            let height = integer(&image["height"])?;
            if image["media_type"] != "image/png"
                || page == 0
                || page > total
                || width == 0
                || height == 0
                || width > 100_000
                || height > 100_000
                || !rendered.insert(page)
            {
                return Err(invalid());
            }
            Ok(
                json!({"sha256":sha(&image["sha256"])?,"mime_type":"image/png",
            "page":page,"width":width,"height":height}),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    if let Some(raw_pages) = raw.get("rendered") {
        let raw_pages = raw_pages.as_array().ok_or_else(invalid)?;
        if raw_pages.len() != projected.len()
            || raw_pages
                .iter()
                .zip(&projected)
                .any(|(page, image)| page != &image["page"])
        {
            return Err(invalid());
        }
    }
    let incomplete = if let Some(pages) = options.get("pages") {
        let requested = pages
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(integer)
            .collect::<Result<std::collections::BTreeSet<_>>>()?;
        if requested.is_empty() || !rendered.is_subset(&requested) {
            return Err(invalid());
        }
        requested != rendered
    } else {
        rendered.len() as u64 != total
    };
    let truncated = match raw.get("truncated") {
        None => false,
        Some(value) => value.as_bool().ok_or_else(invalid)?,
    };
    Ok(
        json!({"source":source,"images":projected,"total_pages":total,"truncated":truncated || incomplete}),
    )
}

fn text_chunk(source: Value, raw: &Value, expected_offset: u64) -> Result<Value> {
    let content = text(&raw["text"])?;
    let offset = integer(&raw["offset"])?;
    if offset != expected_offset {
        return Err(invalid());
    }
    let next = raw.get("next_offset").ok_or_else(invalid)?;
    if !next.is_null() {
        let next = integer(next)?;
        if next <= offset || next != offset + content.chars().count() as u64 {
            return Err(invalid());
        }
    }
    let truncated = raw["truncated"].as_bool().ok_or_else(invalid)?;
    if !next.is_null() && !truncated {
        return Err(invalid());
    }
    // A workflow Text field allows 16,384 Unicode scalar values; the engine's
    // separate byte ceiling can contain more ASCII. Retain a truthful resume
    // offset when projecting a smaller workflow chunk.
    let end = content.char_indices().nth(16_384).map(|(byte, _)| byte);
    let (content, next) = match end {
        Some(end) => (&content[..end], json!(offset + 16_384)),
        None => (content, next.clone()),
    };
    Ok(
        json!({"source":source,"text":content,"offset":offset,"next_offset":next,"truncated":truncated||end.is_some()}),
    )
}

/// Projection of an already validated original tool receipt. This function
/// grants no preview access, engine permission, publication or retry authority.
pub(super) fn output(turn: &WorkflowModelTurn) -> Result<String> {
    if !turn.deterministic
        || turn.context.spec.provider.is_some()
        || turn.context.spec.model_binding.is_some()
    {
        return Err(Error::Forbidden);
    }
    let response = turn.response.as_ref().ok_or_else(invalid)?;
    if response.tools.len() != 1 || turn.tool_results.len() != 1 {
        return Err(invalid());
    }
    let tool = &response.tools[0];
    let receipt = &turn.tool_results[0];
    if !receipt.succeeded
        || receipt.tool != *tool
        || receipt.result_sha256 != canonical_hash(&receipt.result)
    {
        return Err(invalid());
    }
    let call = parse_document_call(tool)?;
    let engine = &receipt.result["engine"];
    if !engine.is_object() || engine.get("error").is_some() {
        return Err(invalid());
    }
    let raw = &engine["result"];
    let mut value = match (call.tool.as_str(), call.operation.as_str()) {
        ("document_read", "text" | "preview" | "ocr") => {
            let input = call.inputs.first().ok_or_else(invalid)?;
            let expected = input.expected_sha256.as_ref().ok_or_else(invalid)?;
            if raw
                .get("source_sha256")
                .is_some_and(|value| value.as_str() != Some(expected.as_str()))
            {
                return Err(invalid());
            }
            let source = json!({"folder_id":input.folder_id,"path":input.path,"sha256":expected});
            if call.operation == "text" {
                if sha(&raw["source_sha256"])? != expected {
                    return Err(invalid());
                }
                text_chunk(source, raw, call.options["offset"].as_u64().unwrap_or(0))?
            } else if call.operation == "ocr" {
                let count = match engine.get("previews") {
                    Some(value) => integer(&value["count"])?,
                    None => 0,
                };
                ocr::project(
                    source,
                    raw,
                    &receipt.result["cowork_images"],
                    &call.options,
                    count,
                )?
            } else {
                let images = receipt.result["cowork_images"]
                    .as_array()
                    .ok_or_else(invalid)?;
                if images.is_empty()
                    || images.len() > 16
                    || engine["previews"]["count"].as_u64() != Some(images.len() as u64)
                {
                    return Err(invalid());
                }
                preview(source, raw, &receipt.result["cowork_images"], &call.options)?
            }
        }
        ("document_write", "create" | "create_pdf" | "edit") => {
            let output = call.output.as_ref().ok_or_else(invalid)?;
            let operation: FileOperation =
                serde_json::from_value(receipt.result["operation_receipt"].clone())
                    .map_err(|_| invalid())?;
            if operation.kind != FileOperationKind::Generate
                || operation.tool_call_id != receipt.id
                || output.folder_id.as_deref() != Some(operation.folder_id.as_str())
                || output.path != operation.path
                || operation.before.is_some()
                || operation.destination.is_some()
            {
                return Err(invalid());
            }
            let after = operation.after.as_ref().ok_or_else(invalid)?;
            if after.kind != FileNodeKind::File || after.size > 9_007_199_254_740_991 {
                return Err(invalid());
            }
            sha(&json!(after.sha256))?;
            let mut result = if call.operation == "edit" {
                tables::project(&call, engine)?
            } else {
                json!({})
            };
            let mime = if call.operation == "edit" {
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            } else if call.operation == "create_pdf" {
                "application/pdf"
            } else {
                if call.spec["format"] != "docx" {
                    return Err(invalid());
                }
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            };
            result["file"] = json!({"folder_id":operation.folder_id,"path":operation.path,"sha256":after.sha256,"bytes":after.size,"mime_type":mime});
            result
        }
        _ => return Err(Error::Forbidden),
    };
    let (diagnostics, truncated) = diagnostics(engine)?;
    value["diagnostics"] = diagnostics;
    value["diagnostics_truncated"] = json!(truncated);
    let result = serde_json::to_string(&value).map_err(storage_error)?;
    if result.len() > 65_536 {
        return Err(Error::ContextLimit);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_large_engine_chunk_keeps_the_exact_unicode_resume_position() {
        let text = "a".repeat(16_383) + "🦀tail";
        let value = text_chunk(
            json!({}),
            &json!({"text":text,"offset":7,"next_offset":null,"truncated":false}),
            7,
        )
        .unwrap();
        assert_eq!(value["text"].as_str().unwrap().chars().count(), 16_384);
        assert!(value["text"].as_str().unwrap().ends_with('🦀'));
        assert_eq!(value["next_offset"], 16_391);
        assert_eq!(value["truncated"], true);
    }
    #[test]
    fn malformed_text_resume_evidence_is_rejected_and_upstream_truncation_is_retained() {
        for raw in [
            json!({"text":"🦀","offset":1,"next_offset":5,"truncated":true}),
            json!({"text":"ok","offset":0,"next_offset":2,"truncated":false}),
            json!({"text":"ok","offset":1,"next_offset":null,"truncated":false}),
        ] {
            assert!(text_chunk(json!({}), &raw, 0).is_err());
        }
        let value = text_chunk(
            json!({}),
            &json!({"text":"last","offset":0,"next_offset":null,"truncated":true}),
            0,
        )
        .unwrap();
        assert_eq!(value["truncated"], true);
        assert!(value["next_offset"].is_null());
    }
    fn images(pages: &[u64]) -> Value {
        json!(pages.iter().map(|page|json!({"page":page,"width":1200,"height":1600,"sha256":"a".repeat(64),"media_type":"image/png"})).collect::<Vec<_>>())
    }
    #[test]
    fn default_preview_exposes_the_unrendered_remainder() {
        let output = preview(
            json!({}),
            &json!({"format":"pdf","pages":6,"rendered":[1,2,3,4]}),
            &images(&[1, 2, 3, 4]),
            &json!({}),
        )
        .unwrap();
        assert_eq!(output["truncated"], true);
        assert_eq!(output["total_pages"], 6);
        assert_eq!(output["images"][3]["page"], 4);
        assert_eq!(output["images"][0]["height"], 1600);
    }
    #[test]
    fn explicit_selection_is_complete_only_when_all_requested_pages_exist() {
        let raw = json!({"format":"pdf","pages":6,"rendered":[2,6]});
        let complete = preview(json!({}), &raw, &images(&[2, 6]), &json!({"pages":[6,2]})).unwrap();
        assert_eq!(complete["truncated"], false);
        let partial =
            preview(json!({}), &raw, &images(&[2, 6]), &json!({"pages":[1,2,6]})).unwrap();
        assert_eq!(partial["truncated"], true);
    }
    #[test]
    fn substituted_or_unusable_preview_metadata_is_rejected() {
        let raw = json!({"format":"pdf","pages":6,"rendered":[2,6]});
        for changed in [
            {
                let mut v = images(&[2, 6]);
                v[0]["width"] = json!(0);
                v
            },
            {
                let mut v = images(&[2, 6]);
                v[0]["sha256"] = json!("FORGED");
                v
            },
            {
                let mut v = images(&[2, 6]);
                v[0]["media_type"] = json!("text/plain");
                v
            },
            images(&[2, 2]),
            images(&[0, 6]),
            images(&[2, 7]),
        ] {
            assert!(preview(json!({}), &raw, &changed, &json!({})).is_err());
        }
        assert!(preview(json!({}), &raw, &images(&[6, 2]), &json!({})).is_err());
        assert!(preview(json!({}), &raw, &images(&[2, 6]), &json!({"pages":[1,2]})).is_err());
        assert!(
            preview(
                json!({}),
                &json!({"format":"pdf"}),
                &images(&[1]),
                &json!({})
            )
            .is_err()
        );
    }
    #[test]
    fn image_previews_keep_a_single_page_and_validate_explicit_selection() {
        let output = preview(
            json!({}),
            &json!({"format":"image"}),
            &images(&[1]),
            &json!({}),
        )
        .unwrap();
        assert_eq!(output["total_pages"], 1);
        assert_eq!(output["truncated"], false);
        assert!(
            preview(
                json!({}),
                &json!({"format":"image"}),
                &images(&[1]),
                &json!({"pages":[2]})
            )
            .is_err()
        );
    }
    #[test]
    fn diagnostics_preserve_locations_and_explicitly_mark_utf8_clipping() {
        let long = "🦀".repeat(200);
        let (value,truncated)=diagnostics(&json!({"diagnostics":[{"severity":"warning","message":long,"file":"source.pdf","line":7}]})).unwrap();
        assert!(truncated);
        assert_eq!(value[0]["message"].as_str().unwrap().len(), 512);
        assert_eq!(value[0]["file"], "source.pdf");
        assert_eq!(value[0]["line"], 7);
    }
    #[test]
    fn diagnostic_count_and_encoded_bytes_are_bounded_without_silent_loss() {
        let (count, cut) = diagnostics(
            &json!({"diagnostics":vec![json!({"severity":"warning","message":"small"});65]}),
        )
        .unwrap();
        assert!(cut);
        assert_eq!(count.as_array().unwrap().len(), 64);
        let (bytes,cut)=diagnostics(&json!({"diagnostics":vec![json!({"severity":"warning","message":"m".repeat(512),"file":"f".repeat(512)});64]})).unwrap();
        assert!(cut);
        assert!(serde_json::to_vec(&bytes).unwrap().len() < 8300);
        for bad in [
            json!({}),
            json!({"diagnostics":{}}),
            json!({"diagnostics":[{"severity":"warning","message":"m","line":1.5}]}),
        ] {
            assert!(diagnostics(&bad).is_err());
        }
    }
}
