use pdfium_render::prelude::*;
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static PDFIUM_SEARCH_PATHS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactParseResponse {
    pub path: String,
    pub format: String,
    pub size_bytes: u64,
    pub summary: String,
    pub preview: String,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfPreviewPage {
    pub page_number: usize,
    pub image_path: String,
    pub width: u32,
    pub height: u32,
}

pub fn parse_artifact(path: &Path) -> Result<ArtifactParseResponse, String> {
    let metadata = fs::metadata(path).map_err(|err| err.to_string())?;
    let size_bytes = metadata.len();
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "txt" | "md" | "log" | "rs" | "ts" | "tsx" | "js" | "jsx" | "py" => {
            parse_text_like(path, "text/plain", size_bytes)
        }
        "csv" => parse_csv(path, size_bytes),
        "json" => parse_json(path, size_bytes),
        "ipynb" => parse_ipynb(path, size_bytes),
        "xml" => parse_xml(path, size_bytes),
        "html" | "htm" => parse_html(path, size_bytes),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => parse_image(path, size_bytes),
        "svg" => parse_svg(path, size_bytes),
        "pdf" => parse_pdf(path, size_bytes),
        "docx" => parse_docx(path, size_bytes),
        "xlsx" => parse_xlsx(path, size_bytes),
        "pptx" => parse_pptx(path, size_bytes),
        _ => parse_binary(path, size_bytes),
    }
}

pub fn render_pdf_pages(
    path: &Path,
    output_dir: &Path,
    max_pages: usize,
    target_width: i32,
) -> Result<Vec<PdfPreviewPage>, String> {
    fs::create_dir_all(output_dir).map_err(|err| err.to_string())?;

    let pdfium = bind_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|err| err.to_string())?;
    let render_config = PdfRenderConfig::new()
        .set_target_width(target_width)
        .set_maximum_height(target_width * 2);

    let mut pages = Vec::new();
    for (index, page) in document.pages().iter().take(max_pages.max(1)).enumerate() {
        let page_number = index + 1;
        let image_path = output_dir.join(format!("page-{}.png", page_number));
        let bitmap = page
            .render_with_config(&render_config)
            .map_err(|err| err.to_string())?;
        let width = bitmap.width() as u32;
        let height = bitmap.height() as u32;
        bitmap
            .as_image()
            .into_rgb8()
            .save_with_format(&image_path, image::ImageFormat::Png)
            .map_err(|err| err.to_string())?;
        pages.push(PdfPreviewPage {
            page_number,
            image_path: image_path.display().to_string(),
            width,
            height,
        });
    }

    Ok(pages)
}

pub fn extract_text_for_llm(path: &Path) -> Result<String, String> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "txt" | "md" | "log" | "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "csv" | "json"
        | "xml" | "html" | "htm" => fs::read_to_string(path).map_err(|err| err.to_string()),
        "ipynb" => {
            let raw = fs::read_to_string(path).map_err(|err| err.to_string())?;
            let parsed: serde_json::Value =
                serde_json::from_str(&raw).map_err(|err| err.to_string())?;
            let mut out = String::new();
            if let Some(cells) = parsed.get("cells").and_then(|value| value.as_array()) {
                for cell in cells {
                    let source = cell
                        .get("source")
                        .and_then(|value| value.as_array())
                        .map(|lines| {
                            lines
                                .iter()
                                .filter_map(|line| line.as_str())
                                .collect::<Vec<_>>()
                                .join("")
                        })
                        .unwrap_or_default();
                    out.push_str(&source);
                    out.push_str("\n\n");
                }
            }
            Ok(out)
        }
        "svg" => fs::read_to_string(path).map_err(|err| err.to_string()),
        "docx" => {
            let xml = read_zip_entry_text(path, "word/document.xml")?;
            Ok(strip_tags(&xml))
        }
        "xlsx" => {
            let shared = read_zip_entry_text(path, "xl/sharedStrings.xml")
                .or_else(|_| read_zip_entry_text(path, "xl/worksheets/sheet1.xml"))?;
            Ok(strip_tags(&shared))
        }
        "pptx" => {
            let file = fs::File::open(path).map_err(|err| err.to_string())?;
            let mut archive = zip::ZipArchive::new(file).map_err(|err| err.to_string())?;
            let mut combined = String::new();
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).map_err(|err| err.to_string())?;
                let name = entry.name().to_string();
                if !name.starts_with("ppt/slides/slide") || !name.ends_with(".xml") {
                    continue;
                }
                let mut xml = String::new();
                entry
                    .read_to_string(&mut xml)
                    .map_err(|err| err.to_string())?;
                combined.push_str(&strip_tags(&xml));
                combined.push_str("\n\n");
            }
            Ok(combined)
        }
        "pdf" => safe_extract_pdf_text(path),
        _ => Err("format does not provide text extraction".to_string()),
    }
}

pub fn extract_text_for_llm_limited(
    path: &Path,
    max_chars: usize,
) -> Result<(String, bool), String> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "pdf" {
        return safe_extract_pdf_text_limited(path, Some(max_chars));
    }

    let text = extract_text_for_llm(path)?;
    Ok(truncate_text(text, max_chars))
}

pub fn set_pdfium_search_paths(paths: Vec<PathBuf>) {
    let search_paths = PDFIUM_SEARCH_PATHS.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut guard) = search_paths.lock() {
        *guard = paths;
    }
}

fn safe_extract_pdf_text(path: &Path) -> Result<String, String> {
    let (text, _) = safe_extract_pdf_text_limited(path, None)?;
    Ok(text)
}

fn safe_extract_pdf_text_limited(
    path: &Path,
    max_chars: Option<usize>,
) -> Result<(String, bool), String> {
    match extract_pdf_text_with_pdfium(path, max_chars) {
        Ok((text, truncated)) if !text.trim().is_empty() => return Ok((text, truncated)),
        Ok(_) => {}
        Err(_) => {}
    }

    match catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text(path))) {
        Ok(Ok(text)) => Ok(match max_chars {
            Some(limit) => truncate_text(text, limit),
            None => (text, false),
        }),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err("PDF text could not be extracted: parser crashed".to_string()),
    }
}

fn truncate_text(text: String, max_chars: usize) -> (String, bool) {
    let mut chars = text.chars();
    let limited: String = chars.by_ref().take(max_chars).collect();
    let truncated = chars.next().is_some();
    (limited, truncated)
}

fn extract_pdf_text_with_pdfium(
    path: &Path,
    max_chars: Option<usize>,
) -> Result<(String, bool), String> {
    let pdfium = bind_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|err| err.to_string())?;
    let mut output = String::new();
    let mut truncated = false;

    for (index, page) in document.pages().iter().enumerate() {
        let text = page.text().map_err(|err| err.to_string())?.all();
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(&format!("--- Seite {} ---\n", index + 1));
        output.push_str(&text);

        if let Some(limit) = max_chars {
            let char_count = output.chars().count();
            if char_count >= limit {
                let (limited, _) = truncate_text(output, limit);
                output = limited;
                truncated = true;
                break;
            }
        }
    }

    Ok((output, truncated))
}

fn bind_pdfium() -> Result<Pdfium, String> {
    let mut errors = Vec::new();

    for candidate in pdfium_library_candidates() {
        if !candidate.is_file() {
            continue;
        }

        match Pdfium::bind_to_library(&candidate) {
            Ok(bindings) => return Ok(Pdfium::new(bindings)),
            Err(err) => errors.push(format!("{}: {}", candidate.display(), err)),
        }
    }

    match Pdfium::bind_to_system_library() {
        Ok(bindings) => Ok(Pdfium::new(bindings)),
        Err(err) => {
            errors.push(format!("system library: {}", err));
            Err(format!(
                "Pdfium could not be loaded ({})",
                errors.join("; ")
            ))
        }
    }
}

fn pdfium_library_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(search_paths) = PDFIUM_SEARCH_PATHS.get() {
        if let Ok(guard) = search_paths.lock() {
            candidates.extend(guard.iter().cloned());
        }
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            candidates.push(exe_dir.join("pdfium.dll"));
            candidates.push(
                exe_dir
                    .join("resources")
                    .join("pdfium")
                    .join("bin")
                    .join("pdfium.dll"),
            );
            candidates.push(exe_dir.join("pdfium").join("bin").join("pdfium.dll"));
        }
    }

    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(
            current_dir
                .join("resources")
                .join("pdfium")
                .join("bin")
                .join("pdfium.dll"),
        );
        candidates.push(
            current_dir
                .join("src-tauri")
                .join("resources")
                .join("pdfium")
                .join("bin")
                .join("pdfium.dll"),
        );
    }

    candidates.sort();
    candidates.dedup();
    candidates
}

fn read_text_limited(path: &Path, max_chars: usize) -> Result<String, String> {
    let content = fs::read_to_string(path).map_err(|err| err.to_string())?;
    Ok(content.chars().take(max_chars).collect())
}

fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut inside = false;

    for ch in input.chars() {
        match ch {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(ch),
            _ => {}
        }
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_text_like(
    path: &Path,
    format: &str,
    size_bytes: u64,
) -> Result<ArtifactParseResponse, String> {
    let preview = read_text_limited(path, 2000)?;
    let line_count = preview.lines().count();

    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: format.to_string(),
        size_bytes,
        summary: format!("Text file with preview, {} lines read", line_count),
        preview,
        metadata: json!({
            "lineCount": line_count,
        }),
    })
}

fn parse_csv(path: &Path, size_bytes: u64) -> Result<ArtifactParseResponse, String> {
    let preview = read_text_limited(path, 3000)?;
    let lines: Vec<&str> = preview.lines().collect();
    let line_count = lines.len();
    let first = lines.first().copied().unwrap_or("");
    let separators = [',', ';', '\t'];

    let mut best_count = 0usize;
    for sep in separators {
        let count = first.split(sep).count();
        if count > best_count {
            best_count = count;
        }
    }

    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: "text/csv".to_string(),
        size_bytes,
        summary: format!("CSV-Vorschau, erkannte Spalten im Header: {}", best_count),
        preview,
        metadata: json!({
            "lineCount": line_count,
            "headerColumns": best_count,
        }),
    })
}

fn parse_json(path: &Path, size_bytes: u64) -> Result<ArtifactParseResponse, String> {
    let raw = fs::read_to_string(path).map_err(|err| err.to_string())?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
    let pretty = serde_json::to_string_pretty(&parsed).map_err(|err| err.to_string())?;
    let preview: String = pretty.chars().take(3000).collect();

    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: "application/json".to_string(),
        size_bytes,
        summary: "JSON erfolgreich geparst".to_string(),
        preview,
        metadata: json!({
            "rootType": if parsed.is_object() { "object" } else if parsed.is_array() { "array" } else { "primitive" }
        }),
    })
}

fn parse_ipynb(path: &Path, size_bytes: u64) -> Result<ArtifactParseResponse, String> {
    let raw = fs::read_to_string(path).map_err(|err| err.to_string())?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).map_err(|err| err.to_string())?;

    let mut preview = String::new();
    let mut cells_count = 0usize;
    let mut markdown_count = 0usize;
    let mut code_count = 0usize;

    if let Some(cells) = parsed.get("cells").and_then(|value| value.as_array()) {
        cells_count = cells.len();

        for (index, cell) in cells.iter().take(6).enumerate() {
            let cell_type = cell
                .get("cell_type")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            if cell_type == "markdown" {
                markdown_count += 1;
            }
            if cell_type == "code" {
                code_count += 1;
            }

            let source = cell
                .get("source")
                .and_then(|value| value.as_array())
                .map(|lines| {
                    lines
                        .iter()
                        .filter_map(|line| line.as_str())
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();

            preview.push_str(&format!("[{}] {}\n{}\n\n", index + 1, cell_type, source));
        }
    }

    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: "application/x-ipynb+json".to_string(),
        size_bytes,
        summary: format!(
            "Notebook mit {} Zellen ({} markdown, {} code)",
            cells_count, markdown_count, code_count
        ),
        preview: preview.chars().take(3500).collect(),
        metadata: json!({
            "cells": cells_count,
            "markdownCells": markdown_count,
            "codeCells": code_count,
        }),
    })
}

fn parse_xml(path: &Path, size_bytes: u64) -> Result<ArtifactParseResponse, String> {
    let raw = read_text_limited(path, 5000)?;
    let clean = strip_tags(&raw);

    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: "application/xml".to_string(),
        size_bytes,
        summary: "XML als Textvorschau extrahiert".to_string(),
        preview: clean.chars().take(3000).collect(),
        metadata: json!({
            "rawPreviewChars": raw.len(),
        }),
    })
}

fn parse_html(path: &Path, size_bytes: u64) -> Result<ArtifactParseResponse, String> {
    let raw = read_text_limited(path, 6000)?;
    let clean = strip_tags(&raw);

    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: "text/html".to_string(),
        size_bytes,
        summary: "HTML in lesbare Textvorschau transformiert".to_string(),
        preview: clean.chars().take(3000).collect(),
        metadata: json!({
            "rawPreviewChars": raw.len(),
        }),
    })
}

fn parse_image(path: &Path, size_bytes: u64) -> Result<ArtifactParseResponse, String> {
    let (width, height) = image::image_dimensions(path).map_err(|err| err.to_string())?;

    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: "image/raster".to_string(),
        size_bytes,
        summary: format!("Image detected: {}x{}", width, height),
        preview: String::new(),
        metadata: json!({
            "width": width,
            "height": height,
        }),
    })
}

fn parse_svg(path: &Path, size_bytes: u64) -> Result<ArtifactParseResponse, String> {
    let raw = read_text_limited(path, 4000)?;

    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: "image/svg+xml".to_string(),
        size_bytes,
        summary: "SVG-Textvorschau extrahiert".to_string(),
        preview: raw,
        metadata: json!({}),
    })
}

fn parse_pdf(path: &Path, size_bytes: u64) -> Result<ArtifactParseResponse, String> {
    let extracted_text = safe_extract_pdf_text_limited(path, Some(3000)).ok();
    let preview = extracted_text
        .as_ref()
        .map(|(text, _)| text.clone())
        .unwrap_or_else(|| "PDF binary format: text preview not available".to_string());
    let preview_chars = preview.chars().count();

    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: "application/pdf".to_string(),
        size_bytes,
        summary: if extracted_text.is_some() {
            let truncated = extracted_text
                .as_ref()
                .map(|(_, value)| *value)
                .unwrap_or(false);
            format!(
                "PDF-Text extrahiert, Vorschauzeichen: {}{}",
                preview_chars,
                if truncated {
                    " (Vorschau gekuerzt)"
                } else {
                    ""
                }
            )
        } else {
            "PDF detected, text could not be extracted".to_string()
        },
        preview,
        metadata: json!({
            "textExtracted": extracted_text.is_some(),
        }),
    })
}

fn read_zip_entry_text(path: &Path, entry_name: &str) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|err| err.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|err| err.to_string())?;
    let mut entry = archive.by_name(entry_name).map_err(|err| err.to_string())?;
    let mut out = String::new();
    entry
        .read_to_string(&mut out)
        .map_err(|err| err.to_string())?;
    Ok(out)
}

fn parse_docx(path: &Path, size_bytes: u64) -> Result<ArtifactParseResponse, String> {
    let xml = read_zip_entry_text(path, "word/document.xml")?;
    let preview = strip_tags(&xml);

    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            .to_string(),
        size_bytes,
        summary: "DOCX-Inhalt aus document.xml extrahiert".to_string(),
        preview: preview.chars().take(3000).collect(),
        metadata: json!({}),
    })
}

fn parse_xlsx(path: &Path, size_bytes: u64) -> Result<ArtifactParseResponse, String> {
    let shared = read_zip_entry_text(path, "xl/sharedStrings.xml")
        .or_else(|_| read_zip_entry_text(path, "xl/worksheets/sheet1.xml"))?;
    let preview = strip_tags(&shared);

    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string(),
        size_bytes,
        summary: "XLSX-Vorschau aus OpenXML-Eintraegen extrahiert".to_string(),
        preview: preview.chars().take(3000).collect(),
        metadata: json!({}),
    })
}

fn parse_pptx(path: &Path, size_bytes: u64) -> Result<ArtifactParseResponse, String> {
    let slide = read_zip_entry_text(path, "ppt/slides/slide1.xml")?;
    let preview = strip_tags(&slide);

    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            .to_string(),
        size_bytes,
        summary: "PPTX-Vorschau aus erster Folie extrahiert".to_string(),
        preview: preview.chars().take(3000).collect(),
        metadata: json!({}),
    })
}

fn parse_binary(path: &Path, size_bytes: u64) -> Result<ArtifactParseResponse, String> {
    Ok(ArtifactParseResponse {
        path: path.display().to_string(),
        format: "application/octet-stream".to_string(),
        size_bytes,
        summary: "Unbekanntes Binaerformat, nur Metadaten available".to_string(),
        preview: String::new(),
        metadata: json!({}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document, Object, Stream};
    use uuid::Uuid;

    #[test]
    fn upgraded_pdf_extractor_reads_generated_text() {
        let path =
            std::env::temp_dir().join(format!("open_cowork_pdf_extract_{}.pdf", Uuid::new_v4()));
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let font_id = document.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let resources_id = document.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), 14.into()]),
                Operation::new("Td", vec![72.into(), 720.into()]),
                Operation::new(
                    "Tj",
                    vec![Object::string_literal("Open Cowork PDF regression")],
                ),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = document.add_object(Stream::new(
            dictionary! {},
            content.encode().expect("PDF content encodes"),
        ));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        document.objects.insert(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }
            .into(),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document.compress();
        document.save(&path).expect("PDF fixture saves");

        let extracted = pdf_extract::extract_text(&path).expect("PDF text extracts");
        assert!(extracted.contains("Open Cowork PDF regression"));

        let _ = fs::remove_file(path);
    }
}
