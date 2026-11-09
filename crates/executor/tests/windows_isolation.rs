#![cfg(windows)]
use cowork_executor::windows_isolation::{Capsule, Limits};
use cowork_executor::{Outcome, Termination};
use sha2::{Digest, Sha256};
use std::{sync::atomic::AtomicBool, time::Duration};

fn run(capsule: &mut Capsule, args: Vec<String>) -> Outcome {
    let outcome = capsule
        .run(
            "probe.exe",
            args,
            Duration::from_secs(10),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "{outcome:?}; stderr {}",
        String::from_utf8_lossy(&outcome.stderr.bytes)
    );
    outcome
}
fn json(outcome: &Outcome) -> serde_json::Value {
    serde_json::from_slice(&outcome.stdout.bytes).unwrap()
}

fn limits() -> Limits {
    Limits {
        memory_bytes: 256 * 1024 * 1024,
        processes: 4,
        cpu_millis: 1000,
    }
}
fn stage(capsule: &mut Capsule) {
    let bytes = std::fs::read(env!("CARGO_BIN_EXE_cowork-isolation-fixture")).unwrap();
    capsule
        .stage_program(
            "probe.exe",
            &bytes,
            &format!("{:x}", Sha256::digest(&bytes)),
        )
        .unwrap();
}

#[test]
fn win32k_gui_calls_are_disabled_before_first_instruction() {
    let parent = tempfile::tempdir().unwrap();
    let mut capsule = Capsule::create(parent.path(), limits()).unwrap();
    stage(&mut capsule);
    let result = json(&run(&mut capsule, vec!["isolation-desktop".into()]));
    assert_eq!(result["win32k_disabled"], true);
    assert_eq!(result["gui_library_blocked"], true);
    println!("GUI observation: {result}");
    capsule.close().unwrap();
}
#[test]
fn child_spawn_limitation_is_reported_without_fallback() {
    let parent = tempfile::tempdir().unwrap();
    let mut capsule = Capsule::create(parent.path(), limits()).unwrap();
    stage(&mut capsule);
    let result = json(&run(&mut capsule, vec!["isolation-child-probe".into()]));
    println!("unqualified child-process observation: {result}");
    // Capability limitation on the reference OS, NOT a descendant-containment test.
    // Unexpected success requires validating the descendant before qualifying a bundle.
    assert_eq!(
        result["spawn"], 0,
        "child capability changed; qualification required: {result}"
    );
    assert_eq!(result["code"], 5);
    assert!(result["marker"].is_null());
    capsule.close().unwrap();
}

#[test]
fn only_staged_input_and_workspace_are_accessible() {
    let parent = tempfile::tempdir().unwrap();
    let secret = parent.path().join("synthetic-private.txt");
    std::fs::write(&secret, b"synthetic host data").unwrap();
    let mut sibling = Capsule::create(parent.path(), limits()).unwrap();
    sibling
        .stage_input("private.txt", b"other capsule input")
        .unwrap();
    stage(&mut sibling);
    run(&mut sibling, vec!["echo".into()]); // Its own package ACLs are now active.
    let all_packages = parent.path().join("all-application-packages.txt");
    std::fs::write(&all_packages, b"synthetic all-application-packages canary").unwrap();
    allow_all_application_packages(&all_packages);
    assert_eq!(
        std::fs::read(&all_packages).unwrap(),
        b"synthetic all-application-packages canary"
    );
    let mut capsule = Capsule::create(parent.path(), limits()).unwrap();
    stage(&mut capsule);
    capsule.stage_input("input.txt", b"approved input").unwrap();
    let paths = [
        secret,
        sibling.workspace().join("private.txt"),
        capsule.directory().join("identity.json"),
        capsule.directory().join("owned"),
        capsule.directory().join("session.lock"),
        all_packages,
    ];
    let mut args = vec!["isolation-files".into()];
    args.extend(paths.iter().map(|p| p.to_string_lossy().into_owned()));
    let result = json(&run(&mut capsule, args));
    assert_eq!(result["denied"], paths.len());
    assert_eq!(
        std::fs::read(capsule.workspace().join("output.txt")).unwrap(),
        b"isolated output"
    );
    capsule.close().unwrap();
    sibling.close().unwrap();
}

fn allow_all_application_packages(path: &std::path::Path) {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{Authorization::*, *},
    };
    // This modifies only a newly created synthetic canary, never an existing host file.
    let sddl: Vec<u16> = "D:P(A;;FA;;;SY)(A;;FA;;;OW)(A;;FR;;;AC)"
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
fn isolated_network_probe_cannot_reach_live_loopback() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let positive = std::net::TcpStream::connect(address).unwrap();
    let (accepted, _) = listener.accept().unwrap();
    drop((positive, accepted));
    listener.set_nonblocking(true).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut capsule = Capsule::create(parent.path(), limits()).unwrap();
    stage(&mut capsule);
    let result = json(&run(
        &mut capsule,
        vec!["isolation-network".into(), address.to_string()],
    ));
    println!("network observation: {result}");
    assert_eq!(result["tcp_connected"], false);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    capsule.close().unwrap();
}

#[test]
fn job_enforces_memory_and_process_limits() {
    let parent = tempfile::tempdir().unwrap();
    let mut bound = limits();
    bound.processes = 1;
    let mut capsule = Capsule::create(parent.path(), bound).unwrap();
    stage(&mut capsule);
    let result = json(&run(&mut capsule, vec!["isolation-limits".into()]));
    println!("resource observation: {result}");
    assert_eq!(result["memory"], bound.memory_bytes);
    assert_eq!(result["processes"], 1);
    assert_eq!(result["allocation_error"], 1455);
    assert_eq!(result["child_error"], 1816);
    assert_eq!(result["cpu_flags"], 5);
    assert!(result["cpu_rate"].as_u64().unwrap() > 0);
    capsule.close().unwrap();
}

#[test]
fn cpu_hard_cap_throttles_a_busy_process() {
    let parent = tempfile::tempdir().unwrap();
    let mut bound = limits();
    bound.cpu_millis = 100;
    let mut capsule = Capsule::create(parent.path(), bound).unwrap();
    stage(&mut capsule);
    let result = json(&run(&mut capsule, vec!["isolation-cpu".into()]));
    println!("CPU observation: {result}");
    let cpu = result["cpu_ms"].as_u64().unwrap();
    let wall = result["wall_ms"].as_u64().unwrap();
    assert!(wall >= 4000 && cpu > 0 && cpu * 100 / wall < 25, "{result}");
    capsule.close().unwrap();
}

#[test]
fn canceled_before_start_is_never_replayed() {
    let parent = tempfile::tempdir().unwrap();
    let mut capsule = Capsule::create(parent.path(), limits()).unwrap();
    stage(&mut capsule);
    let outcome = capsule
        .run(
            "probe.exe",
            vec!["exit".into(), "once".into()],
            Duration::from_secs(5),
            &AtomicBool::new(true),
        )
        .unwrap();
    assert_eq!(outcome.termination, Termination::Canceled);
    assert!(outcome.process_id.is_none());
    assert!(!capsule.workspace().join("once").exists());
    assert!(
        capsule
            .run(
                "probe.exe",
                vec!["echo".into()],
                Duration::from_secs(5),
                &AtomicBool::new(false)
            )
            .is_err()
    );
    assert!(capsule.stage_input("later.txt", b"late").is_err());
    capsule.close().unwrap();
}

#[test]
fn timeout_terminates_an_actually_running_isolated_process() {
    let parent = tempfile::tempdir().unwrap();
    let mut capsule = Capsule::create(parent.path(), limits()).unwrap();
    stage(&mut capsule);
    let work = capsule.workspace();
    let outcome = capsule
        .run(
            "probe.exe",
            vec!["heartbeat".into(), work.to_string_lossy().into_owned()],
            Duration::from_secs(2),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(outcome.termination, Termination::TimedOut, "{outcome:?}");
    assert!(outcome.output_complete);
    heartbeat_stopped(&work.join("heartbeat"));
    capsule.close().unwrap();
}

fn heartbeat_stopped(path: &std::path::Path) {
    let bytes =
        std::fs::read(path).expect("heartbeat must prove the isolated process actually ran");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        std::fs::read(path).unwrap(),
        bytes,
        "isolated process is still running"
    );
}

#[test]
fn killed_owner_terminates_isolated_process_and_recovery_never_relaunches() {
    use cowork_executor::windows_isolation::recover;
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
        time::Instant,
    };
    let parent = tempfile::tempdir().unwrap();
    let mut owner = Command::new(env!("CARGO_BIN_EXE_cowork-executor-fixture"))
        .arg("isolation-owner")
        .arg(parent.path())
        .arg(env!("CARGO_BIN_EXE_cowork-isolation-fixture"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    let mut stdout = BufReader::new(owner.stdout.take().unwrap());
    stdout.read_line(&mut line).unwrap();
    let info: serde_json::Value = serde_json::from_str(&line).unwrap();
    let root = std::path::PathBuf::from(info["root"].as_str().unwrap());
    let heartbeat = root.join("work/heartbeat");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !heartbeat.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !heartbeat.exists() {
        owner.kill().unwrap();
        let output = owner.wait_with_output().unwrap();
        panic!(
            "isolated child did not start: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let live_recovery = recover(&root);
    owner.kill().unwrap();
    owner.wait().unwrap();
    assert!(
        live_recovery.is_err(),
        "live lock must prevent profile deletion"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match recover(&root) {
            Ok(()) => break,
            Err(e) if Instant::now() < deadline => {
                let _ = e;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("recovery did not finish: {e:?}"),
        }
    }
    heartbeat_stopped(&heartbeat);
    assert!(root.join("closed").is_file());
    recover(&root).unwrap();
    heartbeat_stopped(&heartbeat);
}

#[test]
fn staging_rejects_escape_names_and_changed_executable() {
    let parent = tempfile::tempdir().unwrap();
    let mut capsule = Capsule::create(parent.path(), limits()).unwrap();
    for name in [
        "../escape",
        "/absolute",
        "C:/absolute",
        "a\\b",
        "a:b",
        "con.txt",
        "nul.exe",
        "a/../b",
        "a//b",
        "a/",
        "a.",
    ] {
        assert!(capsule.stage_input(name, b"x").is_err(), "accepted {name}");
    }
    assert!(
        capsule
            .stage_program("con.exe", b"x", &format!("{:x}", Sha256::digest(b"x")))
            .is_err()
    );
    assert!(
        capsule
            .stage_program("probe.exe", b"x", &"0".repeat(64))
            .is_err()
    );
    stage(&mut capsule);
    std::fs::write(capsule.directory().join("bundle/probe.exe"), b"changed").unwrap();
    assert!(
        capsule
            .run(
                "probe.exe",
                vec!["echo".into()],
                Duration::from_secs(1),
                &AtomicBool::new(false)
            )
            .is_err()
    );
    capsule.close().unwrap();
}

#[test]
fn recovery_rejects_missing_ownership_corruption_and_foreign_identity() {
    use cowork_executor::windows_isolation::recover;
    let parent = tempfile::tempdir().unwrap();
    let capsule = Capsule::create(parent.path(), limits()).unwrap();
    let root = capsule.close().unwrap();
    let owned = std::fs::read(root.join("owned")).unwrap();
    std::fs::remove_file(root.join("owned")).unwrap();
    assert!(recover(&root).is_err());
    std::fs::write(root.join("owned"), b"foreign").unwrap();
    assert!(recover(&root).is_err());
    std::fs::write(root.join("owned"), owned).unwrap();
    std::fs::write(root.join("closed"), b"corrupt").unwrap();
    assert!(recover(&root).is_err());
    let journal = root.join("identity.json");
    let original = std::fs::read(&journal).unwrap();
    std::fs::write(&journal, vec![b' '; 2048]).unwrap();
    assert!(recover(&root).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    value["profile"] = "LocalAI.Cowork.V2.00000000000000000000000000000000".into();
    std::fs::write(&journal, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(recover(&root).is_err());
}
#[test]
fn isolated_process_cannot_open_host_process_for_injection_or_handle_theft() {
    let parent = tempfile::tempdir().unwrap();
    let mut capsule = Capsule::create(parent.path(), limits()).unwrap();
    stage(&mut capsule);
    let result = json(&run(
        &mut capsule,
        vec![
            "isolation-host-process".into(),
            std::process::id().to_string(),
        ],
    ));
    assert_eq!(result["host_process_denied"], true);
    capsule.close().unwrap();
}

#[test]
fn cancellation_stops_an_actually_running_isolated_process() {
    use std::{sync::atomic::Ordering, time::Instant};
    let parent = tempfile::tempdir().unwrap();
    let mut capsule = Capsule::create(parent.path(), limits()).unwrap();
    stage(&mut capsule);
    let work = capsule.workspace();
    let heartbeat = work.join("heartbeat");
    let cancel = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let signal = scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !heartbeat.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            cancel.store(true, Ordering::Release);
            heartbeat.exists()
        });
        let outcome = capsule
            .run(
                "probe.exe",
                vec!["heartbeat".into(), work.to_string_lossy().into_owned()],
                Duration::from_secs(10),
                &cancel,
            )
            .unwrap();
        assert!(signal.join().unwrap(), "isolated process never started");
        assert_eq!(outcome.termination, Termination::Canceled);
        assert!(outcome.duration_ms < 5000 && outcome.output_complete);
    });
    heartbeat_stopped(&heartbeat);
    capsule.close().unwrap();
}

#[test]
fn failed_isolated_action_is_not_implicitly_repeated() {
    let parent = tempfile::tempdir().unwrap();
    let mut capsule = Capsule::create(parent.path(), limits()).unwrap();
    stage(&mut capsule);
    let result = capsule
        .run(
            "probe.exe",
            vec!["exit".into(), "once".into()],
            Duration::from_secs(10),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(result.exit_code, Some(37));
    assert_eq!(
        std::fs::read(capsule.workspace().join("once")).unwrap(),
        b"executed once"
    );
    assert!(
        capsule
            .run(
                "probe.exe",
                vec!["echo".into()],
                Duration::from_secs(10),
                &AtomicBool::new(false)
            )
            .is_err()
    );
    let root = capsule.close().unwrap();
    cowork_executor::windows_isolation::recover(&root).unwrap();
    assert_eq!(
        std::fs::read(root.join("work/once")).unwrap(),
        b"executed once"
    );
}
#[test]
fn isolated_echo_starts_under_required_identity() {
    let parent = tempfile::tempdir().unwrap();
    let mut capsule = Capsule::create(parent.path(), limits()).unwrap();
    stage(&mut capsule);
    let outcome = capsule
        .run(
            "probe.exe",
            vec!["echo".into(), "isolated".into()],
            Duration::from_secs(10),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(outcome.exit_code, Some(0), "{outcome:?}");
    let result: serde_json::Value = serde_json::from_slice(&outcome.stdout.bytes).unwrap();
    assert_eq!(result["supervised_before_first_action"], true);
    assert_eq!(result["path_inherited"], false);
    assert_eq!(result["stdin_bytes"], 0);
    let root = capsule.close().unwrap();
    assert!(root.join("closed").is_file());
}
