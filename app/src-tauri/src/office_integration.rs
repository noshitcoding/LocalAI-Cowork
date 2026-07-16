use crate::artifact_pipeline::{self, PdfPreviewPage};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const POWERSHELL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeOpenRequest {
    pub path: String,
    pub app_kind: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPreviewRequest {
    pub path: String,
    pub max_pages: Option<usize>,
    pub target_width: Option<i32>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OfficeAppInfo {
    pub kind: String,
    pub display_name: String,
    pub executable_path: Option<String>,
    pub available: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeDetectResponse {
    pub apps: Vec<OfficeAppInfo>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeOpenResponse {
    pub path: String,
    pub format: String,
    pub office_app: String,
    pub executable_path: Option<String>,
    pub launched: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPreviewResponse {
    pub source_path: String,
    pub format: String,
    pub preview_dir: String,
    pub exported_pdf_path: Option<String>,
    pub office_app: Option<String>,
    pub pages: Vec<PdfPreviewPage>,
    pub warnings: Vec<String>,
    pub generated_at: String,
}

pub fn detect_office_apps() -> OfficeDetectResponse {
    let apps = vec![
        detect_app("word", "Microsoft Word", "WINWORD.EXE"),
        detect_app("powerpoint", "Microsoft PowerPoint", "POWERPNT.EXE"),
        detect_app("excel", "Microsoft Excel", "EXCEL.EXE"),
        detect_app("libreoffice", "LibreOffice", "soffice.exe"),
    ];

    let mut warnings = Vec::new();
    if !cfg!(target_os = "windows") {
        warnings.push("Office-Automation ist aktuell nur unter Windows aktiv.".to_string());
    }
    if apps
        .iter()
        .filter(|app| app.kind != "libreoffice")
        .all(|app| !app.available)
    {
        warnings.push(
            "Microsoft Office was not found; direct Word/PowerPoint/Excel automation is not available."
                .to_string(),
        );
    }

    OfficeDetectResponse { apps, warnings }
}

pub fn open_document(path: &Path, app_kind: Option<&str>) -> Result<OfficeOpenResponse, String> {
    let format = document_format(path)?;
    let mut warnings = Vec::new();
    let office_kind = app_kind
        .and_then(|value| normalize_app_kind(value).map(|value| value.to_string()))
        .or_else(|| office_kind_for_format(&format).map(|value| value.to_string()));

    if format == "pdf" && office_kind.is_none() {
        open_with_default_app(path)?;
        return Ok(OfficeOpenResponse {
            path: path.display().to_string(),
            format,
            office_app: "default-pdf-viewer".to_string(),
            executable_path: None,
            launched: true,
            warnings,
        });
    }

    let office_kind = office_kind.ok_or_else(|| {
        format!(
            "No Office program is known for format '{}' (supported: docx, pptx, xlsx, pdf).",
            format
        )
    })?;
    let app = detect_app_for_kind(&office_kind)
        .ok_or_else(|| format!("Unbekannte Office-App: {}", office_kind))?;
    if !app.available {
        return Err(format!(
            "{} was not found; install Microsoft Office or open the file manually.",
            app.display_name
        ));
    }
    let exe = app
        .executable_path
        .clone()
        .ok_or_else(|| format!("{} has no launchable path.", app.display_name))?;

    Command::new(&exe)
        .arg(path)
        .spawn()
        .map_err(|err| format!("{} could not be started: {}", app.display_name, err))?;

    if format == "pdf" {
        warnings.push(
            "PDF was opened in the requested Office app; PDF editing is not guaranteed."
                .to_string(),
        );
    }

    Ok(OfficeOpenResponse {
        path: path.display().to_string(),
        format,
        office_app: app.kind,
        executable_path: Some(exe),
        launched: true,
        warnings,
    })
}

pub fn render_document_preview(
    source_path: &Path,
    app_data_dir: &Path,
    max_pages: Option<usize>,
    target_width: Option<i32>,
) -> Result<DocumentPreviewResponse, String> {
    let format = document_format(source_path)?;
    let preview_dir = preview_cache_dir(app_data_dir, source_path)?;
    if preview_dir.exists() {
        fs::remove_dir_all(&preview_dir).map_err(|err| err.to_string())?;
    }
    fs::create_dir_all(&preview_dir).map_err(|err| err.to_string())?;

    let mut warnings = Vec::new();
    let max_pages = max_pages.unwrap_or(6).clamp(1, 24);
    let target_width = target_width.unwrap_or(1200).clamp(400, 2400);

    let (pdf_path, exported_pdf_path, office_app) = if format == "pdf" {
        (source_path.to_path_buf(), None, None)
    } else {
        let office_kind = office_kind_for_format(&format)
            .ok_or_else(|| format!("Preview for format '{}' is not supported.", format))?;
        let app = detect_app_for_kind(office_kind)
            .ok_or_else(|| format!("Unbekannte Office-App: {}", office_kind))?;
        if !app.available {
            return Err(format!(
                "{} was not found; Office files cannot be rendered as previews without Office.",
                app.display_name
            ));
        }

        let pdf_path = preview_dir.join("office-export.pdf");
        export_office_to_pdf(source_path, &pdf_path, office_kind)?;
        (
            pdf_path.clone(),
            Some(pdf_path.display().to_string()),
            Some(app.kind),
        )
    };

    let pages_dir = preview_dir.join("pages");
    let pages =
        artifact_pipeline::render_pdf_pages(&pdf_path, &pages_dir, max_pages, target_width)?;
    if pages.len() >= max_pages {
        warnings.push(format!("Preview auf {} Seite(n) begrenzt.", max_pages));
    }

    Ok(DocumentPreviewResponse {
        source_path: source_path.display().to_string(),
        format,
        preview_dir: preview_dir.display().to_string(),
        exported_pdf_path,
        office_app,
        pages,
        warnings,
        generated_at: chrono::Utc::now().to_rfc3339(),
    })
}

fn document_format(path: &Path) -> Result<String, String> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "doc" | "docx" => Ok("docx".to_string()),
        "ppt" | "pptx" => Ok("pptx".to_string()),
        "xls" | "xlsx" => Ok("xlsx".to_string()),
        "pdf" => Ok("pdf".to_string()),
        other => Err(format!(
            "Unsupported document format '{}'; allowed formats are pdf, docx, pptx, xlsx.",
            other
        )),
    }
}

fn office_kind_for_format(format: &str) -> Option<&'static str> {
    match format {
        "docx" => Some("word"),
        "pptx" => Some("powerpoint"),
        "xlsx" => Some("excel"),
        _ => None,
    }
}

fn normalize_app_kind(value: &str) -> Option<&'static str> {
    match value.trim().to_lowercase().as_str() {
        "word" | "winword" | "docx" => Some("word"),
        "powerpoint" | "powerpnt" | "pptx" => Some("powerpoint"),
        "excel" | "xlsx" => Some("excel"),
        _ => None,
    }
}

fn detect_app_for_kind(kind: &str) -> Option<OfficeAppInfo> {
    match kind {
        "word" => Some(detect_app("word", "Microsoft Word", "WINWORD.EXE")),
        "powerpoint" => Some(detect_app(
            "powerpoint",
            "Microsoft PowerPoint",
            "POWERPNT.EXE",
        )),
        "excel" => Some(detect_app("excel", "Microsoft Excel", "EXCEL.EXE")),
        "libreoffice" => Some(detect_app("libreoffice", "LibreOffice", "soffice.exe")),
        _ => None,
    }
}

fn detect_app(kind: &str, display_name: &str, exe_name: &str) -> OfficeAppInfo {
    let executable_path = known_executable_candidates(kind, exe_name)
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| find_in_path(exe_name));

    OfficeAppInfo {
        kind: kind.to_string(),
        display_name: display_name.to_string(),
        executable_path: executable_path
            .as_ref()
            .map(|path| path.display().to_string()),
        available: executable_path.is_some(),
    }
}

fn known_executable_candidates(kind: &str, exe_name: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for env_key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = env::var_os(env_key).map(PathBuf::from) {
            match kind {
                "word" | "powerpoint" | "excel" => {
                    for version in ["Office16", "Office15", "Office14"] {
                        candidates.push(
                            root.join("Microsoft Office")
                                .join("root")
                                .join(version)
                                .join(exe_name),
                        );
                        candidates.push(root.join("Microsoft Office").join(version).join(exe_name));
                    }
                }
                "libreoffice" => {
                    candidates.push(root.join("LibreOffice").join("program").join(exe_name));
                }
                _ => {}
            }
        }
    }
    candidates
}

fn find_in_path(exe_name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(exe_name))
        .find(|candidate| candidate.is_file())
}

fn preview_cache_dir(app_data_dir: &Path, source_path: &Path) -> Result<PathBuf, String> {
    let metadata = fs::metadata(source_path).map_err(|err| err.to_string())?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let mut hasher = DefaultHasher::new();
    source_path.display().to_string().hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    modified.hash(&mut hasher);
    let hash = hasher.finish();
    let stem = source_path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(sanitize_stem)
        .unwrap_or_else(|| "document".to_string());

    Ok(app_data_dir
        .join("document_previews")
        .join(format!("{}-{:x}", stem, hash)))
}

fn sanitize_stem(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if sanitized.is_empty() {
        "document".to_string()
    } else {
        sanitized
    }
}

fn export_office_to_pdf(
    source_path: &Path,
    pdf_path: &Path,
    office_kind: &str,
) -> Result<(), String> {
    if !cfg!(target_os = "windows") {
        return Err("Office-PDF-Export ist aktuell nur unter Windows available.".to_string());
    }
    if let Some(parent) = pdf_path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }

    let script = match office_kind {
        "word" => office_word_export_script(source_path, pdf_path),
        "powerpoint" => office_powerpoint_export_script(source_path, pdf_path),
        "excel" => office_excel_export_script(source_path, pdf_path),
        _ => {
            return Err(format!(
                "Office PDF export for '{}' is not supported.",
                office_kind
            ))
        }
    };

    run_powershell_script(&script)?;
    if !pdf_path.is_file() {
        return Err("Office export did not create a PDF file.".to_string());
    }
    Ok(())
}

fn office_word_export_script(source_path: &Path, pdf_path: &Path) -> String {
    format!(
        r#"
$ErrorActionPreference = 'Stop'
$source = {source}
$pdf = {pdf}
if (Test-Path -LiteralPath $pdf) {{ Remove-Item -LiteralPath $pdf -Force }}
$app = $null
$doc = $null
try {{
  $app = New-Object -ComObject Word.Application
  $app.Visible = $false
  $app.DisplayAlerts = 0
  $doc = $app.Documents.Open($source, $false, $true)
  $doc.ExportAsFixedFormat($pdf, 17)
}} finally {{
  if ($doc -ne $null) {{ $doc.Close($false) | Out-Null }}
  if ($app -ne $null) {{ $app.Quit() | Out-Null }}
}}
"#,
        source = ps_quote_path(source_path),
        pdf = ps_quote_path(pdf_path),
    )
}

fn office_powerpoint_export_script(source_path: &Path, pdf_path: &Path) -> String {
    format!(
        r#"
$ErrorActionPreference = 'Stop'
$source = {source}
$pdf = {pdf}
if (Test-Path -LiteralPath $pdf) {{ Remove-Item -LiteralPath $pdf -Force }}
$app = $null
$presentation = $null
try {{
  $app = New-Object -ComObject PowerPoint.Application
  $presentation = $app.Presentations.Open($source, 0, 0, 0)
  $presentation.SaveAs($pdf, 32)
}} finally {{
  if ($presentation -ne $null) {{ $presentation.Close() | Out-Null }}
  if ($app -ne $null) {{ $app.Quit() | Out-Null }}
}}
"#,
        source = ps_quote_path(source_path),
        pdf = ps_quote_path(pdf_path),
    )
}

fn office_excel_export_script(source_path: &Path, pdf_path: &Path) -> String {
    format!(
        r#"
$ErrorActionPreference = 'Stop'
$source = {source}
$pdf = {pdf}
if (Test-Path -LiteralPath $pdf) {{ Remove-Item -LiteralPath $pdf -Force }}
$app = $null
$workbook = $null
try {{
  $app = New-Object -ComObject Excel.Application
  $app.Visible = $false
  $app.DisplayAlerts = $false
  $workbook = $app.Workbooks.Open($source, 3, $true)
  $workbook.ExportAsFixedFormat(0, $pdf)
}} finally {{
  if ($workbook -ne $null) {{ $workbook.Close($false) | Out-Null }}
  if ($app -ne $null) {{ $app.Quit() | Out-Null }}
}}
"#,
        source = ps_quote_path(source_path),
        pdf = ps_quote_path(pdf_path),
    )
}

fn open_with_default_app(path: &Path) -> Result<(), String> {
    let script = format!("Start-Process -LiteralPath {}", ps_quote_path(path));
    run_powershell_script(&script).map(|_| ())
}

fn ps_quote_path(path: &Path) -> String {
    ps_quote(&path.display().to_string())
}

fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn run_powershell_script(script: &str) -> Result<String, String> {
    let mut command = Command::new("powershell");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-STA",
        "-ExecutionPolicy",
        "RemoteSigned",
        "-Command",
        script,
    ]);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to launch powershell: {}", err))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|err| format!("failed to read powershell output: {}", err))?;
                if output.status.success() {
                    return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
                }

                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let details = if stderr.is_empty() { stdout } else { stderr };
                return Err(format!("powershell command failed: {}", details));
            }
            Ok(None) => {
                if started.elapsed() >= POWERSHELL_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "powershell command timed out after {} seconds",
                        POWERSHELL_TIMEOUT.as_secs()
                    ));
                }
                thread::sleep(Duration::from_millis(200));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("failed to wait for powershell command: {}", err));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_document_formats_to_office_apps() {
        assert_eq!(document_format(Path::new("report.docx")).unwrap(), "docx");
        assert_eq!(office_kind_for_format("docx"), Some("word"));
        assert_eq!(office_kind_for_format("pptx"), Some("powerpoint"));
        assert_eq!(office_kind_for_format("xlsx"), Some("excel"));
        assert_eq!(office_kind_for_format("pdf"), None);
    }

    #[test]
    fn sanitizes_preview_cache_stems() {
        assert_eq!(sanitize_stem("Q2 Report (final)"), "Q2_Report__final");
        assert_eq!(sanitize_stem("..."), "document");
    }
}
