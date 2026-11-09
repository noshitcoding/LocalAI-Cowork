//! Native M9 selection: fixed cold-export files, held handles and immutable original requests.
use super::native_client::{NativeClient, failure};
use cowork_contracts::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Duration,
};
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
const CHUNK: usize = 1024 * 1024;
const JOURNAL_BYTES: u64 = 16 * 1024 * 1024;
#[derive(Default)]
pub struct HistoricalImportGate(pub tokio::sync::Mutex<()>);
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    stream: HistoricalStream,
    path: PathBuf,
    size: u64,
    sha256: String,
    identity: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    format: u32,
    request: BeginHistoricalImport,
    sources: Vec<Source>,
    request_sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenRequest {
    format: u32,
    path: String,
    method: String,
    input: Value,
    sha256: String,
}
struct Held {
    source: Source,
    file: File,
    _pins: Vec<File>,
}
fn invalid() -> ApiError {
    failure("historical_import_original_invalid")
}
fn io_error() -> ApiError {
    failure("historical_import_connection_failed")
}
fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn number(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 10
        && value.bytes().all(|v| v.is_ascii_digit())
        && value.parse::<u32>().is_ok()
}
fn source_segment(value: &str) -> bool {
    percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .is_ok_and(|s| {
            !s.is_empty()
                && s.len() <= 300
                && !s.chars().any(char::is_control)
                && !value.contains(['/', '?'])
        })
}
fn allowed(path: &str, method: &str, binary: bool) -> bool {
    if path.len() > 2048 || path.contains(['#', '\\', '\0']) {
        return false;
    }
    if let Some((base, query)) = path.split_once('?') {
        if method != "GET" || binary {
            return false;
        }
        let segments: Vec<_> = base.split('/').collect();
        let Some((offset, limit)) = query
            .strip_prefix("offset=")
            .and_then(|q| q.split_once("&limit="))
        else {
            return false;
        };
        let content = segments.len() == 4
            && segments[1] == "imports"
            && uuid(segments[2])
            && segments[3] == "content"
            && number(offset)
            && number(limit)
            && limit.parse::<u32>().is_ok_and(|v| v > 0 && v <= 100);
        let tables = matches!(segments.as_slice(),["","imports",id,"history","tables"] if uuid(id))
            && number(offset)
            && number(limit)
            && limit.parse::<u32>().is_ok_and(|n| n > 0 && n <= 10);
        let rows = matches!(segments.as_slice(),["","imports",id,"history","tables",table,"rows"] if uuid(id)&&number(table))
            && number(offset)
            && number(limit)
            && limit.parse::<u32>().is_ok_and(|n| n > 0 && n <= 25);
        let catalogue = matches!(segments.as_slice(),["","imports",id,"content","catalog"] if uuid(id))
            && number(offset)
            && number(limit)
            && limit.parse::<u32>().is_ok_and(|n| n > 0 && n <= 10);
        return content || tables || rows || catalogue;
    }
    let s: Vec<_> = path.split('/').collect();
    if s.first() != Some(&"") {
        return false;
    }
    match s.as_slice() {
        ["", "limit"] => !binary && matches!(method, "GET" | "POST"),
        ["", "targets"] => !binary && method == "GET",
        ["", "begin"] => !binary && method == "POST",
        ["", "imports", id] => uuid(id) && !binary && method == "GET",
        ["", "imports", id, "verify" | "preview"] => uuid(id) && !binary && method == "POST",
        ["", "imports", id, "content"] => uuid(id) && !binary && method == "GET",
        ["", "imports", id, "content", "catalog"] => uuid(id) && !binary && method == "GET",
        ["", "imports", id, "content", "catalog", ordinal, page] => {
            uuid(id) && number(ordinal) && number(page) && binary && method == "GET"
        }
        ["", "imports", id, "preview", "paged"] => uuid(id) && !binary && method == "POST",
        ["", "previews", id, "summary"] => uuid(id) && !binary && method == "GET",
        ["", "previews", id, "projects", ordinal, page] => {
            uuid(id) && number(ordinal) && number(page) && binary && method == "GET"
        }
        ["", "imports", id, "history"] => uuid(id) && !binary && method == "GET",
        ["", "imports", id, "history", "index"] => uuid(id) && !binary && method == "POST",
        ["", "imports", id, "history", "tables"] => uuid(id) && !binary && method == "GET",
        ["", "imports", id, "history", "tables", table, "rows"] => {
            uuid(id) && number(table) && !binary && method == "GET"
        }
        [
            "",
            "imports",
            id,
            "history",
            "tables",
            table,
            "rows",
            position,
            "cells",
            page,
        ] => {
            uuid(id)
                && number(table)
                && number(position)
                && number(page)
                && !binary
                && method == "GET"
        }
        [
            "",
            "imports",
            id,
            "history",
            "tables",
            table,
            "rows",
            position,
            "cells",
            column,
            "bytes",
            page,
        ] => {
            uuid(id)
                && number(table)
                && number(position)
                && number(column)
                && number(page)
                && binary
                && method == "GET"
        }
        ["", "imports", id, "content", source, page] => {
            uuid(id) && number(source) && number(page) && binary && method == "GET"
        }
        ["", "imports", id, "chunks", stream, index] => {
            uuid(id)
                && HistoricalStream::all()
                    .iter()
                    .any(|s| s.locator() == *stream)
                && number(index)
                && binary
                && matches!(method, "GET" | "POST")
        }
        ["", "previews", id] => uuid(id) && !binary && method == "GET",
        ["", "previews", id, "activate"] => uuid(id) && !binary && method == "POST",
        ["", "previews", id, "receipt"] => uuid(id) && !binary && method == "GET",
        ["", "previews", id, "files", source, "stage"] => {
            uuid(id) && source_segment(source) && !binary && method == "GET"
        }
        ["", "previews", id, "files", source, "close-unsent"] => {
            uuid(id) && source_segment(source) && !binary && method == "POST"
        }
        _ => false,
    }
}
fn route(path: &str) -> String {
    format!("/native/historical-import{path}")
}
async fn connected(window: &tauri::WebviewWindow) -> Result<NativeClient, ApiError> {
    if !super::application_window(window) {
        return Err(failure("forbidden"));
    }
    super::connection_info(
        window.app_handle().clone(),
        window.state::<tokio::sync::Mutex<()>>(),
    )
    .await
    .map_err(|_| io_error())?;
    NativeClient::new(
        window,
        "historical_import_connection_failed",
        "historical_import_operation_uncertain",
    )
}
async fn confirm(
    window: &tauri::WebviewWindow,
    german: bool,
    text: String,
) -> Result<bool, ApiError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .app_handle()
        .dialog()
        .message(text)
        .title(if german {
            "Historische Inhalte übernehmen"
        } else {
            "Import historical content"
        })
        .parent(window)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancel)
        .show(move |value| {
            let _ = sender.send(value);
        });
    receiver.await.map_err(|_| io_error())
}
async fn pick(
    window: &tauri::WebviewWindow,
    folder: bool,
    title: &str,
) -> Result<Option<PathBuf>, ApiError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let dialog = window
        .app_handle()
        .dialog()
        .file()
        .set_parent(window)
        .set_title(title);
    if folder {
        dialog.pick_folder(move |file| {
            let _ = sender.send(file);
        });
    } else {
        dialog.pick_file(move |file| {
            let _ = sender.send(file);
        });
    }
    receiver
        .await
        .map_err(|_| io_error())?
        .map(|file| file.into_path().map_err(|_| invalid()))
        .transpose()
}
fn ordinary(path: &Path, directory: bool) -> Result<(), ApiError> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| io_error())?;
    if meta.file_type().is_symlink()
        || meta.is_dir() != directory
        || (!directory && !meta.is_file())
    {
        return Err(invalid());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(invalid());
        }
    }
    Ok(())
}
fn identity(file: &File) -> Result<String, ApiError> {
    let meta = file.metadata().map_err(|_| io_error())?;
    if !meta.is_file() {
        return Err(invalid());
    }
    #[cfg(windows)]
    {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if meta.file_attributes() & 0x400 != 0
            || unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
            || info.nNumberOfLinks != 1
        {
            return Err(invalid());
        }
        Ok(format!(
            "windows:{}:{}:{}",
            info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
        ))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(invalid());
        }
        Ok(format!("unix:{}:{}", meta.dev(), meta.ino()))
    }
}
fn held(path: PathBuf, stream: HistoricalStream, maximum: u64) -> Result<Held, ApiError> {
    if !path.is_absolute() {
        return Err(invalid());
    }
    let mut pins = vec![];
    for parent in path
        .parent()
        .ok_or_else(invalid)?
        .ancestors()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        ordinary(parent, true)?;
        let mut options = OpenOptions::new();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options
                .access_mode(0x0002_0080)
                .share_mode(3)
                .custom_flags(0x0220_0000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
        }
        let file = options.open(parent).map_err(|_| io_error())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            let m = file.metadata().map_err(|_| io_error())?;
            if !m.is_dir() || m.file_attributes() & 0x400 != 0 {
                return Err(invalid());
            }
        }
        pins.push(file);
    }
    ordinary(&path, false)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(&path).map_err(|_| io_error())?;
    let meta = file.metadata().map_err(|_| io_error())?;
    let id = identity(&file)?;
    if meta.len() > maximum {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::symlink_metadata(&path).map_err(|_| io_error())?;
        if m.dev() != meta.dev() || m.ino() != meta.ino() {
            return Err(invalid());
        }
    }
    Ok(Held {
        source: Source {
            stream,
            path,
            size: meta.len(),
            sha256: String::new(),
            identity: id,
        },
        file,
        _pins: pins,
    })
}
fn digest(source: &mut Held) -> Result<String, ApiError> {
    if identity(&source.file)? != source.source.identity
        || source.file.metadata().map_err(|_| io_error())?.len() != source.source.size
    {
        return Err(invalid());
    }
    source
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|_| io_error())?;
    let mut h = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    let mut bytes = 0u64;
    loop {
        let n = source.file.read(&mut buffer).map_err(|_| io_error())?;
        if n == 0 {
            break;
        }
        bytes = bytes.checked_add(n as u64).ok_or_else(invalid)?;
        if bytes > source.source.size {
            return Err(invalid());
        }
        h.update(&buffer[..n]);
    }
    if bytes != source.source.size || identity(&source.file)? != source.source.identity {
        return Err(invalid());
    }
    source
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|_| io_error())?;
    Ok(format!("{:x}", h.finalize()))
}
fn capture(
    paths: Vec<PathBuf>,
    request_id: uuid::Uuid,
    revision: i64,
) -> Result<(Capture, Vec<Held>), ApiError> {
    if paths.len() != 5 {
        return Err(invalid());
    }
    let mut sources = vec![];
    for (path, (stream, _name)) in paths.into_iter().zip([
        (HistoricalStream::QuarantineManifest, "quarantine.json"),
        (HistoricalStream::QuarantineDatabase, "quarantine.sqlite3"),
        (HistoricalStream::Mapping, "mapping.json"),
        (HistoricalStream::Preview, "preview.json"),
        (HistoricalStream::ColdTargetSnapshot, "cold-target.sqlite3"),
    ]) {
        let maximum = if stream == HistoricalStream::Mapping {
            256 * 1024 * 1024
        } else {
            stream.maximum()
        };
        let mut source = held(path, stream, maximum)?;
        source.source.sha256 = digest(&mut source)?;
        sources.push(source);
    }
    let mut manifest = Vec::new();
    sources[0]
        .file
        .read_to_end(&mut manifest)
        .map_err(|_| io_error())?;
    sources[0]
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|_| io_error())?;
    let parsed: Value = serde_json::from_slice(&manifest).map_err(|_| invalid())?;
    let export = parsed["export_sha256"]
        .as_str()
        .filter(|s| sha(s))
        .ok_or_else(invalid)?
        .to_owned();
    #[derive(Deserialize)]
    struct PreviewFormat {
        format: String,
    }
    let preview = sources
        .iter_mut()
        .find(|s| s.source.stream == HistoricalStream::Preview)
        .ok_or_else(invalid)?;
    let preview_format: PreviewFormat =
        serde_json::from_reader(std::io::BufReader::new(&mut preview.file))
            .map_err(|_| invalid())?;
    preview
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|_| io_error())?;
    let mapping_maximum = match preview_format.format.as_str() {
        "localai-cowork-quarantine-preview/1" => 2 * 1024 * 1024,
        "localai-cowork-quarantine-preview/2" => 256 * 1024 * 1024,
        _ => return Err(invalid()),
    };
    if sources
        .iter()
        .find(|s| s.source.stream == HistoricalStream::Mapping)
        .ok_or_else(invalid)?
        .source
        .size
        > mapping_maximum
    {
        return Err(invalid());
    }
    let pin = |stream| {
        sources
            .iter()
            .find(|s| s.source.stream == stream)
            .map(|s| s.source.sha256.clone())
            .ok_or_else(invalid)
    };
    let request = BeginHistoricalImport {
        request_id,
        expected_limit_revision: revision,
        pins: HistoricalPinSet {
            quarantine_manifest: pin(HistoricalStream::QuarantineManifest)?,
            quarantine_database: pin(HistoricalStream::QuarantineDatabase)?,
            export_manifest: export,
            mapping: pin(HistoricalStream::Mapping)?,
            preview: pin(HistoricalStream::Preview)?,
            cold_target_snapshot: pin(HistoricalStream::ColdTargetSnapshot)?,
        },
        files: sources
            .iter()
            .map(|s| HistoricalFileDeclaration {
                stream: s.source.stream,
                size: s.source.size,
                sha256: s.source.sha256.clone(),
            })
            .collect(),
        preview_format: Some(preview_format.format),
    };
    let frozen = Capture {
        format: 1,
        request_sha256: cowork_domain::canonical_hash(&request),
        request,
        sources: sources.iter().map(|s| s.source.clone()).collect(),
    };
    Ok((frozen, sources))
}
async fn raw(client: &NativeClient, path: &str) -> Result<Vec<u8>, ApiError> {
    let mut response = client
        .request(reqwest::Method::GET, &route(path))
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|_| io_error())?;
    if !response.status().is_success()
        || response.content_length().is_some_and(|v| v > CHUNK as u64)
    {
        return Err(io_error());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| io_error())? {
        if chunk.len() > CHUNK - bytes.len() {
            return Err(invalid());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
#[tauri::command]
pub async fn historical_import_request(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, HistoricalImportGate>,
    path: String,
    method: String,
    input: Value,
    language: Option<UiLanguage>,
) -> Result<Value, ApiError> {
    let german = matches!(language, Some(UiLanguage::De));
    if !allowed(&path, &method, false) {
        return Err(failure("forbidden"));
    }
    let client = connected(&window).await?;
    if method == "GET" {
        return client
            .result(
                client
                    .request(reqwest::Method::GET, &route(&path))
                    .timeout(Duration::from_secs(120)),
                false,
            )
            .await;
    }
    let _gate = gate
        .0
        .try_lock()
        .map_err(|_| failure("historical_import_busy"))?;
    let data = super::private_data_dir(window.app_handle()).map_err(|_| io_error())?;
    let (folder, _pins, _lock) = super::model_registry::journal_named(&data, "historical-imports")?;
    let key = cowork_domain::canonical_hash(&(&path, &method, &input));
    let journal = folder.join(format!("{key}.request.json"));
    let digest = cowork_domain::canonical_hash(&(&path, &method, &input));
    let frozen = if journal.try_exists().map_err(|_| io_error())? {
        let original: FrozenRequest = serde_json::from_slice(
            &super::model_registry::read_selected(&journal, JOURNAL_BYTES, true)?,
        )
        .map_err(|_| invalid())?;
        if original.format != 1
            || original.path != path
            || original.method != method
            || original.input != input
            || original.sha256 != digest
        {
            return Err(invalid());
        }
        original
    } else {
        if path == "/limit" {
            let policy: ConfigureHistoricalImportLimit =
                serde_json::from_value(input.clone()).map_err(|_| invalid())?;
            let text = if german {
                format!(
                    "Import-Speicherlimit ausdrücklich auf {} Bytes setzen (Revision {})? Bereits reservierte Originaldaten bleiben erhalten.",
                    policy.limit_bytes, policy.expected_revision
                )
            } else {
                format!(
                    "Explicitly set the import storage limit to {} bytes (revision {})? Existing reserved original data is retained.",
                    policy.limit_bytes, policy.expected_revision
                )
            };
            if !confirm(&window, german, text).await? {
                return Err(failure("cancelled"));
            }
        } else if path.ends_with("/history/index") {
            let build: BuildHistoricalHistoryIndex =
                serde_json::from_value(input.clone()).map_err(|_| invalid())?;
            if build.request_id.is_nil()
                || build.expected_import_revision <= 0
                || build.expected_limit_revision <= 0
            {
                return Err(invalid());
            }
            let text = if german {
                format!(
                    "Vollständigen lesenden Historienindex ausdrücklich erstellen (Importrevision {}, Limitrevision {})? Zusätzliche verschlüsselte Kopien werden unter dem aktuellen Speicherlimit reserviert. Historische Akteure und ACLs verleihen keine aktuellen Rechte.",
                    build.expected_import_revision, build.expected_limit_revision
                )
            } else {
                format!(
                    "Explicitly build the complete read-only history index (import revision {}, limit revision {})? Additional encrypted copies are reserved under the current storage limit. Historical actors and ACLs grant no current permissions.",
                    build.expected_import_revision, build.expected_limit_revision
                )
            };
            if !confirm(&window, german, text).await? {
                return Err(failure("cancelled"));
            }
        } else if path.ends_with("/activate") {
            let activation: ActivateHistoricalImport =
                serde_json::from_value(input.clone()).map_err(|_| invalid())?;
            if !activation.explicit_activation || !sha(&activation.plan_sha256) {
                return Err(invalid());
            }
            let text = if german {
                format!(
                    "Schritt 4: Genau den Plan {} jetzt als bearbeitbare aktuelle Inhalte aktivieren? Alte Ausführungsrechte bleiben Historie.",
                    activation.plan_sha256
                )
            } else {
                format!(
                    "Step 4: Activate exactly plan {} as editable current content? Historical execution permissions remain inert history.",
                    activation.plan_sha256
                )
            };
            if !confirm(&window, german, text).await? {
                return Err(failure("cancelled"));
            }
        } else if path.ends_with("/close-unsent") {
            let closure: CloseHistoricalUnsentFile =
                serde_json::from_value(input.clone()).map_err(|_| invalid())?;
            if !sha(&closure.expected_request_sha256) {
                return Err(invalid());
            }
            let text = if german {
                format!(
                    "Die nachweislich ungesendete Dateioperation {} ausdrücklich abschließen? Nur nie geschriebene Dateibytes werden freigegeben; Originaldaten und Belege bleiben erhalten. Ein bereits zugelassener Versand wird abgewiesen.",
                    closure.expected_request_sha256
                )
            } else {
                format!(
                    "Explicitly close the proved unsent file operation {}? Only never-written file bytes are released; original data and receipts are retained. An operation already admitted for dispatch is rejected.",
                    closure.expected_request_sha256
                )
            };
            if !confirm(&window, german, text).await? {
                return Err(failure("cancelled"));
            }
        }
        let frozen = FrozenRequest {
            format: 1,
            path: path.clone(),
            method: method.clone(),
            input,
            sha256: digest,
        };
        super::model_registry::publish(&folder, &journal, &frozen, JOURNAL_BYTES)?;
        frozen
    };
    let request = client
        .request(reqwest::Method::POST, &route(&frozen.path))
        .timeout(Duration::from_secs(1800));
    client
        .result(
            if frozen.input.is_null() {
                request
            } else {
                request
                    .header("content-type", "application/json")
                    .body(serde_json::to_vec(&frozen.input).map_err(|_| invalid())?)
            },
            true,
        )
        .await
}
#[tauri::command]
pub async fn historical_import_upload_chunk(
    window: tauri::WebviewWindow,
    path: String,
    bytes: Vec<u8>,
) -> Result<HistoricalChunkReceipt, ApiError> {
    if !allowed(&path, "POST", true) || bytes.len() > CHUNK {
        return Err(failure("forbidden"));
    }
    let client = connected(&window).await?;
    client
        .result(
            client
                .request(reqwest::Method::POST, &route(&path))
                .header("content-type", "application/octet-stream")
                .body(bytes)
                .timeout(Duration::from_secs(120)),
            true,
        )
        .await
}
#[tauri::command]
pub async fn historical_import_read_page(
    window: tauri::WebviewWindow,
    path: String,
) -> Result<Vec<u8>, ApiError> {
    if path.contains("/history/")
        || path.contains("/content/catalog/")
        || path.starts_with("/previews/")
        || !allowed(&path, "GET", true)
    {
        return Err(failure("forbidden"));
    }
    raw(&connected(&window).await?, &path).await
}
/// Base64 keeps a 1 MiB original page well below the native JSON envelope cap.
#[tauri::command]
pub async fn historical_import_read_history_bytes(
    window: tauri::WebviewWindow,
    path: String,
) -> Result<String, ApiError> {
    use base64::Engine;
    if !(path.contains("/history/")
        || path.contains("/content/catalog/")
        || path.starts_with("/previews/"))
        || !allowed(&path, "GET", true)
    {
        return Err(failure("forbidden"));
    }
    let bytes = raw(&connected(&window).await?, &path).await?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}
#[tauri::command]
pub async fn historical_import_select(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, HistoricalImportGate>,
    request_id: String,
    language: Option<UiLanguage>,
) -> Result<Option<HistoricalImportRecord>, ApiError> {
    let german = matches!(language, Some(UiLanguage::De));
    if !uuid(&request_id) {
        return Err(invalid());
    }
    let _gate = gate
        .0
        .try_lock()
        .map_err(|_| failure("historical_import_busy"))?;
    let client = connected(&window).await?;
    let data = super::private_data_dir(window.app_handle()).map_err(|_| io_error())?;
    let (folder, _pins, _lock) = super::model_registry::journal_named(&data, "historical-imports")?;
    let journal = folder.join(format!("capture-{request_id}.request.json"));
    let (frozen, mut sources) = if journal.try_exists().map_err(|_| io_error())? {
        let frozen: Capture = serde_json::from_slice(&super::model_registry::read_selected(
            &journal,
            JOURNAL_BYTES,
            true,
        )?)
        .map_err(|_| invalid())?;
        if frozen.format != 1
            || frozen.request.request_id.to_string() != request_id
            || cowork_domain::canonical_hash(&frozen.request) != frozen.request_sha256
            || frozen.sources.len() != 5
        {
            return Err(invalid());
        }
        // A previously committed verification/activation is recoverable even
        // after the original offline files have been moved or removed.
        let retained: HistoricalImportRecord = client
            .result(
                client
                    .request(reqwest::Method::POST, &route("/begin"))
                    .header("content-type", "application/json")
                    .body(serde_json::to_vec(&frozen.request).map_err(|_| invalid())?)
                    .timeout(Duration::from_secs(120)),
                true,
            )
            .await?;
        if retained.request_id != frozen.request.request_id
            || retained.preview_format != frozen.request.preview_format
            || retained.request_sha256
                != cowork_domain::canonical_hash(&(
                    "historical-intake/v1",
                    &retained.owner,
                    &frozen.request,
                ))
            || !matches!(retained.owner, HistoricalOwner::PersonalNative { .. })
        {
            return Err(invalid());
        }
        if retained.state != "receiving" {
            return Ok(Some(retained));
        }
        let selected = frozen.sources.clone();
        let current_format = frozen.request.preview_format.clone();
        let sources = tauri::async_runtime::spawn_blocking(move || {
            let mut all = vec![];
            for source in selected {
                let maximum = if source.stream == HistoricalStream::Mapping
                    && current_format.as_deref() == Some("localai-cowork-quarantine-preview/2")
                {
                    256 * 1024 * 1024
                } else {
                    source.stream.maximum()
                };
                let mut current = held(source.path.clone(), source.stream, maximum)?;
                if current.source.identity != source.identity
                    || current.source.size != source.size
                    || digest(&mut current)? != source.sha256
                {
                    return Err(invalid());
                }
                current.source = source;
                all.push(current);
            }
            Ok::<_, ApiError>(all)
        })
        .await
        .map_err(|_| io_error())??;
        (frozen, sources)
    } else {
        let mut paths = vec![];
        for name in [
            "quarantine.json",
            "quarantine.sqlite3",
            "mapping.json",
            "preview.json",
            "cold-target.sqlite3",
        ] {
            let title = if german {
                format!("Original {name} auswählen")
            } else {
                format!("Select original {name}")
            };
            let Some(path) = pick(&window, false, &title).await? else {
                return Ok(None);
            };
            paths.push(path);
        }
        let limit: HistoricalImportLimit = client
            .result(
                client.request(reqwest::Method::GET, &route("/limit")),
                false,
            )
            .await?;
        let request = uuid::Uuid::parse_str(&request_id).map_err(|_| invalid())?;
        let (frozen, sources) =
            tauri::async_runtime::spawn_blocking(move || capture(paths, request, limit.revision))
                .await
                .map_err(|_| io_error())??;
        let text = if german {
            format!(
                "Genau diese fünf kalten M9-Dateien aufnehmen? Quarantäne-SHA: {}. Aktuelle Speicherlimitrevision: {}. Die ursprünglichen Dateien werden ausschließlich gelesen.",
                frozen.request.pins.quarantine_database, frozen.request.expected_limit_revision
            )
        } else {
            format!(
                "Import exactly these five cold M9 files? Quarantine SHA: {}. Current storage limit revision: {}. The original files are read only.",
                frozen.request.pins.quarantine_database, frozen.request.expected_limit_revision
            )
        };
        if !confirm(&window, german, text).await? {
            return Ok(None);
        }
        super::model_registry::publish(&folder, &journal, &frozen, JOURNAL_BYTES)?;
        (frozen, sources)
    };
    let record: HistoricalImportRecord = client
        .result(
            client
                .request(reqwest::Method::POST, &route("/begin"))
                .header("content-type", "application/json")
                .body(serde_json::to_vec(&frozen.request).map_err(|_| invalid())?)
                .timeout(Duration::from_secs(120)),
            true,
        )
        .await?;
    if record.request_id != frozen.request.request_id
        || record.preview_format != frozen.request.preview_format
        || record.request_sha256
            != cowork_domain::canonical_hash(&(
                "historical-intake/v1",
                &record.owner,
                &frozen.request,
            ))
        || !matches!(record.owner, HistoricalOwner::PersonalNative { .. })
        || record.pins.quarantine_database != frozen.request.pins.quarantine_database
    {
        return Err(invalid());
    }
    if record.state != "receiving" {
        return Ok(Some(record));
    }
    for source in &mut sources {
        source
            .file
            .seek(SeekFrom::Start(0))
            .map_err(|_| io_error())?;
        let mut offset = 0u64;
        let mut index = 0u32;
        while offset < source.source.size {
            let size = (source.source.size - offset).min(CHUNK as u64) as usize;
            let mut bytes = vec![0; size];
            source.file.read_exact(&mut bytes).map_err(|_| io_error())?;
            let digest = cowork_domain::hash(&bytes);
            let path = format!(
                "/imports/{}/chunks/{}/{}",
                record.id,
                source.source.stream.locator(),
                index
            );
            let ack: HistoricalChunkReceipt = client
                .result(
                    client
                        .request(reqwest::Method::POST, &route(&path))
                        .header("content-type", "application/octet-stream")
                        .body(bytes)
                        .timeout(Duration::from_secs(120)),
                    true,
                )
                .await?;
            if ack.import_id != record.id
                || ack.stream != source.source.stream
                || ack.chunk_index != index
                || ack.size != size as u64
                || ack.sha256 != digest
            {
                return Err(invalid());
            }
            offset += size as u64;
            index = index.checked_add(1).ok_or_else(invalid)?;
        }
    }
    let expected = frozen.sources.clone();
    tauri::async_runtime::spawn_blocking(move || {
        for (source, original) in sources.iter_mut().zip(expected) {
            if digest(source)? != original.sha256 {
                return Err(invalid());
            }
        }
        Ok::<_, ApiError>(())
    })
    .await
    .map_err(|_| io_error())??;
    let verified: HistoricalImportRecord = client
        .result(
            client
                .request(
                    reqwest::Method::POST,
                    &route(&format!("/imports/{}/verify", record.id)),
                )
                .timeout(Duration::from_secs(1800)),
            true,
        )
        .await?;
    if verified.id != record.id
        || verified.preview_format != frozen.request.preview_format
        || verified.request_id != frozen.request.request_id
        || verified.pins.quarantine_database != frozen.request.pins.quarantine_database
    {
        return Err(invalid());
    }
    Ok(Some(verified))
}
fn encoded(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            result.push(byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileCapture {
    format: u32,
    preview: uuid::Uuid,
    plan_sha256: String,
    source_id: String,
    source: Source,
}
fn check_file_operation(
    plan: &HistoricalImportPreview,
    source: &str,
    op: &HistoricalFileOperation,
    capture: Option<&FileCapture>,
) -> Result<(), ApiError> {
    let (project, index) = plan
        .projects
        .iter()
        .find_map(|p| {
            p.source
                .files
                .iter()
                .position(|f| f.source_id == source)
                .map(|i| (p, i))
        })
        .ok_or_else(invalid)?;
    let file = &project.source.files[index];
    let request = HistoricalFileStageRequest {
        source_id: source.into(),
        backend_id: op.backend_id.clone(),
        size: op.size,
        sha256: op.sha256.clone(),
    };
    if op.preview_id != plan.id
        || op.import_id != plan.import_id
        || op.plan_sha256 != plan.plan_sha256
        || op.owner != plan.owner
        || !matches!(op.owner, HistoricalOwner::PersonalNative { .. })
        || op.source_id != source
        || op.target_id != *project.file_ids.get(index).ok_or_else(invalid)?
        || op.project_id != project.project_id
        || Some(&op.order_id) != project.order_ids.last()
        || op.name != file.name
        || op.media_type != file.media_type
        || op.backend_id != "native-file-broker-v1"
        || op.size > MAX_DOWNLOAD_BYTES
        || !sha(&op.sha256)
        || file.size.is_some_and(|s| s != op.size)
        || file.sha256.as_ref().is_some_and(|s| s != &op.sha256)
        || op.request_sha256
            != cowork_domain::canonical_hash(&(
                "historical-file-stage/v1",
                &plan.owner,
                plan.id,
                &plan.plan_sha256,
                &request,
                &project.project_id,
                &project.file_ids[index],
            ))
        || capture.is_some_and(|c| {
            c.preview != op.preview_id
                || c.plan_sha256 != op.plan_sha256
                || c.source_id != op.source_id
                || c.source.sha256 != op.sha256
                || c.source.size != op.size
        })
    {
        return Err(invalid());
    }
    Ok(())
}
#[tauri::command]
pub async fn historical_import_stage_file(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, HistoricalImportGate>,
    preview: String,
    source: String,
    language: Option<UiLanguage>,
) -> Result<Option<HistoricalFileOperation>, ApiError> {
    let german = matches!(language, Some(UiLanguage::De));
    if !uuid(&preview)
        || source.is_empty()
        || source.len() > 300
        || source.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    let _gate = gate
        .0
        .try_lock()
        .map_err(|_| failure("historical_import_busy"))?;
    let client = connected(&window).await?;
    let plan: HistoricalImportPreview = client
        .result(
            client
                .request(
                    reqwest::Method::GET,
                    &route(&format!("/previews/{preview}")),
                )
                .timeout(Duration::from_secs(120)),
            false,
        )
        .await?;
    let file = plan
        .projects
        .iter()
        .filter(|p| !matches!(p.target, HistoricalProjectTarget::HistoryOnly { .. }))
        .flat_map(|p| p.source.files.iter())
        .find(|f| f.source_id == source)
        .ok_or_else(invalid)?;
    if plan.id.to_string() != preview
        || !matches!(plan.state.as_str(), "preview" | "active")
        || plan.history_only_file_ids.contains(&source)
    {
        return Err(invalid());
    }
    let data = super::private_data_dir(window.app_handle()).map_err(|_| io_error())?;
    let (folder, _pins, _lock) = super::model_registry::journal_named(&data, "historical-imports")?;
    let key = cowork_domain::canonical_hash(&(&preview, &source));
    let journal = folder.join(format!("file-{key}.request.json"));
    let captured =
        if journal.try_exists().map_err(|_| io_error())? {
            let original: FileCapture = serde_json::from_slice(
                &super::model_registry::read_selected(&journal, JOURNAL_BYTES, true)?,
            )
            .map_err(|_| invalid())?;
            if original.format != 1
                || original.preview != plan.id
                || original.plan_sha256 != plan.plan_sha256
                || original.source_id != source
                || !sha(&original.source.sha256)
            {
                return Err(invalid());
            }
            Some(original)
        } else {
            None
        };
    // Inspect the actual durable original before reopening any source. A lost
    // ACK is recoverable after source removal, with no second physical write.
    let path = format!("/previews/{preview}/files/{}/stage", encoded(&source));
    let retained: Option<HistoricalFileOperation> = match client
        .result::<HistoricalFileOperation>(
            client
                .request(reqwest::Method::GET, &route(&path))
                .timeout(Duration::from_secs(120)),
            false,
        )
        .await
    {
        Ok(op) => Some(op),
        Err(e) if e.code == "not_found" => None,
        Err(e) => return Err(e),
    };
    if let Some(op) = &retained {
        check_file_operation(&plan, &source, op, captured.as_ref())?;
        if op.state == "received" {
            return Ok(Some(op.clone()));
        }
        if op.state == "unsent_closed" {
            return Err(failure("historical_import_unsent_closed"));
        }
        if !matches!(op.state.as_str(), "prepared" | "uncertain") {
            return Err(invalid());
        }
    }
    if retained.is_none() && plan.state != "preview" {
        return Err(invalid());
    }
    let (frozen, mut held) = if let Some(original) = captured {
        let selected = original.source.clone();
        let current = tauri::async_runtime::spawn_blocking(move || {
            let mut current = held(
                selected.path.clone(),
                HistoricalStream::QuarantineManifest,
                MAX_DOWNLOAD_BYTES,
            )?;
            if current.source.identity != selected.identity
                || current.source.size != selected.size
                || digest(&mut current)? != selected.sha256
            {
                return Err(invalid());
            }
            current.source = selected;
            Ok::<_, ApiError>(current)
        })
        .await
        .map_err(|_| io_error())??;
        (original, current)
    } else {
        if plan.state != "preview" {
            return Err(invalid());
        }
        let title = if german {
            format!("Originaldatei ausdrücklich auswählen: {}", file.name)
        } else {
            format!("Explicitly select original file: {}", file.name)
        };
        let Some(path) = pick(&window, false, &title).await? else {
            return Ok(None);
        };
        let current = tauri::async_runtime::spawn_blocking(move || {
            let mut current = held(
                path,
                HistoricalStream::QuarantineManifest,
                MAX_DOWNLOAD_BYTES,
            )?;
            current.source.sha256 = digest(&mut current)?;
            Ok::<_, ApiError>(current)
        })
        .await
        .map_err(|_| io_error())??;
        if file.size.is_some_and(|v| v != current.source.size)
            || file
                .sha256
                .as_ref()
                .is_some_and(|v| v != &current.source.sha256)
        {
            return Err(invalid());
        }
        if retained
            .as_ref()
            .is_some_and(|op| op.size != current.source.size || op.sha256 != current.source.sha256)
        {
            return Err(invalid());
        }
        let text = if german {
            format!(
                "{} ausdrücklich als aktuelle Datei zum Plan {} übernehmen? {} Bytes, SHA {}. Eine ungewisse ursprüngliche Operation wird nur abgeglichen.",
                file.name, plan.plan_sha256, current.source.size, current.source.sha256
            )
        } else {
            format!(
                "Explicitly import {} as a current file for plan {}? {} bytes, SHA {}. An uncertain original operation is reconciled without another dispatch.",
                file.name, plan.plan_sha256, current.source.size, current.source.sha256
            )
        };
        if !confirm(&window, german, text).await? {
            return Ok(None);
        }
        let frozen = FileCapture {
            format: 1,
            preview: plan.id,
            plan_sha256: plan.plan_sha256.clone(),
            source_id: source.clone(),
            source: current.source.clone(),
        };
        super::model_registry::publish(&folder, &journal, &frozen, JOURNAL_BYTES)?;
        (frozen, current)
    };
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        let expected = held.source.sha256.clone();
        if digest(&mut held)? != expected {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        held.file.read_to_end(&mut bytes).map_err(|_| io_error())?;
        if bytes.len() as u64 != held.source.size || cowork_domain::hash(&bytes) != expected {
            return Err(invalid());
        }
        Ok::<_, ApiError>(bytes)
    })
    .await
    .map_err(|_| io_error())??;
    let operation: HistoricalFileOperation = client
        .result(
            client
                .request(reqwest::Method::POST, &route(&path))
                .header("content-type", "application/octet-stream")
                .body(bytes)
                .timeout(Duration::from_secs(120)),
            true,
        )
        .await?;
    check_file_operation(&plan, &source, &operation, Some(&frozen))?;
    if operation.preview_id != frozen.preview
        || operation.plan_sha256 != frozen.plan_sha256
        || operation.source_id != frozen.source_id
        || operation.sha256 != frozen.source.sha256
        || operation.size != frozen.source.size
        || operation.state != "received"
    {
        return Err(invalid());
    }
    Ok(Some(operation))
}
