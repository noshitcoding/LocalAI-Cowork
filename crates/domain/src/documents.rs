//! Model-facing arguments of the built-in document and image tools.
//! Paths are only shape-checked here; the file broker remains the path authority.
use crate::{Error, Result};
use cowork_contracts::ToolCall;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const MAX_DOCUMENT_INPUTS: usize = 16;
pub const MAX_DOCUMENT_SPEC_BYTES: usize = 512 * 1024;
pub const MAX_IMAGE_STEPS: usize = 50;
const READ_OPERATIONS: [&str; 6] = ["text", "inspect", "metadata", "preview", "ocr", "help"];
const WRITE_OPERATIONS: [&str; 6] = [
    "create",
    "edit",
    "pdf",
    "compile_latex",
    "create_pdf",
    "convert",
];
const HELP_TOPICS: [&str; 7] = ["overview", "docx", "xlsx", "pptx", "pdf", "latex", "image"];
const PDF_ACTIONS: [&str; 9] = [
    "merge",
    "split",
    "extract_pages",
    "rotate",
    "reorder",
    "delete_pages",
    "watermark",
    "fill_form",
    "set_metadata",
];
const OFFICE_FORMATS: [&str; 3] = ["docx", "xlsx", "pptx"];
const CONVERT_FORMATS: [&str; 7] = ["pdf", "docx", "xlsx", "pptx", "odt", "ods", "odp"];
const IMAGE_FORMATS: [&str; 8] = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentInput {
    pub folder_id: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentOutput {
    /// Omitted means the order's managed working folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    pub path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    operation: String,
    #[serde(default)]
    folder_id: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    expected_sha256: Option<String>,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    sheet: Option<String>,
    #[serde(default)]
    pages: Option<Vec<u32>>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    languages: Option<Vec<String>>,
    #[serde(default)]
    dpi: Option<u32>,
    #[serde(default)]
    rotation: Option<u16>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    operation: String,
    #[serde(default)]
    inputs: Vec<DocumentInput>,
    output: DocumentOutput,
    #[serde(default)]
    spec: Option<Value>,
    #[serde(default)]
    preview: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageArgs {
    input: DocumentInput,
    output: DocumentOutput,
    steps: Vec<Value>,
    #[serde(default)]
    overlays: Vec<DocumentInput>,
    #[serde(default)]
    quality: Option<u8>,
    #[serde(default)]
    keep_exif: bool,
    #[serde(default)]
    preview: bool,
}

/// A validated document tool call in engine terms.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DocumentCall {
    pub tool: String,
    /// Engine operation; `pipeline` for image edits.
    pub operation: String,
    pub inputs: Vec<DocumentInput>,
    pub output: Option<DocumentOutput>,
    pub spec: Value,
    pub options: Value,
}

impl DocumentCall {
    /// PDF splits publish one new directory instead of a single file.
    pub fn output_is_directory(&self) -> bool {
        self.tool == "document_write" && self.operation == "pdf" && self.spec["action"] == "split"
    }
    pub fn preview_requested(&self) -> bool {
        self.operation == "preview" || self.options["preview"] == true
    }
}

fn invalid(message: &str) -> Error {
    Error::Validation(message.into())
}

pub fn extension(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let (stem, extension) = name.rsplit_once('.')?;
    (!stem.is_empty() && !extension.is_empty()).then(|| extension.to_ascii_lowercase())
}

fn valid_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.starts_with('/')
        || path.contains(['\\', '\0'])
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(invalid("Use a relative path inside a granted folder"));
    }
    Ok(())
}

fn valid_hash(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid(
            "expected_sha256 must be the 64-character lowercase SHA-256 from document_read or stat_file",
        ));
    }
    Ok(())
}

fn valid_input(input: &DocumentInput) -> Result<()> {
    if input.folder_id.is_empty() || input.folder_id.len() > 128 {
        return Err(invalid("Choose a granted folder_id"));
    }
    valid_path(&input.path)?;
    if let Some(hash) = &input.expected_sha256 {
        valid_hash(hash)?;
    }
    Ok(())
}

fn valid_output(output: &DocumentOutput) -> Result<()> {
    if output
        .folder_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 128)
    {
        return Err(invalid(
            "Choose a granted folder_id or omit it for the working folder",
        ));
    }
    valid_path(&output.path)
}

fn object(value: Option<Value>) -> Result<Value> {
    let value = value.unwrap_or_else(|| Value::Object(Map::new()));
    if !value.is_object() {
        return Err(invalid("spec must be a JSON object"));
    }
    if serde_json::to_vec(&value)
        .map_err(|_| invalid("spec is not valid JSON"))?
        .len()
        > MAX_DOCUMENT_SPEC_BYTES
    {
        return Err(invalid(
            "spec exceeds 512 KiB; build long documents in several edit calls",
        ));
    }
    Ok(value)
}

fn require_extension(path: &str, allowed: &[&str], message: &str) -> Result<String> {
    match extension(path) {
        Some(extension) if allowed.contains(&extension.as_str()) => Ok(extension),
        _ => Err(invalid(message)),
    }
}

pub fn parse_document_call(tool: &ToolCall) -> Result<DocumentCall> {
    match tool.name.as_str() {
        "document_read" => parse_read(tool),
        "document_write" => parse_write(tool),
        "image_edit" => parse_image(tool),
        _ => Err(Error::Forbidden),
    }
}

fn parse_read(tool: &ToolCall) -> Result<DocumentCall> {
    let args: ReadArgs = serde_json::from_value(tool.arguments.clone())
        .map_err(|e| invalid(&format!("Invalid document_read arguments: {e}")))?;
    if !READ_OPERATIONS.contains(&args.operation.as_str()) {
        return Err(invalid(
            "operation must be text, inspect, metadata, preview, ocr or help",
        ));
    }
    if args
        .sheet
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.len() > 256)
        || args.offset.is_some_and(|o| o > 1_000_000_000)
        || args.limit.is_some_and(|l| l == 0 || l > 100)
        || args.pages.as_ref().is_some_and(|p| {
            p.is_empty() || p.len() > 16 || p.iter().any(|n| *n == 0 || *n > 10_000)
        })
    {
        return Err(invalid(
            "Use offset ≥ 0, limit 1-100, a sheet name and 1-16 page numbers starting at 1",
        ));
    }
    let mut options = json!({});
    if args.operation == "ocr" {
        let languages = args
            .languages
            .clone()
            .unwrap_or_else(|| vec!["deu".into(), "eng".into()]);
        if languages.is_empty()
            || languages.len() > 2
            || languages
                .iter()
                .any(|value| !matches!(value.as_str(), "deu" | "eng"))
            || languages
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != languages.len()
            || args.dpi.is_some_and(|dpi| !(100..=400).contains(&dpi))
            || args
                .rotation
                .is_some_and(|rotation| !matches!(rotation, 0 | 90 | 180 | 270))
            || args.sheet.is_some()
        {
            return Err(invalid(
                "OCR needs distinct deu/eng languages, DPI 100-400, rotation 0/90/180/270, and no sheet",
            ));
        }
        options["languages"] = json!(languages);
        options["dpi"] = json!(args.dpi.unwrap_or(200));
        options["rotation"] = json!(args.rotation.unwrap_or(0));
        options["limit"] = json!(args.limit.unwrap_or(100));
    } else if args.languages.is_some() || args.dpi.is_some() || args.rotation.is_some() {
        return Err(invalid("languages, dpi and rotation are only used by OCR"));
    }
    if let Some(offset) = args.offset {
        options["offset"] = json!(offset);
    }
    if let Some(limit) = args.limit {
        options["limit"] = json!(limit);
    }
    if let Some(sheet) = &args.sheet {
        options["sheet"] = json!(sheet);
    }
    if let Some(pages) = &args.pages {
        options["pages"] = json!(pages);
    }
    let inputs = if args.operation == "help" {
        let topic = args.topic.clone().unwrap_or_else(|| "overview".into());
        if !HELP_TOPICS.contains(&topic.as_str()) || args.folder_id.is_some() || args.path.is_some()
        {
            return Err(invalid(
                "help takes only topic: overview, docx, xlsx, pptx, pdf, latex or image",
            ));
        }
        options["topic"] = json!(topic);
        Vec::new()
    } else {
        if args.topic.is_some() {
            return Err(invalid("topic is only used with operation help"));
        }
        let input = DocumentInput {
            folder_id: args
                .folder_id
                .ok_or_else(|| invalid("folder_id is required"))?,
            path: args.path.ok_or_else(|| invalid("path is required"))?,
            expected_sha256: args.expected_sha256,
        };
        valid_input(&input)?;
        if args.operation == "ocr" {
            require_extension(
                &input.path,
                &[
                    "pdf", "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff",
                ],
                "OCR needs a PDF or supported image file",
            )?;
        }
        vec![input]
    };
    Ok(DocumentCall {
        tool: tool.name.clone(),
        operation: args.operation,
        inputs,
        output: None,
        spec: json!({}),
        options,
    })
}

fn parse_write(tool: &ToolCall) -> Result<DocumentCall> {
    let args: WriteArgs = serde_json::from_value(tool.arguments.clone())
        .map_err(|e| invalid(&format!("Invalid document_write arguments: {e}")))?;
    if !WRITE_OPERATIONS.contains(&args.operation.as_str()) {
        return Err(invalid(
            "operation must be create, edit, pdf, compile_latex, create_pdf or convert",
        ));
    }
    if args.inputs.len() > MAX_DOCUMENT_INPUTS {
        return Err(invalid("Use at most 16 inputs"));
    }
    args.inputs.iter().try_for_each(valid_input)?;
    valid_output(&args.output)?;
    let mut spec = object(args.spec)?;
    let output = &args.output.path;
    match args.operation.as_str() {
        "create" => {
            let format = spec["format"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !OFFICE_FORMATS.contains(&format.as_str())
                || extension(output).as_deref() != Some(format.as_str())
            {
                return Err(invalid(
                    "create needs spec.format docx, xlsx or pptx and an output path with that extension",
                ));
            }
        }
        "edit" => {
            let source = args
                .inputs
                .first()
                .ok_or_else(|| invalid("edit needs the source document as inputs[0]"))?;
            let hash = source.expected_sha256.clone().ok_or_else(|| {
                invalid("edit needs inputs[0].expected_sha256; call document_read inspect first")
            })?;
            let format = require_extension(
                &source.path,
                &OFFICE_FORMATS,
                "edit supports DOCX, XLSX and PPTX sources",
            )?;
            if extension(output).as_deref() != Some(format.as_str()) {
                return Err(invalid(
                    "The edited output must keep the source file extension",
                ));
            }
            match spec.get("source_sha256") {
                Some(Value::String(value)) if *value != hash => {
                    return Err(invalid(
                        "spec.source_sha256 differs from inputs[0].expected_sha256",
                    ));
                }
                Some(Value::String(_)) | None => spec["source_sha256"] = json!(hash),
                Some(_) => return Err(invalid("spec.source_sha256 must be a string")),
            }
        }
        "pdf" => {
            let action = spec["action"].as_str().unwrap_or_default();
            if !PDF_ACTIONS.contains(&action) {
                return Err(invalid(
                    "pdf needs spec.action merge, split, extract_pages, rotate, reorder, delete_pages, watermark, fill_form or set_metadata",
                ));
            }
            if args.inputs.is_empty()
                || require_extension(
                    &args.inputs[0].path,
                    &["pdf"],
                    "pdf operations start from a PDF input",
                )
                .is_err()
            {
                return Err(invalid("pdf operations need the PDF as inputs[0]"));
            }
            if action == "split" {
                if extension(output).is_some() {
                    return Err(invalid(
                        "split writes a new folder; use an output path without extension",
                    ));
                }
            } else {
                require_extension(output, &["pdf"], "The output path must end with .pdf")?;
            }
        }
        "compile_latex" => {
            let main = args
                .inputs
                .first()
                .ok_or_else(|| invalid("compile_latex needs the .tex file as inputs[0]"))?;
            require_extension(&main.path, &["tex"], "inputs[0] must be a .tex file")?;
            require_extension(output, &["pdf"], "The output path must end with .pdf")?;
            if args.inputs.iter().any(|i| i.folder_id != main.folder_id) {
                return Err(invalid(
                    "LaTeX assets must be in the same folder as the main .tex file",
                ));
            }
        }
        "create_pdf" => {
            require_extension(output, &["pdf"], "The output path must end with .pdf")?;
        }
        "convert" => {
            if args.inputs.len() != 1 {
                return Err(invalid("convert needs exactly one input"));
            }
            let target = require_extension(
                output,
                &CONVERT_FORMATS,
                "convert writes pdf, docx, xlsx, pptx, odt, ods or odp",
            )?;
            let recalculate = match spec.get("recalculate") {
                None => false,
                Some(Value::Bool(value)) => *value,
                _ => return Err(invalid("spec.recalculate must be a boolean")),
            };
            let source = extension(&args.inputs[0].path);
            if recalculate && (source.as_deref() != Some("xlsx") || target != "xlsx") {
                return Err(invalid(
                    "recalculation needs an XLSX input and a new XLSX output",
                ));
            }
            if source.as_deref() == Some(target.as_str()) && !recalculate {
                return Err(invalid(
                    "convert needs a different format, or spec.recalculate=true for XLSX",
                ));
            }
        }
        _ => unreachable!(),
    }
    Ok(DocumentCall {
        tool: tool.name.clone(),
        operation: args.operation,
        inputs: args.inputs,
        output: Some(args.output),
        spec,
        options: json!({"preview": args.preview}),
    })
}

fn parse_image(tool: &ToolCall) -> Result<DocumentCall> {
    let args: ImageArgs = serde_json::from_value(tool.arguments.clone())
        .map_err(|e| invalid(&format!("Invalid image_edit arguments: {e}")))?;
    valid_input(&args.input)?;
    valid_output(&args.output)?;
    if args.overlays.len() > 8 {
        return Err(invalid("Use at most 8 overlay images"));
    }
    args.overlays.iter().try_for_each(valid_input)?;
    if args.steps.is_empty() || args.steps.len() > MAX_IMAGE_STEPS {
        return Err(invalid("steps needs 1-50 operations"));
    }
    if args
        .steps
        .iter()
        .any(|step| !step.get("op").is_some_and(Value::is_string))
    {
        return Err(invalid(
            "Every step needs an op name, e.g. {\"op\":\"resize\",\"width\":800}",
        ));
    }
    if args.quality.is_some_and(|q| q == 0 || q > 100) {
        return Err(invalid("quality must be 1-100"));
    }
    require_extension(
        &args.input.path,
        &IMAGE_FORMATS,
        "The input must be a PNG, JPEG, WebP, GIF, BMP or TIFF image",
    )?;
    require_extension(
        &args.output.path,
        &IMAGE_FORMATS,
        "The output path must end with .png, .jpg, .webp, .gif, .bmp or .tiff",
    )?;
    let mut spec = json!({"steps": args.steps, "keep_exif": args.keep_exif});
    if let Some(quality) = args.quality {
        spec["quality"] = json!(quality);
    }
    let spec = object(Some(spec))?;
    let mut inputs = vec![args.input];
    inputs.extend(args.overlays);
    Ok(DocumentCall {
        tool: tool.name.clone(),
        operation: "pipeline".into(),
        inputs,
        output: Some(args.output),
        spec,
        options: json!({"preview": args.preview}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: "call".into(),
            name: name.into(),
            arguments,
        }
    }

    #[test]
    fn read_calls_are_normalized() {
        let parsed = parse_document_call(&call(
            "document_read",
            json!({"operation":"text","folder_id":"f","path":"a/b.docx","offset":10}),
        ))
        .unwrap();
        assert_eq!(parsed.inputs[0].path, "a/b.docx");
        assert_eq!(parsed.options["offset"], 10);
        let help = parse_document_call(&call(
            "document_read",
            json!({"operation":"help","topic":"latex"}),
        ))
        .unwrap();
        assert!(help.inputs.is_empty() && help.options["topic"] == "latex");
        for bad in [
            json!({"operation":"text","folder_id":"f"}),
            json!({"operation":"help","path":"x"}),
            json!({"operation":"run","folder_id":"f","path":"x"}),
            json!({"operation":"text","folder_id":"f","path":"../x"}),
            json!({"operation":"text","folder_id":"f","path":"x","unknown":1}),
        ] {
            assert!(parse_document_call(&call("document_read", bad)).is_err());
        }
    }

    #[test]
    fn write_calls_check_formats_and_hashes() {
        let sha = "a".repeat(64);
        let edit = parse_document_call(&call("document_write", json!({"operation":"edit","inputs":[{"folder_id":"f","path":"r.docx","expected_sha256":sha}],"output":{"path":"r-v2.docx"},"spec":{"patches":[]}}))).unwrap();
        assert_eq!(edit.spec["source_sha256"], sha);
        assert!(edit.output.unwrap().folder_id.is_none());
        let split = parse_document_call(&call("document_write", json!({"operation":"pdf","inputs":[{"folder_id":"f","path":"a.pdf"}],"output":{"path":"parts"},"spec":{"action":"split"}}))).unwrap();
        assert!(split.output_is_directory());
        for bad in [
            json!({"operation":"create","output":{"path":"x.docx"},"spec":{"format":"xlsx"}}),
            json!({"operation":"edit","inputs":[{"folder_id":"f","path":"r.docx"}],"output":{"path":"r2.docx"}}),
            json!({"operation":"edit","inputs":[{"folder_id":"f","path":"r.docx","expected_sha256":sha}],"output":{"path":"r2.pdf"}}),
            json!({"operation":"compile_latex","inputs":[{"folder_id":"f","path":"main.md"}],"output":{"path":"x.pdf"}}),
            json!({"operation":"pdf","inputs":[{"folder_id":"f","path":"a.pdf"}],"output":{"path":"b.pdf"},"spec":{"action":"explode"}}),
            json!({"operation":"convert","inputs":[{"folder_id":"f","path":"a.docx"}],"output":{"path":"b.docx"}}),
            json!({"operation":"create","output":{"path":"x.docx"},"spec":"text"}),
        ] {
            assert!(
                parse_document_call(&call("document_write", bad.clone())).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn image_calls_need_steps() {
        let parsed = parse_document_call(&call("image_edit", json!({"input":{"folder_id":"f","path":"a.jpg"},"output":{"path":"b.png"},"steps":[{"op":"resize","width":100}],"overlays":[{"folder_id":"f","path":"logo.png"}]}))).unwrap();
        assert_eq!(parsed.operation, "pipeline");
        assert_eq!(parsed.inputs.len(), 2);
        assert_eq!(parsed.spec["keep_exif"], false);
        assert!(parse_document_call(&call("image_edit", json!({"input":{"folder_id":"f","path":"a.jpg"},"output":{"path":"b.png"},"steps":[]}))).is_err());
        assert!(parse_document_call(&call("image_edit", json!({"input":{"folder_id":"f","path":"a.jpg"},"output":{"path":"b.exe"},"steps":[{"op":"grayscale"}]}))).is_err());
    }

    #[test]
    fn recalculation_requires_an_explicit_xlsx_action() {
        for (source, output, spec, allowed) in [
            ("a.xlsx", "b.xlsx", json!({}), false),
            ("a.xlsx", "b.xlsx", json!({"recalculate":true}), true),
            ("a.xlsx", "b.pdf", json!({"recalculate":true}), false),
            ("a.docx", "b.docx", json!({"recalculate":true}), false),
            ("a.xlsx", "b.xlsx", json!({"recalculate":"true"}), false),
        ] {
            let tool = call(
                "document_write",
                json!({"operation":"convert","inputs":[{"folder_id":"f","path":source}],"output":{"path":output},"spec":spec}),
            );
            assert_eq!(parse_document_call(&tool).is_ok(), allowed, "{tool:?}");
        }
    }
}
