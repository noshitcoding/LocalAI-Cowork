use cowork_executor::{Termination, TrustedLaunch, supervise};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn launch(directory: &Path, mode: &str) -> TrustedLaunch {
    TrustedLaunch {
        program: PathBuf::from(env!("CARGO_BIN_EXE_cowork-executor-fixture")),
        arguments: vec![mode.into()],
        directory: directory.to_path_buf(),
        environment: Default::default(),
        timeout: Duration::from_secs(10),
        capture_bytes_per_stream: 4096,
    }
}
fn wait_for_heartbeat(directory: &Path) {
    let until = Instant::now() + Duration::from_secs(5);
    while !directory.join("heartbeat").exists() {
        assert!(Instant::now() < until, "fixture did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn stopped_heartbeat(directory: &Path) {
    let file = directory.join("heartbeat");
    std::thread::sleep(Duration::from_millis(100));
    let before = std::fs::read(&file).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        std::fs::read(file).unwrap(),
        before,
        "descendant kept writing after termination"
    );
}

#[test]
fn direct_arguments_unicode_environment_and_stdin_are_exact() {
    let temp = tempfile::tempdir().unwrap();
    let mut launch = launch(temp.path(), "echo");
    let values = [
        "",
        "with space",
        "quote\"inside",
        "trailing\\",
        "ends \\",
        "\\\"quoted\\\"",
        "$(touch x); & | %PATH%",
        "Prüfung 📄",
    ];
    launch.arguments.extend(values.map(str::to_owned));
    launch
        .environment
        .insert("COWORK_EXECUTOR_MARKER".into(), "explicit only".into());
    let result = supervise(&launch, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.termination, Termination::Exited);
    assert_eq!(result.exit_code, Some(0));
    assert!(result.output_complete);
    let value: serde_json::Value = serde_json::from_slice(&result.stdout.bytes).unwrap();
    assert_eq!(value["arguments"], serde_json::json!(values));
    assert_eq!(value["path_inherited"], false);
    assert_eq!(value["marker"], "explicit only");
    assert_eq!(value["stdin_bytes"], 0);
    assert_eq!(value["supervised_before_first_action"], true);
    assert_eq!(
        std::fs::canonicalize(value["directory"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(temp.path()).unwrap()
    );
    assert!(!temp.path().join("x").exists());
}
#[test]
fn nonzero_exit_is_recorded_without_repetition() {
    let temp = tempfile::tempdir().unwrap();
    let mut l = launch(temp.path(), "exit");
    l.arguments
        .push(temp.path().join("effect").to_str().unwrap().into());
    let result = supervise(&l, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.exit_code, Some(37));
    assert_eq!(result.termination, Termination::Exited);
    assert_eq!(
        std::fs::read(temp.path().join("effect")).unwrap(),
        b"executed once"
    );
}
#[test]
fn simultaneous_binary_output_is_drained_and_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let result = supervise(&launch(temp.path(), "flood"), &AtomicBool::new(false)).unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert!(result.output_complete);
    for output in [&result.stdout, &result.stderr] {
        assert_eq!(output.bytes.len(), 4096);
        assert_eq!(output.total_bytes, 100 * 65536 + 1);
        assert!(output.truncated && output.invalid_utf8);
    }
}
#[test]
fn output_flood_cannot_starve_a_monotonic_timeout() {
    let temp = tempfile::tempdir().unwrap();
    let mut l = launch(temp.path(), "endless");
    // Allow process startup under parallel CI load before testing sustained output.
    l.timeout = Duration::from_secs(1);
    let result = supervise(&l, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.termination, Termination::TimedOut);
    assert!(result.duration_ms < 3500);
    assert!(result.stdout.truncated);
    assert!(result.output_complete);
}
#[test]
fn cancellation_stops_root_and_background_descendant() {
    let temp = tempfile::tempdir().unwrap();
    let mut l = launch(temp.path(), "tree");
    l.arguments.push(temp.path().to_str().unwrap().into());
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let worker = std::thread::spawn(move || supervise(&l, &signal));
    wait_for_heartbeat(temp.path());
    let started = Instant::now();
    cancel.store(true, Ordering::Release);
    let result = worker.join().unwrap().unwrap();
    assert_eq!(result.termination, Termination::Canceled);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(result.output_complete);
    stopped_heartbeat(temp.path());
}
#[test]
fn root_exit_cleans_background_descendants_and_pipes() {
    let temp = tempfile::tempdir().unwrap();
    let mut l = launch(temp.path(), "root-exit");
    l.arguments.push(temp.path().to_str().unwrap().into());
    let result = supervise(&l, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.termination, Termination::Exited);
    assert_eq!(result.exit_code, Some(0));
    assert!(result.output_complete);
    stopped_heartbeat(temp.path());
}
#[test]
fn canceled_before_start_never_creates_a_process_or_effect() {
    let temp = tempfile::tempdir().unwrap();
    let mut l = launch(temp.path(), "exit");
    l.arguments
        .push(temp.path().join("effect").to_str().unwrap().into());
    let result = supervise(&l, &AtomicBool::new(true)).unwrap();
    assert_eq!(result.process_id, None);
    assert_eq!(result.termination, Termination::Canceled);
    assert!(!temp.path().join("effect").exists());
}
#[test]
fn invalid_configuration_is_rejected_before_launch() {
    let temp = tempfile::tempdir().unwrap();
    let normal = launch(temp.path(), "echo");
    let mut invalid = normal.clone();
    invalid.program = PathBuf::from("relative");
    assert!(supervise(&invalid, &AtomicBool::new(false)).is_err());
    let mut invalid = normal.clone();
    invalid.arguments.push("nul\0argument".into());
    assert!(supervise(&invalid, &AtomicBool::new(false)).is_err());
    let mut invalid = normal.clone();
    invalid.environment.insert("Path".into(), "a".into());
    invalid.environment.insert("PATH".into(), "b".into());
    assert!(supervise(&invalid, &AtomicBool::new(false)).is_err());
    let mut invalid = normal.clone();
    invalid.capture_bytes_per_stream = 0;
    assert!(supervise(&invalid, &AtomicBool::new(false)).is_err());
    let mut invalid = normal;
    invalid.timeout = Duration::ZERO;
    assert!(supervise(&invalid, &AtomicBool::new(false)).is_err());
}
fn parent_death(mode: &str) {
    let temp = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_cowork-executor-fixture"));
    command
        .arg(mode)
        .arg(temp.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut parent = command.spawn().unwrap();
    wait_for_heartbeat(temp.path());
    parent.kill().unwrap();
    parent.wait().unwrap();
    stopped_heartbeat(temp.path());
}
#[test]
fn hard_kill_of_supervisor_stops_direct_adapter() {
    parent_death("supervisor-single");
}
#[cfg(windows)]
#[test]
fn windows_job_close_stops_descendants_after_supervisor_hard_kill() {
    parent_death("supervisor-tree");
}

#[test]
fn explicit_parent_secret_fixture_is_not_inherited() {
    let temp = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_cowork-executor-fixture"));
    command
        .arg("supervisor-echo")
        .arg(temp.path())
        .env(
            "COWORK_EXECUTOR_PRIVATE_FIXTURE",
            "synthetic-not-a-real-secret",
        )
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output().unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["private_environment_inherited"], false);
}
#[cfg(windows)]
#[test]
fn unrelated_inheritable_handle_never_reaches_child() {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::WAIT_TIMEOUT, Security::SECURITY_ATTRIBUTES, System::Threading::*,
    };
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let event = unsafe { CreateEventW(&security, 1, 0, std::ptr::null()) };
    assert!(!event.is_null());
    let event = unsafe { OwnedHandle::from_raw_handle(event) };
    let temp = tempfile::tempdir().unwrap();
    let mut l = launch(temp.path(), "foreign-handle");
    l.arguments
        .push((event.as_raw_handle() as usize).to_string());
    let result = supervise(&l, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(
        unsafe { WaitForSingleObject(event.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
}
#[cfg(windows)]
#[test]
fn windows_child_cannot_request_job_breakaway() {
    let temp = tempfile::tempdir().unwrap();
    let mut l = launch(temp.path(), "breakaway");
    l.arguments.push(temp.path().to_str().unwrap().into());
    let result = supervise(&l, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout.bytes, b"breakaway denied\n");
    assert!(!temp.path().join("heartbeat").exists());
}

#[cfg(windows)]
#[test]
fn windows_working_directory_budget_counts_utf16_not_utf8_or_codepoints() {
    use cowork_executor::working_directory_path_fits;
    for directory in [
        format!("C:\\{}", "x".repeat(255)),
        format!("C:\\{}", "ä".repeat(255)),
        format!("C:\\{}x", "📄".repeat(127)),
    ] {
        use std::os::windows::ffi::OsStrExt;
        let directory = PathBuf::from(directory);
        assert_eq!(directory.as_os_str().encode_wide().count(), 258);
        assert!(working_directory_path_fits(&directory));
        let mut longer = directory.into_os_string();
        longer.push("x");
        assert!(!working_directory_path_fits(Path::new(&longer)));
    }
}

#[cfg(windows)]
#[test]
fn windows_boundary_launch_succeeds_and_longer_cwd_is_rejected_before_effect() {
    use std::os::windows::ffi::OsStrExt;
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path().as_os_str().encode_wide().count() + 1;
    let boundary = temp.path().join("x".repeat(258 - prefix));
    std::fs::create_dir(&boundary).unwrap();
    let once = supervise(&launch(&boundary, "echo"), &AtomicBool::new(false)).unwrap();
    assert_eq!(once.exit_code, Some(0));
    assert_eq!(once.termination, Termination::Exited);
    assert!(once.output_complete);

    let longer = temp.path().join("y".repeat(259 - prefix));
    std::fs::create_dir(&longer).unwrap();
    let marker = temp.path().join("must-not-execute");
    let mut invalid = launch(&longer, "exit");
    invalid.arguments.push(marker.to_str().unwrap().into());
    let error = supervise(&invalid, &AtomicBool::new(false)).unwrap_err();
    assert!(
        matches!(error, cowork_executor::Error::Invalid(message) if message.contains("working directory is too long"))
    );
    assert!(!marker.exists());
}
