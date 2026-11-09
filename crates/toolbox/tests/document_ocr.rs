//! Actual native engine calls. No model account, Runtime admission or UI claim.
use cowork_toolbox::{Config, DocumentInputFile, DocumentJob, Toolbox};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

const SOURCE: &[u8] = include_bytes!("../../../tests/fixtures/ocr-invoice-scan.pdf");
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Fixture {
    toolbox: Arc<Toolbox>,
    root: PathBuf,
    base: PathBuf,
}
impl Fixture {
    fn new(python_variable: &str) -> Self {
        let base = PathBuf::from(
            std::env::var_os("COWORK_TEST_OCR_TEMP")
                .expect("Explicit owned test directory required"),
        );
        assert!(base.is_absolute() && base.is_dir());
        let base = base.canonicalize().unwrap();
        let root = base.join(format!("ocr-toolbox-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let models = PathBuf::from(
            std::env::var_os("COWORK_TEST_OCR_MODELS").expect("Exact language pack required"),
        );
        let runtime = root.join("runtime");
        fs::create_dir_all(runtime.join("ocr/tessdata")).unwrap();
        for language in ["deu", "eng"] {
            fs::copy(
                models.join(format!("{language}.traineddata")),
                runtime.join(format!("ocr/tessdata/{language}.traineddata")),
            )
            .unwrap();
        }
        let python = PathBuf::from(
            std::env::var_os(python_variable).expect("Explicit actual document Python required"),
        );
        let environment = ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok().map(|value| (name.into(), value)))
            .collect();
        let toolbox = Toolbox::open(Config {
            root: root.join("toolbox"),
            programs: BTreeMap::from([("python".into(), python)]),
            environment,
            github_token: None,
        })
        .unwrap();
        toolbox.configure_document_runtime(runtime).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !toolbox.availability().documents_available {
            assert!(
                Instant::now() < deadline,
                "Actual base document runtime was not ready"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        Self {
            toolbox,
            root,
            base,
        }
    }
    fn job(&self) -> DocumentJob {
        DocumentJob {
            engine_revision: Some(self.toolbox.document_engine_revision()),
            tool: "document_read".into(),
            operation: "ocr".into(),
            inputs: vec![DocumentInputFile {
                folder_id: "copy-files".into(),
                path: "scan.pdf".into(),
                sha256: hash(SOURCE),
                bytes: SOURCE.to_vec(),
            }],
            output_name: None,
            output_directory: false,
            spec: json!({}),
            options: json!({"languages":["deu","eng"],"dpi":200}),
            timeout: Duration::from_secs(30),
        }
    }
    fn assert_stages_closed(&self) {
        assert_eq!(
            fs::read_dir(self.root.join("toolbox/document-stages"))
                .unwrap()
                .count(),
            0
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.toolbox.shutdown();
        let actual = self.root.canonicalize().unwrap();
        assert_eq!(actual.parent(), Some(self.base.as_path()));
        assert!(
            actual
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ocr-toolbox-")
        );
        fs::remove_dir_all(actual).unwrap();
    }
}

#[test]
#[ignore = "Requires the pinned optional OCR runtime and its exact language pack"]
fn actual_ocr_reads_a_raster_pdf_and_retains_bounded_page_locations_without_outputs() {
    let fixture = Fixture::new("COWORK_TEST_DOCUMENT_PYTHON");
    assert!(fixture.toolbox.availability().ocr_available);
    let outcome = fixture
        .toolbox
        .execute_document(fixture.job(), Arc::new(AtomicBool::new(false)))
        .unwrap();
    assert!(outcome.ok, "{}", outcome.receipt);
    assert_eq!(outcome.receipt["result"]["source_sha256"], hash(SOURCE));
    assert!(
        outcome.receipt["result"]["text"]
            .as_str()
            .unwrap()
            .contains("119,00")
    );
    assert_eq!(outcome.receipt["result"]["pages"][0]["page"], 1);
    assert!(outcome.outputs.is_empty());
    assert_eq!(outcome.previews.len(), 1);
    assert_eq!(outcome.previews[0].sha256, hash(&outcome.previews[0].bytes));
    assert_eq!(
        outcome.receipt["result"]["pages"][0]["preview_sha256"],
        outcome.previews[0].sha256
    );
    fixture.assert_stages_closed();
}

#[test]
#[ignore = "Requires the pinned optional OCR runtime and its exact language pack"]
fn changed_language_data_invalidates_the_original_review_and_never_silently_falls_back() {
    let fixture = Fixture::new("COWORK_TEST_DOCUMENT_PYTHON");
    let original = fixture.job();
    let path = fixture.root.join("runtime/ocr/tessdata/deu.traineddata");
    let mut changed = fs::read(&path).unwrap();
    changed[0] ^= 1;
    fs::write(&path, changed).unwrap();
    assert_ne!(
        Some(fixture.toolbox.document_engine_revision()),
        original.engine_revision
    );
    assert!(
        fixture
            .toolbox
            .execute_document(original, Arc::new(AtomicBool::new(false)))
            .is_err()
    );
    assert!(
        fixture
            .toolbox
            .execute_document(fixture.job(), Arc::new(AtomicBool::new(false)))
            .is_err()
    );
    fixture.assert_stages_closed();
}

#[test]
#[ignore = "Requires the pinned optional OCR runtime and its exact language pack"]
fn changed_input_and_expired_deadline_leave_no_published_file_or_stage() {
    let fixture = Fixture::new("COWORK_TEST_DOCUMENT_PYTHON");
    let mut changed = fixture.job();
    changed.inputs[0].sha256 = "0".repeat(64);
    assert!(
        fixture
            .toolbox
            .execute_document(changed, Arc::new(AtomicBool::new(false)))
            .is_err()
    );
    let mut expired = fixture.job();
    expired.timeout = Duration::ZERO;
    assert!(
        fixture
            .toolbox
            .execute_document(expired, Arc::new(AtomicBool::new(false)))
            .is_err()
    );
    fixture.assert_stages_closed();
}

#[test]
#[ignore = "Requires a second real document Python without the optional OCR package"]
fn missing_optional_binding_is_reported_instead_of_fabricated_text() {
    let fixture = Fixture::new("COWORK_TEST_DOCUMENT_PYTHON_WITHOUT_OCR");
    assert!(!fixture.toolbox.availability().ocr_available);
    let error = fixture
        .toolbox
        .execute_document(fixture.job(), Arc::new(AtomicBool::new(false)))
        .unwrap_err();
    assert!(error.to_string().contains("OCR is unavailable"));
    fixture.assert_stages_closed();
}

#[test]
#[ignore = "Requires a separate disposable OCR Python copy; never mutate an installed runtime"]
fn a_changed_native_library_invalidates_the_original_review_before_recognition() {
    let fixture = Fixture::new("COWORK_TEST_OCR_MUTATION_PYTHON");
    assert!(fixture.toolbox.availability().ocr_available);
    let library = PathBuf::from(
        std::env::var_os("COWORK_TEST_OCR_MUTATION_LIBRARY")
            .expect("Disposable native library required"),
    );
    let python = PathBuf::from(std::env::var_os("COWORK_TEST_OCR_MUTATION_PYTHON").unwrap());
    let prefix = python.parent().unwrap().parent().unwrap();
    assert!(
        library
            .canonicalize()
            .unwrap()
            .starts_with(prefix.canonicalize().unwrap())
    );
    assert!(
        prefix
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("mutation-runtime")
    );
    let original = fs::read(&library).unwrap();
    assert!(original.len() < 64 * 1024 * 1024);
    let reviewed = fixture.job();
    let mut changed = original.clone();
    changed.push(0);
    fs::write(&library, &changed).unwrap();
    assert_ne!(
        Some(fixture.toolbox.document_engine_revision()),
        reviewed.engine_revision
    );
    assert!(!fixture.toolbox.availability().ocr_available);
    assert!(
        fixture
            .toolbox
            .execute_document(reviewed, Arc::new(AtomicBool::new(false)))
            .is_err()
    );
    fs::write(&library, &original).unwrap();
    assert_eq!(hash(&fs::read(&library).unwrap()), hash(&original));
    fixture.assert_stages_closed();
}

#[test]
#[ignore = "Requires a separate disposable OCR Python copy; never mutate an installed runtime"]
fn a_corrupt_native_header_is_rejected_by_hash_before_the_extension_loader_runs() {
    let fixture = Fixture::new("COWORK_TEST_OCR_MUTATION_PYTHON");
    let library = PathBuf::from(std::env::var_os("COWORK_TEST_OCR_MUTATION_LIBRARY").unwrap());
    let python = PathBuf::from(std::env::var_os("COWORK_TEST_OCR_MUTATION_PYTHON").unwrap());
    assert!(library.starts_with(python.parent().unwrap().parent().unwrap()));
    struct Restore {
        path: PathBuf,
        bytes: Vec<u8>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            fs::write(&self.path, &self.bytes).unwrap();
        }
    }
    let restore = Restore {
        path: library.clone(),
        bytes: fs::read(&library).unwrap(),
    };
    let mut changed = restore.bytes.clone();
    changed[0] ^= 255;
    fs::write(&library, changed).unwrap();
    let probe = fs::read_dir(fixture.root.join("toolbox/adapters"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("ocr_probe.py"))
        .find(|path| path.is_file())
        .expect("provisioned original probe");
    let mut command = std::process::Command::new(python);
    command.args(["-I", "-X", "utf8"]).arg(probe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        receipt["reason"], "native_library_record_changed",
        "{receipt}"
    );
    drop(restore);
    fixture.assert_stages_closed();
}
