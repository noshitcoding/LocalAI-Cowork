//! Fixed OCR language data for the existing private document stage.
use super::*;
use serde_json::{Value, json};
use std::time::SystemTime;

pub(super) const LOCK: &[u8] = include_bytes!("../../../tools/toolbox/ocr-languages.lock.json");

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeIdentity {
    binding_version: String,
    engine_version: String,
    files: Vec<NativeFile>,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeFile {
    path: PathBuf,
    bytes: u64,
    sha256: String,
    #[serde(skip)]
    modified: Option<SystemTime>,
}

pub(super) struct NativeLease {
    identity: NativeIdentity,
    _files: Vec<fs::File>,
}

fn native_file(file: &NativeFile) -> Result<fs::File> {
    if !file.path.is_absolute()
        || file.bytes > 64 * 1024 * 1024
        || file.sha256.len() != 64
        || !file
            .sha256
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(invalid("The selected OCR native identity is invalid"));
    }
    namespace(&file.path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1); // Retain read access while denying update/delete during OCR.
    }
    let mut opened = options.open(&file.path)?;
    single_file(&opened)?;
    if opened.metadata()?.len() != file.bytes {
        return Err(invalid("The selected OCR native library changed"));
    }
    let mut bytes = Vec::new();
    (&mut opened).take(file.bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != file.bytes || hash(&bytes) != file.sha256 {
        return Err(invalid("The selected OCR native library changed"));
    }
    Ok(opened)
}

impl NativeIdentity {
    pub(super) fn lease(&self) -> Result<NativeLease> {
        if self.binding_version != "2.12.0"
            || self.engine_version != "tesseract 5.5.3"
            || self.files.is_empty()
            || self.files.len() > 64
            || self
                .files
                .iter()
                .try_fold(0u64, |total, file| total.checked_add(file.bytes))
                .is_none_or(|total| total > 192 * 1024 * 1024)
        {
            return Err(invalid("The selected OCR native identity is invalid"));
        }
        let files = self
            .files
            .iter()
            .map(native_file)
            .collect::<Result<Vec<_>>>()?;
        Ok(NativeLease {
            identity: self.clone(),
            _files: files,
        })
    }
    pub(super) fn unchanged_metadata(&self) -> bool {
        self.files.iter().all(|file| {
            fs::metadata(&file.path).is_ok_and(|metadata| {
                metadata.is_file()
                    && metadata.len() == file.bytes
                    && metadata.modified().ok() == file.modified
            })
        })
    }
    pub(super) fn revision(&self) -> Value {
        let files: Vec<Value> = self
            .files
            .iter()
            .map(|file| {
                let actual = native_file(file)
                    .map(|_| file.sha256.clone())
                    .unwrap_or_else(|_| "changed".into());
                json!([file.path, actual])
            })
            .collect();
        json!([
            "ocr-native/1",
            self.binding_version,
            self.engine_version,
            files
        ])
    }
}

impl NativeLease {
    pub(super) fn assert_current(&self) -> Result<()> {
        for file in &self.identity.files {
            drop(native_file(file)?);
        }
        Ok(())
    }
}

pub(super) fn probe(
    python: &Path,
    adapter: &Path,
    environment: &BTreeMap<String, String>,
    home: &Path,
) -> Option<NativeIdentity> {
    let mut environment = environment.clone();
    for name in ["HOME", "USERPROFILE"] {
        environment.insert(name.into(), home.to_string_lossy().into());
    }
    let launch = cowork_executor::TrustedLaunch {
        program: python.into(),
        arguments: vec![
            "-I".into(),
            "-X".into(),
            "utf8".into(),
            adapter.to_string_lossy().into(),
        ],
        directory: home.into(),
        environment,
        timeout: Duration::from_secs(30),
        capture_bytes_per_stream: 64 * 1024,
    };
    let outcome = cowork_executor::supervise(&launch, &AtomicBool::new(false)).ok()?;
    let value: Value = serde_json::from_slice(&outcome.stdout.bytes).ok()?;
    if outcome.exit_code != Some(0)
        || outcome.stdout.truncated
        || value["format"] != "cowork-document-ocr-probe/1"
        || value["ok"] != true
    {
        return None;
    }
    let mut identity: NativeIdentity = serde_json::from_value(value["native"].clone()).ok()?;
    let _lease = identity.lease().ok()?;
    for file in &mut identity.files {
        file.modified = fs::metadata(&file.path).ok()?.modified().ok();
    }
    Some(identity)
}

pub(super) fn models_ready(runtime: Option<&Path>) -> bool {
    runtime.is_some_and(|runtime| {
        model_rows().is_ok_and(|rows| rows.iter().all(|row| read_model(runtime, row).is_ok()))
    })
}

fn model_rows() -> Result<Vec<Value>> {
    let lock: Value = serde_json::from_slice(LOCK)?;
    if lock["format"] != "cowork-ocr-language-pack/1" || lock["engine"] != "tesseract-lstm" {
        return Err(invalid("The pinned OCR language manifest is invalid"));
    }
    let rows = lock["models"]
        .as_array()
        .filter(|rows| rows.len() == 2)
        .ok_or_else(|| invalid("The pinned OCR language manifest is invalid"))?;
    let mut seen = std::collections::BTreeSet::new();
    for row in rows {
        let language = row["language"]
            .as_str()
            .ok_or_else(|| invalid("Invalid OCR language"))?;
        if !matches!(language, "deu" | "eng")
            || !seen.insert(language)
            || row["path"].as_str() != Some(format!("{language}.traineddata").as_str())
            || !row["sha256"].as_str().is_some_and(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
            || !row["bytes"]
                .as_u64()
                .is_some_and(|n| n > 0 && n <= 8 * 1024 * 1024)
        {
            return Err(invalid("The pinned OCR language manifest is invalid"));
        }
    }
    Ok(rows.clone())
}

fn read_model(runtime: &Path, row: &Value) -> Result<Vec<u8>> {
    let path = runtime
        .join("ocr/tessdata")
        .join(row["path"].as_str().unwrap_or_default());
    namespace(&path)?;
    let file = OpenOptions::new().read(true).open(&path)?;
    single_file(&file)?;
    let expected = row["bytes"].as_u64().unwrap_or(0);
    if file.metadata()?.len() != expected {
        return Err(invalid(
            "The OCR language data changed; repair the private OCR runtime",
        ));
    }
    let mut bytes = Vec::new();
    file.take(expected + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != expected || row["sha256"].as_str() != Some(hash(&bytes).as_str()) {
        return Err(invalid(
            "The OCR language data changed; repair the private OCR runtime",
        ));
    }
    Ok(bytes)
}

/// Identity changes even when setup changes bytes without changing a path.
pub(super) fn identity(runtime: Option<&Path>) -> Value {
    let rows = match model_rows() {
        Ok(rows) => rows,
        Err(_) => return json!(["ocr-language-pack/1", hash(LOCK), "invalid_manifest"]),
    };
    let Some(runtime) = runtime else {
        return json!(["ocr-language-pack/1", hash(LOCK), "unconfigured"]);
    };
    let values: Vec<Value> = rows
        .iter()
        .map(|row| {
            let path = runtime
                .join("ocr/tessdata")
                .join(row["path"].as_str().unwrap_or_default());
            let digest = read_model(runtime, row)
                .map(|bytes| hash(&bytes))
                .unwrap_or_else(|_| "unavailable".into());
            json!([path, digest])
        })
        .collect();
    json!(["ocr-language-pack/1", hash(LOCK), values])
}

/// Only verified bytes are copied; the engine never receives the runtime's host path.
pub(super) fn stage(runtime: Option<&Path>, root: &Path) -> Result<()> {
    let runtime = runtime
        .ok_or_else(|| invalid("OCR is not installed yet; set up the private OCR runtime"))?;
    let rows = model_rows()?;
    let directory = root.join("in/ocr-models");
    private_dir(&directory)?;
    for row in rows {
        let bytes = read_model(runtime, &row).map_err(|_| {
            invalid("OCR language data is unavailable or changed; repair the private OCR runtime")
        })?;
        let name = row["path"]
            .as_str()
            .ok_or_else(|| invalid("Invalid OCR language filename"))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(name))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        single_file(&file)?;
    }
    Ok(())
}
