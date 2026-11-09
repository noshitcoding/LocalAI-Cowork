//! Boundary proof for `supervise_confined`, the launch the toolbox uses for Tectonic.
//!
//! The TeX cases mirror the toolbox stage layout and arguments. They need
//! `COWORK_TEST_TECTONIC` (the pinned Tectonic 0.17.0 binary, alone in a scratch directory)
//! and `COWORK_TEST_TECTONIC_CACHE` (a scratch cache warmed for the documents below) and are
//! skipped without them unless `COWORK_REQUIRE_TEX_TESTS=1`. Never point them at the real
//! data directory: Windows keeps an ALL APPLICATION PACKAGES read grant on both trees.
#![cfg(any(windows, target_os = "linux"))]
use cowork_executor::{Confinement, Outcome, TrustedLaunch, supervise, supervise_confined};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    net::TcpListener,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

const MARKER: &str = "SECRETMARKER";
/// When TeX reads this file, the marker reaches the log (`\typeout`) and the page.
const CANARY: &str = "\\typeout{LEAKED-SECRETMARKER}SECRETMARKER\n";
/// 4x4 RGB PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x08, 0x02, 0x00, 0x00, 0x00, 0x26, 0x93, 0x09,
    0x29, 0x00, 0x00, 0x00, 0x29, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x0d, 0xc7, 0x31, 0x01, 0x00,
    0x00, 0x0c, 0xc2, 0x30, 0x84, 0x55, 0x18, 0x67, 0x45, 0x21, 0x70, 0xcb, 0x97, 0x24, 0x12, 0x1b,
    0x17, 0x13, 0x04, 0x8b, 0xe3, 0x53, 0xa9, 0xad, 0xeb, 0x67, 0x32, 0x3b, 0x37, 0x0f, 0xa7, 0x63,
    0x13, 0x41, 0x06, 0x1a, 0x2b, 0x55, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
    0x60, 0x82,
];

fn confinement(
    state: &Path,
    shared: Vec<PathBuf>,
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
) -> Confinement {
    Confinement {
        state: state.into(),
        shared_read: shared,
        read,
        write,
        memory_bytes: 2 << 30,
        processes: 1,
        cpu_millis: 2000,
    }
}
/// The program's shared tree: its directory on Windows (grants are per directory), the
/// binary itself on Linux.
fn program_tree(program: &Path) -> PathBuf {
    if cfg!(windows) {
        program.parent().unwrap().into()
    } else {
        program.into()
    }
}
fn linux_loader() -> Option<PathBuf> {
    if cfg!(not(target_os = "linux")) {
        return None;
    }
    ["/lib64/ld-linux-x86-64.so.2", "/lib/ld-linux-aarch64.so.1"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
}
fn tex_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}
/// Scratch directory under the user's temp folder: user-profile data, which no AppContainer
/// can read by default (the real threat: the owner's documents).
fn scratch() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[cfg(windows)]
fn without_inherited_host_access(path: &Path) {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{Authorization::*, *},
    };
    // A newly created private fixture only. The host can use the directory itself,
    // but child-created files must not depend on an unrelated parent ACL.
    let sddl: Vec<u16> = "D:P(A;;FA;;;SY)(A;;FA;;;OW)"
        .encode_utf16()
        .chain([0])
        .collect();
    let mut descriptor = ptr::null_mut();
    assert_ne!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        },
        0
    );
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let result = unsafe {
        SetFileSecurityW(
            wide.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    let error = std::io::Error::last_os_error();
    unsafe {
        LocalFree(descriptor);
    }
    assert_ne!(result, 0, "{error}");
}

#[test]
fn confined_probe_has_no_network_processes_or_outside_access() {
    let base = scratch();
    let state = base.path().join("state");
    let bin = base.path().join("bin");
    let own = base.path().join("own");
    for directory in [&state, &bin, &own] {
        fs::create_dir(directory).unwrap();
    }
    #[cfg(windows)]
    without_inherited_host_access(&own);
    let outside = base.path().join("outside.txt");
    fs::write(&outside, b"ORIGINAL").unwrap();
    // A private copy, so the shared grant never touches the build directory.
    let program = bin.join(if cfg!(windows) { "probe.exe" } else { "probe" });
    fs::copy(env!("CARGO_BIN_EXE_cowork-isolation-fixture"), &program).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    // Control: the host itself reaches the listener.
    drop(std::net::TcpStream::connect(address).unwrap());
    while listener.accept().is_err() {}
    let mut arguments = vec![
        "confined-probe".into(),
        outside.display().to_string(),
        own.display().to_string(),
        address.to_string(),
        std::process::id().to_string(),
    ];
    let mut shared = vec![program_tree(&program)];
    let mut executable = program.clone();
    // Landlock confines static programs such as the musl Tectonic; this glibc fixture runs
    // through the dynamic loader with read-only system libraries instead.
    if let Some(loader) = linux_loader() {
        arguments.insert(0, program.display().to_string());
        shared.extend([loader.clone(), "/usr/lib".into(), "/etc/ld.so.cache".into()]);
        executable = loader;
    }
    #[cfg(windows)]
    let environment = BTreeMap::from([
        ("uSeRpRoFiLe".into(), base.path().display().to_string()),
        ("ApPdAtA".into(), base.path().display().to_string()),
    ]);
    #[cfg(not(windows))]
    let environment = BTreeMap::new();
    let launch = TrustedLaunch {
        program: executable,
        arguments,
        directory: own.clone(),
        environment,
        timeout: Duration::from_secs(30),
        capture_bytes_per_stream: 64 * 1024,
    };
    let outcome = supervise_confined(
        &launch,
        &confinement(&state, shared, vec![], vec![own.clone()]),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "{}",
        String::from_utf8_lossy(&outcome.stderr.bytes)
    );
    let report: serde_json::Value = serde_json::from_slice(&outcome.stdout.bytes).unwrap();
    println!("confined probe: {report}");
    assert_eq!(report["write_own"], "ok");
    assert_eq!(fs::read(own.join("own.txt")).unwrap(), b"confined output");
    for key in [
        "read_outside",
        "append_outside",
        "create_beside_outside",
        "connect",
        "spawn",
    ] {
        assert_ne!(report[key], "ok", "{key}: {report}");
    }
    assert!(
        listener.accept().is_err(),
        "the confined probe reached the host listener"
    );
    assert_eq!(fs::read(&outside).unwrap(), b"ORIGINAL");
    assert!(!outside.with_extension("planted").exists());
    if cfg!(windows) {
        let folders = &report["known_folders"];
        let profile = PathBuf::from(folders["profile"].as_str().unwrap());
        assert!(profile.is_absolute());
        for name in ["local", "roaming"] {
            assert_eq!(folders[name]["hresult"], "0x00000000", "{folders}");
            assert_eq!(folders[name]["exists"], true, "{folders}");
            let path = PathBuf::from(folders[name]["path"].as_str().unwrap());
            assert!(path.starts_with(&profile), "{folders}");
        }
        assert!(
            !profile.parent().unwrap().exists(),
            "fresh profile storage survived cleanup"
        );
        assert!(
            report["host_process"]
                .as_str()
                .unwrap()
                .starts_with("denied"),
            "{report}"
        );
    } else {
        assert_eq!(report["host_process"], "PermissionDenied", "{report}");
        assert_ne!(report["symlink"], "ok", "{report}");
        assert_ne!(report["hardlink"], "ok", "{report}");
    }
}

struct Tex {
    tectonic: PathBuf,
    cache: PathBuf,
}
fn tex() -> Option<Tex> {
    match (
        std::env::var_os("COWORK_TEST_TECTONIC"),
        std::env::var_os("COWORK_TEST_TECTONIC_CACHE"),
    ) {
        (Some(tectonic), Some(cache)) => Some(Tex {
            tectonic: tectonic.into(),
            cache: cache.into(),
        }),
        _ => {
            assert!(
                std::env::var_os("COWORK_REQUIRE_TEX_TESTS").is_none(),
                "COWORK_TEST_TECTONIC and COWORK_TEST_TECTONIC_CACHE are required"
            );
            eprintln!("skipped: set COWORK_TEST_TECTONIC and COWORK_TEST_TECTONIC_CACHE");
            None
        }
    }
}

struct Stage(PathBuf);
impl Stage {
    /// The toolbox layout: request.json, in/tex (read-only), work/latex-out (writable), out.
    fn new(base: &Path, name: &str, body: &str) -> Self {
        let root = base.join(name);
        for directory in [
            "in/tex/chapters",
            "in/tex/figures",
            "work/latex-out/tmp",
            "out",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        #[cfg(windows)]
        without_inherited_host_access(&root.join("work/latex-out"));
        fs::write(root.join("request.json"), b"REQUEST").unwrap();
        fs::write(root.join("secret-parent.txt"), CANARY).unwrap();
        fs::write(
            root.join("in/tex/main.tex"),
            format!(
                "\\documentclass[11pt]{{article}}\n\\usepackage{{fontspec,graphicx}}\n\\setmainfont{{NotoSerif}}[Extension=.ttf,UprightFont=*-Regular,ItalicFont=*-Italic,BoldFont=*-Bold,BoldItalicFont=*-BoldItalic]\n\\newread\\r\n\\newwrite\\w\n\\begin{{document}}\nx\n{body}\n\\end{{document}}\n"
            ),
        )
        .unwrap();
        fs::write(
            root.join("in/tex/chapters/one.tex"),
            "\\typeout{LEGIT-CHAPTER-READ}\nChapter one text.\n",
        )
        .unwrap();
        fs::write(root.join("in/tex/figures/dot.png"), PNG).unwrap();
        fs::write(
            root.join("in/tex/refs.bib"),
            "@book{knuth, author={Donald Knuth}, title={The TeXbook}, year={1984}, note={\\typeout{LEGIT-BIB-READ}}}\n",
        )
        .unwrap();
        fs::write(root.join("in/tex/canary-copy.txt"), CANARY).unwrap();
        Self(root)
    }
    fn file(&self, name: &str) -> Vec<u8> {
        fs::read(self.0.join(name)).unwrap_or_default()
    }
}

struct Run {
    outcome: Outcome,
    log: String,
    pdf: Vec<u8>,
}
impl Run {
    /// Everything a later phase could quote: log, BibTeX log, PDF and the captured streams.
    fn leaked(&self, stage: &Stage) -> bool {
        let mut text = self.log.clone();
        text += &String::from_utf8_lossy(&stage.file("work/latex-out/main.blg"));
        text += &String::from_utf8_lossy(&self.pdf);
        text += &String::from_utf8_lossy(&self.outcome.stdout.bytes);
        text += &String::from_utf8_lossy(&self.outcome.stderr.bytes);
        text.contains(MARKER)
    }
    fn image(&self) -> bool {
        self.pdf.windows(6).any(|w| w == b"/Image")
    }
}

/// The toolbox's Tectonic invocation, confined or (`confined = false`, controls only) not.
fn compile(tex: &Tex, state: &Path, stage: &Stage, confined: bool) -> Run {
    let source = stage.0.join("in/tex");
    let output = stage.0.join("work/latex-out");
    let temp = output.join("tmp").display().to_string();
    let mut environment = BTreeMap::from([(
        "TECTONIC_CACHE_DIR".to_string(),
        tex.cache.display().to_string(),
    )]);
    for key in if cfg!(windows) {
        ["TEMP", "TMP"]
    } else {
        ["TMPDIR", "HOME"]
    } {
        environment.insert(key.into(), temp.clone());
    }
    let launch = TrustedLaunch {
        program: tex.tectonic.clone(),
        arguments: [
            "-X",
            "compile",
            "--untrusted",
            "--only-cached",
            "--keep-logs",
            "--outdir",
            &output.display().to_string(),
            &source.join("main.tex").display().to_string(),
        ]
        .map(String::from)
        .to_vec(),
        directory: source.clone(),
        environment,
        timeout: Duration::from_secs(120),
        capture_bytes_per_stream: 256 * 1024,
    };
    let outcome = if confined {
        let shared = vec![program_tree(&tex.tectonic), tex.cache.clone()];
        supervise_confined(
            &launch,
            &confinement(state, shared, vec![source], vec![output.clone()]),
            &AtomicBool::new(false),
        )
    } else {
        let mut launch = launch;
        if cfg!(windows) {
            launch
                .environment
                .insert("SystemRoot".into(), std::env::var("SystemRoot").unwrap());
        }
        supervise(&launch, &AtomicBool::new(false))
    }
    .unwrap();
    let mut log = String::new();
    if let Ok(mut file) = fs::File::open(output.join("main.log")) {
        file.read_to_string(&mut log).unwrap();
    }
    Run {
        outcome,
        log,
        pdf: fs::read(output.join("main.pdf")).unwrap_or_default(),
    }
}

fn listing(root: &Path) -> Vec<(PathBuf, u64)> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                pending.push(entry.path());
            }
            entries.push((entry.path(), metadata.len()));
        }
    }
    entries.sort();
    entries
}

fn copy_cache(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink(), "cache fixture must not contain links");
        if kind.is_dir() {
            copy_cache(&entry.path(), &destination.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
        }
    }
}

#[test]
fn confined_tectonic_reads_and_writes_only_its_stage() {
    let Some(tex) = tex() else { return };
    let base = scratch();
    let state = base.path().join("state");
    let outside = base.path().join("outside");
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let canary = outside.join("canary.txt");
    fs::write(&canary, CANARY).unwrap();
    fs::write(outside.join("canary.png"), PNG).unwrap();
    fs::write(
        outside.join("canary.bib"),
        "@book{leak, author={A}, title={SECRETMARKER}, year={2026}, note={\\typeout{LEAKED-SECRETMARKER}}}\n",
    )
    .unwrap();
    let target = outside.join("target.txt");
    fs::write(&target, b"ORIGINAL").unwrap();
    let cache_before = listing(&tex.cache);
    let program_dir = tex.tectonic.parent().unwrap().to_path_buf();
    let abs = tex_path(&canary);
    let caret = abs.replace(':', "^^3a").replace('/', "^^2f");
    let openout = |path: &str| {
        format!(
            "\\immediate\\openout\\w={path}\\relax\\immediate\\write\\w{{hello}}\\immediate\\closeout\\w"
        )
    };
    let mut failures = Vec::new();
    let mut check = |name: &str, ok: bool, run: &Run| {
        let exit = run.outcome.exit_code;
        println!(
            "{name}: {} (exit {exit:?})",
            if ok { "ok" } else { "FAILED" }
        );
        if !ok {
            let stderr = String::from_utf8_lossy(&run.outcome.stderr.bytes);
            println!("--- stderr\n{stderr}\n--- log\n{}", run.log);
            failures.push(name.to_string());
        }
    };

    // 1. A legitimate multi-file document with images and a bibliography compiles.
    let stage = Stage::new(
        base.path(),
        "legit",
        "\\input{chapters/one}\n\\includegraphics[width=1cm]{figures/dot.png}\n\\includegraphics[width=1cm]{figures/dot}\n\\nocite{*}\\bibliographystyle{plain}\\bibliography{refs}",
    );
    let run = compile(&tex, &state, &stage, true);
    check(
        "legit multi-file document",
        run.outcome.exit_code == Some(0)
            && run.pdf.starts_with(b"%PDF")
            && run.log.contains("LEGIT-CHAPTER-READ")
            && run.log.contains("LEGIT-BIB-READ")
            && run.image()
            && !run.leaked(&stage),
        &run,
    );
    // Controls: the detector sees a staged copy of the canary, and the same documents leak
    // and overwrite without confinement.
    let stage = Stage::new(base.path(), "control-staged", "\\input{canary-copy.txt}");
    let run = compile(&tex, &state, &stage, true);
    check(
        "control: staged canary is detected",
        run.leaked(&stage),
        &run,
    );
    let stage = Stage::new(
        base.path(),
        "control-unconfined",
        &format!("\\input{{{abs}}}\n{}", openout(&tex_path(&target))),
    );
    // The unconfined control may legitimately update Tectonic's prefetch index.
    // Give it a separate cache so those writes cannot invalidate the boundary proof.
    let control_cache = base.path().join("control-cache");
    copy_cache(&tex.cache, &control_cache);
    let control_tex = Tex {
        tectonic: tex.tectonic.clone(),
        cache: control_cache,
    };
    let run = compile(&control_tex, &state, &stage, false);
    let overwritten = fs::read(&target).unwrap() != b"ORIGINAL";
    check(
        "control: unconfined Tectonic leaks and overwrites",
        run.leaked(&stage) && overwritten,
        &run,
    );
    fs::write(&target, b"ORIGINAL").unwrap();

    // 2. Reads outside the stage fail.
    for (index, (name, body)) in [
        ("\\input{C:/abs}", format!("\\input{{{abs}}}")),
        ("\\input{../../x}", "\\input{../../secret-parent.txt}".to_string()),
        ("\\openin", format!("\\openin\\r={abs}\\relax\\ifeof\\r\\typeout{{OPENIN-EOF}}\\else\\read\\r to\\x\\typeout{{GOT:\\meaning\\x}}\\fi")),
        ("^^ path", format!("\\input{{{caret}}}")),
        (
            "\\bibliography{C:/abs}",
            format!("\\nocite{{*}}\\bibliographystyle{{plain}}\\bibliography{{{}}}", tex_path(&outside.join("canary"))),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let stage = Stage::new(base.path(), &format!("read-{index}"), &body);
        let run = compile(&tex, &state, &stage, true);
        check(name, !run.leaked(&stage), &run);
    }
    let stage = Stage::new(
        base.path(),
        "read-image",
        &format!(
            "\\includegraphics{{{}}}",
            tex_path(&outside.join("canary.png"))
        ),
    );
    let run = compile(&tex, &state, &stage, true);
    check(
        "\\includegraphics{C:/abs.png}",
        !run.image() && run.outcome.exit_code != Some(0),
        &run,
    );

    // Documented limit, observed only: a plain AppContainer still reads what ALL
    // APPLICATION PACKAGES can read, e.g. the Windows directory.
    if cfg!(windows) {
        let system = tex_path(&Path::new(&std::env::var("SystemRoot").unwrap()).join("win.ini"));
        let stage = Stage::new(
            base.path(),
            "observe-system",
            &format!(
                "\\openin\\r={system}\\relax\\ifeof\\r\\typeout{{SYSTEM-UNREADABLE}}\\else\\read\\r to\\x\\typeout{{SYSTEM-READ:\\meaning\\x}}\\fi"
            ),
        );
        let run = compile(&tex, &state, &stage, true);
        println!(
            "observation (not a boundary): win.ini readable under confinement: {}",
            run.log.contains("SYSTEM-READ:")
        );
    }

    // 3. Writes outside work/latex-out fail and leave their targets unchanged.
    let stage = Stage::new(base.path(), "write-abs", &openout(&tex_path(&target)));
    let run = compile(&tex, &state, &stage, true);
    check(
        "\\openout to an absolute path",
        fs::read(&target).unwrap() == b"ORIGINAL",
        &run,
    );
    let stage = Stage::new(
        base.path(),
        "write-parent",
        &[
            "../../written-parent.txt",
            "../../request.json",
            "../../out/evil.txt",
            "../../in/tex/evil.txt",
        ]
        .map(openout)
        .join("\n"),
    );
    let run = compile(&tex, &state, &stage, true);
    check(
        "\\openout to parent paths (stage control files, out/, in/tex)",
        !stage.0.join("written-parent.txt").exists()
            && stage.file("request.json") == b"REQUEST"
            && !stage.0.join("out/evil.txt").exists()
            && !stage.0.join("in/tex/evil.txt").exists(),
        &run,
    );
    let stage = Stage::new(
        base.path(),
        "write-shared",
        &format!(
            "{}\n{}",
            openout(&tex_path(&tex.cache.join("evil.txt"))),
            openout(&tex_path(&program_dir.join("evil.txt")))
        ),
    );
    let run = compile(&tex, &state, &stage, true);
    check(
        "\\openout into the cache and the program directory",
        !tex.cache.join("evil.txt").exists() && !program_dir.join("evil.txt").exists(),
        &run,
    );
    let _ = fs::remove_file(tex.cache.join("evil.txt"));
    let _ = fs::remove_file(program_dir.join("evil.txt"));
    assert_eq!(
        listing(&tex.cache),
        cache_before,
        "the read-only cache changed"
    );
    assert!(failures.is_empty(), "boundary failures: {failures:?}");
    let leftovers: Vec<_> = fs::read_dir(&state)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(
        leftovers.is_empty(),
        "run journals left behind: {leftovers:?}"
    );
}
