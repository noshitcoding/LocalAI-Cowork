use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use pdfium_render::prelude::*;
use quick_xml::{
    escape::unescape,
    events::{BytesText, Event},
    Reader, Writer as XmlWriter,
};
use serde_json::{json, Value};
use tokio::{process::Command, time::timeout};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

use crate::ManagedProcessTree;

const ACTIVE_EXTENSIONS: &[&str] = &[
    "docm", "dotm", "xlsm", "xltm", "xlam", "pptm", "potm", "ppam", "sldm",
];
const MAX_OFFICE_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OFFICE_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

pub fn inspect(path: &Path) -> Result<Value> {
    reject_active_extension(path)?;
    match extension(path).as_str() {
        "docx" => inspect_ooxml(path, "word", |name| {
            name == "word/document.xml"
                || (name.starts_with("word/header") && name.ends_with(".xml"))
                || (name.starts_with("word/footer") && name.ends_with(".xml"))
        }),
        "xlsx" => inspect_ooxml(path, "excel", |name| {
            name == "xl/sharedStrings.xml"
                || (name.starts_with("xl/worksheets/") && name.ends_with(".xml"))
        }),
        "pptx" => inspect_ooxml(path, "powerpoint", |name| {
            name.starts_with("ppt/slides/slide") && name.ends_with(".xml")
        }),
        "pdf" => {
            let text = pdf_extract::extract_text(path).context("failed to extract PDF text")?;
            Ok(json!({
                "type":"pdf",
                "path":path,
                "text":truncate_chars(&text, 200_000),
            }))
        }
        "doc" | "xls" | "ppt" => bail!(
            "legacy Office formats must be converted to OOXML or inspected through LibreOffice"
        ),
        _ => bail!("supported Office formats are DOC/DOCX, XLS/XLSX, PPT/PPTX and PDF"),
    }
}

pub async fn inspect_document(path: &Path, temporary_root: &Path) -> Result<Value> {
    reject_active_extension(path)?;
    if !matches!(extension(path).as_str(), "doc" | "xls" | "ppt") {
        let source = path.to_owned();
        return tokio::task::spawn_blocking(move || inspect(&source))
            .await
            .context("Office inspection worker panicked")?;
    }

    let converted_extension = match extension(path).as_str() {
        "doc" => "docx",
        "xls" => "xlsx",
        "ppt" => "pptx",
        _ => unreachable!(),
    };
    let conversion_dir = temporary_root.join(format!("legacy-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&conversion_dir)?;
    let converted = conversion_dir.join(format!("converted.{converted_extension}"));

    #[cfg(windows)]
    let native_result = convert_with_microsoft_office(path, &converted).await;
    #[cfg(not(windows))]
    let native_result: Result<()> = Err(anyhow::anyhow!("Microsoft Office is unavailable"));
    let conversion_result = if native_result.is_ok() {
        Ok(())
    } else {
        convert_with_libreoffice(path, &converted, converted_extension)
            .await
            .with_context(|| format!("native Office conversion also failed: {native_result:?}"))
    };
    if let Err(error) = conversion_result {
        let _ = fs::remove_dir_all(&conversion_dir);
        return Err(error);
    }
    if !converted.is_file() {
        let _ = fs::remove_dir_all(&conversion_dir);
        bail!("legacy Office conversion did not create the expected OOXML document");
    }

    let converted_for_inspection = converted.clone();
    let inspected = tokio::task::spawn_blocking(move || inspect(&converted_for_inspection))
        .await
        .context("Office inspection worker panicked")?;
    let cleanup_result = fs::remove_dir_all(&conversion_dir);
    let mut result = inspected?;
    if let Some(object) = result.as_object_mut() {
        object.insert("path".to_owned(), json!(path));
        object.insert("converted_from".to_owned(), json!(extension(path)));
        object.insert("active_content".to_owned(), json!("disabled_and_stripped"));
    }
    cleanup_result.context("failed to remove temporary legacy Office conversion")?;
    Ok(result)
}

fn inspect_ooxml(path: &Path, kind: &str, include: impl Fn(&str) -> bool) -> Result<Value> {
    let file = File::open(path).context("failed to open Office document")?;
    let mut archive = ZipArchive::new(file).context("Office document is not valid OOXML")?;
    reject_active_archive(&mut archive)?;
    let mut parts = Vec::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if !include(entry.name()) {
            continue;
        }
        if entry.size() > MAX_OFFICE_ENTRY_BYTES {
            bail!("Office XML part exceeds the 64 MiB inspection limit");
        }
        total = total.saturating_add(entry.size());
        if total > MAX_OFFICE_TOTAL_BYTES {
            bail!("Office document exceeds the 512 MiB expanded inspection limit");
        }
        let name = entry.name().to_owned();
        let mut xml = String::new();
        entry
            .read_to_string(&mut xml)
            .with_context(|| format!("Office XML part {name} is not UTF-8"))?;
        parts.push(json!({"part":name,"text":truncate_chars(&xml_text(&xml)?, 200_000)}));
    }
    Ok(json!({"type":kind,"path":path,"parts":parts}))
}

pub fn replace_text(
    source: &Path,
    target: &Path,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
) -> Result<Value> {
    reject_active_extension(source)?;
    if old_text.is_empty() {
        bail!("old_text cannot be empty");
    }
    if !matches!(extension(source).as_str(), "docx" | "xlsx" | "pptx") {
        bail!("structured replacement requires DOCX, XLSX or PPTX");
    }
    if extension(source) != extension(target) {
        bail!("Office replacement output must keep the source format");
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = target.with_extension(format!("{}.cowork-tmp", extension(target)));
    let input = File::open(source)?;
    let mut archive = ZipArchive::new(input).context("Office document is not valid OOXML")?;
    reject_active_archive(&mut archive)?;
    let output = File::create(&temporary)?;
    let mut writer = ZipWriter::new(output);
    let mut replacements = 0_usize;
    let mut expanded = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        expanded = expanded.saturating_add(entry.size());
        if entry.size() > MAX_OFFICE_ENTRY_BYTES || expanded > MAX_OFFICE_TOTAL_BYTES {
            let _ = fs::remove_file(&temporary);
            bail!("Office document exceeds expanded archive safety limits");
        }
        let name = entry.name().to_owned();
        let options = SimpleFileOptions::default()
            .compression_method(entry.compression())
            .unix_permissions(entry.unix_mode().unwrap_or(0o644));
        if entry.is_dir() {
            writer.add_directory(name, options)?;
            continue;
        }
        writer.start_file(name.clone(), options)?;
        let mut bytes = Vec::with_capacity(entry.size().min(usize::MAX as u64) as usize);
        entry.read_to_end(&mut bytes)?;
        if editable_ooxml_part(&extension(source), &name) && (replace_all || replacements == 0) {
            if let Ok(xml) = std::str::from_utf8(&bytes) {
                let maximum = if replace_all { usize::MAX } else { 1 };
                let (updated, count) = replace_ooxml_text_runs(
                    xml,
                    old_text,
                    new_text,
                    maximum.saturating_sub(replacements),
                )?;
                if count > 0 {
                    replacements = replacements.saturating_add(count);
                    writer.write_all(&updated)?;
                    continue;
                }
            }
        }
        writer.write_all(&bytes)?;
    }
    writer.finish()?;
    drop(archive);
    if replacements == 0 {
        let _ = fs::remove_file(&temporary);
        bail!("old_text was not found in an OOXML text run");
    }
    if target.exists() {
        fs::remove_file(target)?;
    }
    fs::rename(&temporary, target)?;
    Ok(json!({"path":target,"replacements":replacements,"artifacts":[target]}))
}

#[derive(Debug)]
struct OoxmlTextNode {
    event_indices: Vec<usize>,
    group: usize,
    text: String,
}

fn editable_ooxml_part(kind: &str, name: &str) -> bool {
    match kind {
        "docx" => {
            name == "word/document.xml"
                || ((name.starts_with("word/header")
                    || name.starts_with("word/footer")
                    || name.starts_with("word/footnotes")
                    || name.starts_with("word/endnotes")
                    || name.starts_with("word/comments"))
                    && name.ends_with(".xml"))
        }
        "xlsx" => {
            name == "xl/sharedStrings.xml"
                || (name.starts_with("xl/worksheets/") && name.ends_with(".xml"))
        }
        "pptx" => {
            (name.starts_with("ppt/slides/slide") || name.starts_with("ppt/notesSlides/notesSlide"))
                && name.ends_with(".xml")
        }
        _ => false,
    }
}

fn replace_ooxml_text_runs(
    xml: &str,
    old_text: &str,
    new_text: &str,
    maximum: usize,
) -> Result<(Vec<u8>, usize)> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut events = Vec::new();
    let mut nodes = Vec::new();
    let mut group = 0_usize;
    let mut current_node: Option<OoxmlTextNode> = None;
    loop {
        let event = reader.read_event()?.into_owned();
        match &event {
            Event::Start(start) => {
                let local = start.local_name();
                if matches!(local.as_ref(), b"p" | b"si" | b"c") {
                    group = group.saturating_add(1);
                }
                if local.as_ref() == b"t" {
                    current_node = Some(OoxmlTextNode {
                        event_indices: Vec::new(),
                        group,
                        text: String::new(),
                    });
                }
            }
            Event::End(end) if end.local_name().as_ref() == b"t" => {
                if let Some(node) = current_node.take() {
                    if !node.event_indices.is_empty() {
                        nodes.push(node);
                    }
                }
            }
            Event::Text(text) if current_node.is_some() => {
                let decoded = text.decode()?;
                let visible = unescape(&decoded)?.into_owned();
                let node = current_node.as_mut().expect("checked above");
                node.event_indices.push(events.len());
                node.text.push_str(&visible);
            }
            Event::CData(text) if current_node.is_some() => {
                let visible = text.decode()?;
                let node = current_node.as_mut().expect("checked above");
                node.event_indices.push(events.len());
                node.text.push_str(&visible);
            }
            Event::GeneralRef(reference) if current_node.is_some() => {
                let encoded = format!("&{};", reference.decode()?);
                let visible = unescape(&encoded)?;
                let node = current_node.as_mut().expect("checked above");
                node.event_indices.push(events.len());
                node.text.push_str(&visible);
            }
            Event::Eof => {
                events.push(event);
                break;
            }
            _ => {}
        }
        events.push(event);
    }

    let mut replacements = 0_usize;
    let mut first = 0_usize;
    while first < nodes.len() && replacements < maximum {
        let current_group = nodes[first].group;
        let mut end = first + 1;
        while end < nodes.len() && nodes[end].group == current_group {
            end += 1;
        }
        let remaining = maximum.saturating_sub(replacements);
        let (updated, count) =
            replace_ooxml_group(&nodes[first..end], old_text, new_text, remaining);
        if count > 0 {
            replacements = replacements.saturating_add(count);
            for (node, text) in nodes[first..end].iter().zip(updated) {
                if let Some((first_event, remaining_events)) = node.event_indices.split_first() {
                    events[*first_event] = Event::Text(BytesText::new(&text).into_owned());
                    for event_index in remaining_events {
                        events[*event_index] = Event::Text(BytesText::new("").into_owned());
                    }
                }
            }
        }
        first = end;
    }

    let mut writer = XmlWriter::new(Vec::with_capacity(xml.len()));
    for event in events {
        writer.write_event(event)?;
    }
    Ok((writer.into_inner(), replacements))
}

fn replace_ooxml_group(
    nodes: &[OoxmlTextNode],
    old_text: &str,
    new_text: &str,
    maximum: usize,
) -> (Vec<String>, usize) {
    let joined = nodes
        .iter()
        .map(|node| node.text.as_str())
        .collect::<String>();
    let mut ranges = Vec::with_capacity(nodes.len());
    let mut offset = 0_usize;
    for node in nodes {
        let end = offset + node.text.len();
        ranges.push((offset, end));
        offset = end;
    }
    let mut outputs = vec![String::new(); nodes.len()];
    let mut cursor = 0_usize;
    let mut count = 0_usize;
    while count < maximum {
        let Some(relative) = joined[cursor..].find(old_text) else {
            break;
        };
        let start = cursor + relative;
        let end = start + old_text.len();
        distribute_ooxml_text(&joined, cursor, start, &ranges, &mut outputs);
        if let Some(index) = ranges
            .iter()
            .position(|(node_start, node_end)| start >= *node_start && start < *node_end)
        {
            outputs[index].push_str(new_text);
        }
        cursor = end;
        count += 1;
    }
    distribute_ooxml_text(&joined, cursor, joined.len(), &ranges, &mut outputs);
    (outputs, count)
}

fn distribute_ooxml_text(
    joined: &str,
    start: usize,
    end: usize,
    ranges: &[(usize, usize)],
    outputs: &mut [String],
) {
    if start >= end {
        return;
    }
    for (index, (node_start, node_end)) in ranges.iter().copied().enumerate() {
        let overlap_start = start.max(node_start);
        let overlap_end = end.min(node_end);
        if overlap_start < overlap_end {
            outputs[index].push_str(&joined[overlap_start..overlap_end]);
        }
    }
}

pub async fn export_pdf(source: &Path, target: &Path) -> Result<Value> {
    reject_active_extension(source)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if extension(source) == "pdf" {
        fs::copy(source, target)?;
        return Ok(json!({"path":target,"artifacts":[target]}));
    }

    #[cfg(windows)]
    let native_result = export_with_microsoft_office(source, target).await;
    #[cfg(not(windows))]
    let native_result: Result<()> = Err(anyhow::anyhow!("Microsoft Office is unavailable"));
    if native_result.is_err() {
        export_with_libreoffice(source, target)
            .await
            .with_context(|| format!("native Office export also failed: {native_result:?}"))?;
    }
    if !target.is_file() {
        bail!("Office export did not create the requested PDF");
    }
    Ok(json!({"path":target,"artifacts":[target]}))
}

pub fn preview_pdf(pdf: &Path, output_dir: &Path, all_pages: bool, dpi: u16) -> Result<Value> {
    fs::create_dir_all(output_dir)?;
    let bindings = match bind_pdfium() {
        Ok(bindings) => bindings,
        Err(pdfium_error) => {
            return preview_with_pdftoppm(pdf, output_dir, all_pages, dpi)
                .with_context(|| format!("PDFium preview failed: {pdfium_error}"));
        }
    };
    let pdfium = Pdfium::new(bindings);
    let document = pdfium
        .load_pdf_from_file(pdf, None)
        .context("failed to load PDF for preview")?;
    let width = ((dpi.clamp(72, 200) as f32 / 72.0) * 816.0) as i32;
    let config = PdfRenderConfig::new()
        .set_target_width(width)
        .set_maximum_height(width.saturating_mul(3));
    let limit = if all_pages { usize::MAX } else { 1 };
    let mut images = Vec::new();
    for (index, page) in document.pages().iter().take(limit).enumerate() {
        let path = output_dir.join(format!("page-{}.png", index + 1));
        page.render_with_config(&config)?
            .as_image()
            .into_rgb8()
            .save_with_format(&path, image::ImageFormat::Png)?;
        images.push(path);
    }
    Ok(json!({"pdf":pdf,"images":images,"artifacts":images}))
}

fn preview_with_pdftoppm(
    pdf: &Path,
    output_dir: &Path,
    all_pages: bool,
    dpi: u16,
) -> Result<Value> {
    let prefix = output_dir.join("page");
    let dpi = dpi.clamp(72, 200).to_string();
    let mut command = std::process::Command::new("pdftoppm");
    command.args(["-png", "-r", &dpi]);
    if !all_pages {
        command.args(["-f", "1", "-singlefile"]);
    }
    let status = command
        .arg(pdf)
        .arg(&prefix)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("failed to start pdftoppm")?;
    if !status.status.success() {
        bail!(
            "pdftoppm failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }
    let mut images = fs::read_dir(output_dir)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("png"))
        .collect::<Vec<_>>();
    images.sort();
    Ok(json!({"pdf":pdf,"images":images,"artifacts":images,"renderer":"pdftoppm"}))
}

pub async fn open_visible(path: &Path) -> Result<Value> {
    reject_active_extension(path)?;
    #[cfg(windows)]
    {
        let script = format!("Start-Process -LiteralPath {}", ps_quote_path(path));
        run_command(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", &script],
        )
        .await?;
    }
    #[cfg(target_os = "linux")]
    run_command("xdg-open", &[path.to_string_lossy().as_ref()]).await?;
    Ok(json!({"path":path,"launched":true}))
}

fn bind_pdfium() -> Result<Box<dyn PdfiumLibraryBindings>> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("COWORK_PDFIUM_PATH") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join(Pdfium::pdfium_platform_library_name()));
        }
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../app/src-tauri/resources/pdfium/bin")
            .join(Pdfium::pdfium_platform_library_name()),
    );
    for candidate in candidates {
        if candidate.is_file() {
            if let Ok(bindings) = Pdfium::bind_to_library(&candidate) {
                return Ok(bindings);
            }
        }
    }
    Pdfium::bind_to_system_library().context("PDFium is unavailable for Office previews")
}

fn xml_text(xml: &str) -> Result<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut result = String::new();
    let mut inside_text = false;
    loop {
        match reader.read_event()? {
            Event::Start(start) => {
                let local = start.local_name();
                if matches!(local.as_ref(), b"p" | b"si" | b"c")
                    && !result.is_empty()
                    && !result.ends_with('\n')
                {
                    result.push('\n');
                }
                if local.as_ref() == b"t" {
                    inside_text = true;
                }
            }
            Event::End(end) if end.local_name().as_ref() == b"t" => inside_text = false,
            Event::Text(text) if inside_text => {
                let decoded = text.decode()?;
                let text = unescape(&decoded)?;
                result.push_str(&text);
            }
            Event::CData(text) if inside_text => result.push_str(&text.decode()?),
            Event::GeneralRef(reference) if inside_text => {
                let encoded = format!("&{};", reference.decode()?);
                result.push_str(&unescape(&encoded)?);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(result)
}

fn reject_active_archive(archive: &mut ZipArchive<File>) -> Result<()> {
    for index in 0..archive.len() {
        let name = archive.by_index(index)?.name().to_ascii_lowercase();
        if name.ends_with("vbaproject.bin")
            || name.contains("/embeddings/")
            || name.contains("/activex/")
        {
            bail!("active Office content is blocked by the local executor policy");
        }
    }
    Ok(())
}

fn reject_active_extension(path: &Path) -> Result<()> {
    if ACTIVE_EXTENSIONS.contains(&extension(path).as_str()) {
        bail!("macro-enabled Office formats are blocked by policy");
    }
    Ok(())
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

async fn export_with_libreoffice(source: &Path, target: &Path) -> Result<()> {
    let output_dir = target.parent().context("PDF target has no parent")?;
    let program = if cfg!(windows) {
        "soffice.exe"
    } else {
        "libreoffice"
    };
    run_command(
        program,
        &[
            "--headless",
            "--safe-mode",
            "--nologo",
            "--nodefault",
            "--nofirststartwizard",
            "--nolockcheck",
            "--invisible",
            "--convert-to",
            "pdf",
            "--outdir",
            output_dir.to_string_lossy().as_ref(),
            source.to_string_lossy().as_ref(),
        ],
    )
    .await?;
    let generated = output_dir.join(format!(
        "{}.pdf",
        source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("office")
    ));
    if generated != target {
        if target.exists() {
            fs::remove_file(target)?;
        }
        fs::rename(generated, target)?;
    }
    Ok(())
}

async fn convert_with_libreoffice(
    source: &Path,
    target: &Path,
    target_extension: &str,
) -> Result<()> {
    let output_dir = target.parent().context("Office target has no parent")?;
    let program = if cfg!(windows) {
        "soffice.exe"
    } else {
        "libreoffice"
    };
    run_command(
        program,
        &[
            "--headless",
            "--safe-mode",
            "--nologo",
            "--nodefault",
            "--nofirststartwizard",
            "--nolockcheck",
            "--invisible",
            "--convert-to",
            target_extension,
            "--outdir",
            output_dir.to_string_lossy().as_ref(),
            source.to_string_lossy().as_ref(),
        ],
    )
    .await?;
    let generated = output_dir.join(format!(
        "{}.{}",
        source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("converted"),
        target_extension
    ));
    if generated != target {
        if target.exists() {
            fs::remove_file(target)?;
        }
        fs::rename(generated, target)?;
    }
    Ok(())
}

#[cfg(windows)]
async fn convert_with_microsoft_office(source: &Path, target: &Path) -> Result<()> {
    let source_value = ps_quote_path(source);
    let target_value = ps_quote_path(target);
    let script = match extension(source).as_str() {
        "doc" => format!(
            "$ErrorActionPreference='Stop';$app=$null;$doc=$null;try{{$app=New-Object -ComObject Word.Application;$app.Visible=$false;$app.DisplayAlerts=0;$app.AutomationSecurity=3;$doc=$app.Documents.Open({source_value},$false,$true);$doc.SaveAs2({target_value},16)}}finally{{if($doc-ne$null){{$doc.Close($false)|Out-Null}};if($app-ne$null){{$app.Quit()|Out-Null}}}}"
        ),
        "xls" => format!(
            "$ErrorActionPreference='Stop';$app=$null;$book=$null;try{{$app=New-Object -ComObject Excel.Application;$app.Visible=$false;$app.DisplayAlerts=$false;$app.AutomationSecurity=3;$book=$app.Workbooks.Open({source_value},0,$true);$book.SaveAs({target_value},51)}}finally{{if($book-ne$null){{$book.Close($false)|Out-Null}};if($app-ne$null){{$app.Quit()|Out-Null}}}}"
        ),
        "ppt" => format!(
            "$ErrorActionPreference='Stop';$app=$null;$deck=$null;try{{$app=New-Object -ComObject PowerPoint.Application;$app.AutomationSecurity=3;$deck=$app.Presentations.Open({source_value},0,0,0);$deck.SaveAs({target_value},24)}}finally{{if($deck-ne$null){{$deck.Close()|Out-Null}};if($app-ne$null){{$app.Quit()|Out-Null}}}}"
        ),
        _ => bail!("Microsoft Office cannot convert this legacy format"),
    };
    run_command(
        "powershell.exe",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    )
    .await
}

#[cfg(windows)]
async fn export_with_microsoft_office(source: &Path, target: &Path) -> Result<()> {
    let source_value = ps_quote_path(source);
    let target_value = ps_quote_path(target);
    let script = match extension(source).as_str() {
        "doc" | "docx" => format!(
            "$ErrorActionPreference='Stop';$app=$null;$doc=$null;try{{$app=New-Object -ComObject Word.Application;$app.Visible=$false;$app.DisplayAlerts=0;$app.AutomationSecurity=3;$doc=$app.Documents.Open({source_value},$false,$true);$doc.ExportAsFixedFormat({target_value},17)}}finally{{if($doc-ne$null){{$doc.Close($false)|Out-Null}};if($app-ne$null){{$app.Quit()|Out-Null}}}}"
        ),
        "xls" | "xlsx" => format!(
            "$ErrorActionPreference='Stop';$app=$null;$book=$null;try{{$app=New-Object -ComObject Excel.Application;$app.Visible=$false;$app.DisplayAlerts=$false;$app.AutomationSecurity=3;$book=$app.Workbooks.Open({source_value},0,$true);$book.ExportAsFixedFormat(0,{target_value})}}finally{{if($book-ne$null){{$book.Close($false)|Out-Null}};if($app-ne$null){{$app.Quit()|Out-Null}}}}"
        ),
        "ppt" | "pptx" => format!(
            "$ErrorActionPreference='Stop';$app=$null;$deck=$null;try{{$app=New-Object -ComObject PowerPoint.Application;$app.AutomationSecurity=3;$deck=$app.Presentations.Open({source_value},0,0,0);$deck.SaveAs({target_value},32)}}finally{{if($deck-ne$null){{$deck.Close()|Out-Null}};if($app-ne$null){{$app.Quit()|Out-Null}}}}"
        ),
        _ => bail!("Microsoft Office cannot export this format"),
    };
    run_command(
        "powershell.exe",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    )
    .await
}

#[cfg(windows)]
fn ps_quote_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

async fn run_command(program: &str, arguments: &[&str]) -> Result<()> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let child = command
        .spawn()
        .with_context(|| format!("failed to start {program}"))?;
    let process_tree = ManagedProcessTree::attach(&child)?;
    let status = match timeout(Duration::from_secs(180), child.wait_with_output()).await {
        Ok(status) => status?,
        Err(_) => {
            process_tree.terminate();
            bail!("{program} timed out and its process tree was terminated");
        }
    };
    drop(process_tree);
    if !status.status.success() {
        bail!(
            "{} failed: {}",
            program,
            String::from_utf8_lossy(&status.stderr)
                .chars()
                .take(4000)
                .collect::<String>()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{inspect, replace_text};
    use serde_json::Value;
    use std::{fs, io::Write};
    use zip::{write::SimpleFileOptions, ZipWriter};

    fn test_docx(path: &std::path::Path, text: &str) {
        test_docx_xml(path, &format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>"));
    }

    fn test_docx_xml(path: &std::path::Path, body: &str) {
        let file = fs::File::create(path).unwrap();
        let mut archive = ZipWriter::new(file);
        archive
            .start_file("[Content_Types].xml", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"<Types/>").unwrap();
        archive
            .start_file("word/document.xml", SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(format!("<w:document xmlns:w='x'>{body}</w:document>").as_bytes())
            .unwrap();
        archive.finish().unwrap();
    }

    #[test]
    fn ooxml_inspection_and_replacement_do_not_execute_active_content() {
        let root = std::env::temp_dir().join(format!("cowork-office-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.docx");
        let target = root.join("target.docx");
        test_docx(&source, "Hello durable Office");
        let inspected = inspect(&source).unwrap();
        assert!(inspected.to_string().contains("Hello durable Office"));
        replace_text(&source, &target, "durable", "background", true).unwrap();
        assert!(inspect(&target)
            .unwrap()
            .to_string()
            .contains("background Office"));

        let split_source = root.join("split-source.docx");
        let split_target = root.join("split-target.docx");
        test_docx_xml(
            &split_source,
            "<w:p><w:r><w:t>Hello dur</w:t></w:r><w:r><w:t>able &amp; sa</w:t></w:r><w:r><w:t>fe</w:t></w:r></w:p><w:p><w:r><w:t>Next paragraph</w:t></w:r></w:p>",
        );
        let split = replace_text(
            &split_source,
            &split_target,
            "durable & safe",
            "background & verified",
            true,
        )
        .unwrap();
        assert_eq!(split.get("replacements").and_then(Value::as_u64), Some(1));
        let split_inspection = inspect(&split_target).unwrap().to_string();
        assert!(split_inspection.contains("Hello background & verified"));
        assert!(split_inspection.contains("Next paragraph"));

        let boundary_target = root.join("boundary-target.docx");
        assert!(replace_text(
            &split_source,
            &boundary_target,
            "safeNext",
            "must-not-cross",
            true,
        )
        .is_err());
        assert!(!boundary_target.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
