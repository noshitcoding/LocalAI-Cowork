//! Opt-in real engines, isolated long original data directory, no model or Runtime admission.
use super::*;
use std::{os::windows::ffi::OsStrExt, time::Instant};

const SCAN: &[u8] = include_bytes!("../../../../../tests/fixtures/ocr-invoice-scan.pdf");

struct Scratch {
    path: PathBuf,
    canonical: PathBuf,
    parent: PathBuf,
}
impl Scratch {
    fn new() -> Self {
        let parent = PathBuf::from(
            std::env::var_os("COWORK_LONG_ROOT_TEMP")
                .expect("Explicit own temporary parent required"),
        )
        .canonicalize()
        .unwrap();
        assert!(parent.is_absolute() && parent.is_dir());
        let path = parent.join(format!("lr-{}", Uuid::new_v4().simple()));
        fs::create_dir(&path).unwrap();
        let canonical = path.canonicalize().unwrap();
        Self {
            path,
            canonical,
            parent,
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if self.path.is_absolute()
            && self.path.canonicalize().ok().as_ref() == Some(&self.canonical)
            && self.canonical.parent() == Some(self.parent.as_path())
            && self
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("lr-")
        {
            let _ = fs::remove_dir_all(&self.canonical);
        }
    }
}
fn input(name: &str, bytes: &[u8]) -> DocumentInputFile {
    DocumentInputFile {
        folder_id: "own-long-root-fixture".into(),
        path: name.into(),
        sha256: hash(bytes),
        bytes: bytes.to_vec(),
    }
}
fn job(
    toolbox: &Toolbox,
    action: (&str, &str),
    inputs: Vec<DocumentInputFile>,
    output: Option<&str>,
    spec: Value,
    options: Value,
    timeout: u64,
) -> DocumentJob {
    DocumentJob {
        engine_revision: Some(toolbox.document_engine_revision()),
        tool: action.0.into(),
        operation: action.1.into(),
        inputs,
        output_name: output.map(str::to_owned),
        output_directory: false,
        spec,
        options,
        timeout: Duration::from_secs(timeout),
    }
}
fn run(toolbox: &Toolbox, name: &str, job: DocumentJob) -> DocumentOutcome {
    let started = Instant::now();
    let result = toolbox
        .execute_document(job, Arc::new(AtomicBool::new(false)))
        .unwrap();
    assert!(result.ok, "{name}: {}", result.receipt);
    assert_eq!(
        fs::read_dir(
            toolbox
                .documents
                .root
                .path()
                .unwrap()
                .join("document-stages")
        )
        .unwrap()
        .count(),
        0
    );
    for output in &result.outputs {
        assert_eq!(output.sha256, hash(&output.bytes));
    }
    for preview in &result.previews {
        assert_eq!(preview.sha256, hash(&preview.bytes));
    }
    let evidence = PathBuf::from(
        std::env::var_os("COWORK_LONG_ROOT_EVIDENCE").expect("Owned evidence directory required"),
    );
    assert!(evidence.is_absolute() && evidence.is_dir());
    let case = evidence.join(name);
    fs::create_dir(&case).unwrap();
    fs::write(
        case.join("receipt.json"),
        serde_json::to_vec_pretty(&result.receipt).unwrap(),
    )
    .unwrap();
    for (index, file) in result.outputs.iter().enumerate() {
        fs::write(case.join(format!("output-{index}.bin")), &file.bytes).unwrap();
    }
    for (index, preview) in result.previews.iter().enumerate() {
        fs::write(case.join(format!("preview-{index}.png")), &preview.bytes).unwrap();
    }
    println!(
        "{}",
        json!({"case":name,"ok":true,"elapsed_ms":started.elapsed().as_millis(),"receipt_sha256":result.receipt_sha256,
        "outputs":result.outputs.iter().map(|file|json!({"path":file.path,"sha256":file.sha256,"bytes":file.bytes.len()})).collect::<Vec<_>>(),
        "previews":result.previews.iter().map(|preview|preview.sha256.clone()).collect::<Vec<_>>() })
    );
    result
}

#[test]
#[ignore = "Requires the explicit pinned Python/OCR, LibreOffice and own copied warm Tectonic runtime"]
fn actual_document_operations_under_one_long_root() {
    let scratch = Scratch::new();
    let mut original = scratch.path.clone();
    for index in 0..3 {
        original = original.join(format!(
            "Dokumentarbeitsverzeichnis-{index}-Prüfung-📄-{}",
            "x".repeat(72)
        ));
    }
    original = original.join("toolbox");
    let original_units = original.as_os_str().encode_wide().count();
    assert!(original_units > 320);
    let python = PathBuf::from(
        std::env::var_os("COWORK_LONG_ROOT_PYTHON").expect("Real document Python required"),
    );
    let office = PathBuf::from(
        std::env::var_os("COWORK_LONG_ROOT_OFFICE").expect("Real LibreOffice required"),
    );
    let tectonic = PathBuf::from(
        std::env::var_os("COWORK_LONG_ROOT_TECTONIC").expect("Own warm Tectonic copy required"),
    );
    let runtime = PathBuf::from(
        std::env::var_os("COWORK_LONG_ROOT_RUNTIME").expect("Own copied runtime required"),
    );
    let environment = ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
        .collect();
    let mut toolbox = Toolbox::open(Config {
        root: original.clone(),
        programs: BTreeMap::from([
            ("python".into(), python),
            ("libreoffice".into(), office),
            ("tectonic".into(), tectonic),
        ]),
        environment,
        github_token: None,
    })
    .unwrap();
    let workspace = scratch.path.join("work");
    fs::create_dir(&workspace).unwrap();
    Arc::get_mut(&mut toolbox)
        .unwrap()
        .configure_document_workspace(
            workspace.clone(),
            Arc::new(directory(&scratch.path).unwrap()),
        )
        .unwrap();
    toolbox.configure_document_runtime(runtime).unwrap();
    let launch = toolbox.documents.root.path().unwrap().to_path_buf();
    let launch_units = launch.as_os_str().encode_wide().count();
    assert!(
        launch_units < original_units
            && document_working_directory_fits(&launch)
            && office_profile_path_fits(&launch)
    );
    assert_eq!(
        identity(&directory(&launch).unwrap()).unwrap(),
        identity(&directory(&workspace).unwrap()).unwrap()
    );
    assert_eq!(
        toolbox.documents.root.source.as_ref().unwrap().identity,
        identity(&directory(&original).unwrap()).unwrap()
    );
    println!(
        "{}",
        json!({"original_utf16_units":original_units,"launch_utf16_units":launch_units,"original_data_directory_binding_retained":true,"physically_short_private_workspace":true})
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    let available = loop {
        let state = toolbox.availability();
        if state.document_status != "probing" {
            break state;
        }
        assert!(
            Instant::now() < deadline,
            "long-root readiness did not finish"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        available.documents_available
            && available.ocr_available
            && available.office_convert_available
            && available.latex_available,
        "document={} ocr={} office={} latex={}",
        available.document_status,
        available.ocr_status,
        available.office_convert_status,
        available.latex_status
    );
    let source = b"Original UTF-8: Gr\xc3\xbc\xc3\x9fe aus dem langen Datenpfad.";
    let original_source = scratch.path.join("original.txt");
    fs::write(&original_source, source).unwrap();
    let read = run(
        &toolbox,
        "text",
        job(
            &toolbox,
            ("document_read", "text"),
            vec![input("original.txt", source)],
            None,
            json!({}),
            json!({}),
            30,
        ),
    );
    assert!(
        read.receipt["result"]["text"]
            .as_str()
            .unwrap()
            .contains("Grüße")
    );
    let docx = run(
        &toolbox,
        "docx",
        job(
            &toolbox,
            ("document_write", "create"),
            vec![],
            Some("Bericht.docx"),
            json!({"format":"docx","blocks":[{"type":"paragraph","text":"Grüße vom langen Originalordner."}]}),
            json!({}),
            30,
        ),
    );
    assert_eq!(docx.outputs.len(), 1);
    assert!(docx.outputs[0].bytes.starts_with(b"PK"));
    let pdf = run(
        &toolbox,
        "office_convert",
        job(
            &toolbox,
            ("document_write", "convert"),
            vec![input("Bericht.docx", &docx.outputs[0].bytes)],
            Some("Bericht.pdf"),
            json!({}),
            json!({}),
            90,
        ),
    );
    assert_eq!(pdf.outputs.len(), 1);
    assert!(pdf.outputs[0].bytes.starts_with(b"%PDF-"));
    let pdf_text = run(
        &toolbox,
        "pdf_text",
        job(
            &toolbox,
            ("document_read", "text"),
            vec![input("Bericht.pdf", &pdf.outputs[0].bytes)],
            None,
            json!({}),
            json!({}),
            30,
        ),
    );
    assert!(
        pdf_text.receipt["result"]["text"]
            .as_str()
            .unwrap()
            .contains("Grüße")
    );
    let ocr = run(
        &toolbox,
        "native_ocr",
        job(
            &toolbox,
            ("document_read", "ocr"),
            vec![input("scan.pdf", SCAN)],
            None,
            json!({}),
            json!({"languages":["deu","eng"],"dpi":200}),
            30,
        ),
    );
    assert_eq!(ocr.receipt["result"]["source_sha256"], hash(SCAN));
    assert!(
        ocr.receipt["result"]["text"]
            .as_str()
            .unwrap()
            .contains("119,00")
    );
    assert_eq!(ocr.previews.len(), 1);
    let image = run(
        &toolbox,
        "image",
        job(
            &toolbox,
            ("image_edit", "pipeline"),
            vec![input("preview.png", &ocr.previews[0].bytes)],
            Some("thumbnail.png"),
            json!({"steps":[{"op":"thumbnail","max_edge":64}]}),
            json!({}),
            30,
        ),
    );
    assert_eq!(image.outputs.len(), 1);
    assert!(image.outputs[0].bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    let tex=b"\\documentclass{article}\n\\begin{document}\nLong root native qualification.\n\\end{document}\n";
    let latex = run(
        &toolbox,
        "confined_latex",
        job(
            &toolbox,
            ("document_write", "compile_latex"),
            vec![input("main.tex", tex)],
            Some("LaTeX.pdf"),
            json!({}),
            json!({}),
            120,
        ),
    );
    assert_eq!(latex.outputs.len(), 1);
    assert!(latex.outputs[0].bytes.starts_with(b"%PDF-"));
    assert_eq!(fs::read(&original_source).unwrap(), source);
    assert_eq!(
        fs::read_dir(original.join("document-stages"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(fs::read_dir(original.join(".lo")).unwrap().count(), 0);
    assert_eq!(fs::read_dir(workspace.join(".lo")).unwrap().count(), 0);
    toolbox.shutdown();
    drop(toolbox);
    println!(
        "{}",
        json!({"all_seven_operations_passed":true,"original_source_unchanged":true,"stages_and_profiles_empty":true,"model_calls":0})
    );
}

#[test]
#[ignore = "Explicit actual Office diagnostic in own short and long fixture directories"]
fn actual_office_probe_diagnostic_comparing_short_spelling() {
    let scratch = Scratch::new();
    let program = PathBuf::from(
        std::env::var_os("COWORK_LONG_ROOT_OFFICE").expect("Real LibreOffice required"),
    );
    let mut long = scratch.path.clone();
    for index in 0..3 {
        long = long.join(format!(
            "Dokumentarbeitsverzeichnis-{index}-Prüfung-📄-{}",
            "x".repeat(72)
        ));
    }
    let environment: BTreeMap<String, String> = ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
        .collect();
    let mut observations = Vec::new();
    for (kind, original) in [
        ("baseline", scratch.path.join("baseline")),
        ("long", long.join("toolbox")),
    ] {
        let owner = Toolbox::open(Config {
            root: original.clone(),
            programs: BTreeMap::new(),
            environment: BTreeMap::new(),
            github_token: None,
        })
        .unwrap();
        let root = owner.documents.root.path().unwrap();
        let stage = Stage(root.join(format!("office-probe-{}", Uuid::new_v4())));
        private_dir(&stage.0).unwrap();
        write_new(&stage.0.join("probe.txt"), b"Cowork conversion probe").unwrap();
        let profile = office_profile(root).unwrap();
        let uri = file_uri(&profile.0);
        let launch = cowork_executor::TrustedLaunch {
            program: program.clone(),
            arguments: vec![
                "--headless".into(),
                "--norestore".into(),
                "--nodefault".into(),
                "--nologo".into(),
                format!("-env:UserInstallation={uri}"),
                "--convert-to".into(),
                "pdf".into(),
                "--outdir".into(),
                stage.0.to_string_lossy().into(),
                stage.0.join("probe.txt").to_string_lossy().into(),
            ],
            directory: stage.0.clone(),
            environment: environment.clone(),
            timeout: Duration::from_secs(90),
            capture_bytes_per_stream: 16 * 1024,
        };
        let result = cowork_executor::supervise(&launch, &AtomicBool::new(false));
        let observation = match result {
            Ok(outcome) => {
                json!({"kind":kind,"original_utf16_units":original.as_os_str().encode_wide().count(),"launch_utf16_units":root.as_os_str().encode_wide().count(),
                "profile_utf16_units":profile.0.as_os_str().encode_wide().count(),"uri_utf16_units":uri.encode_utf16().count(),"exit_code":outcome.exit_code,"termination":outcome.termination,
                "stdout":String::from_utf8_lossy(&outcome.stdout.bytes),"stderr":String::from_utf8_lossy(&outcome.stderr.bytes),"pdf_created":stage.0.join("probe.pdf").is_file()})
            }
            Err(error) => json!({"kind":kind,"error":error.to_string(),"pdf_created":false}),
        };
        println!("{observation}");
        observations.push(observation);
    }
    assert!(
        observations.iter().all(|row| row["pdf_created"] == true),
        "actual Office comparison failed"
    );
}
