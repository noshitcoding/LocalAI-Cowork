//! Real supervised probe and separate contender process; no Python/Office install needed.
use cowork_toolbox::{Config, Toolbox};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::write(self.0.join("release"), b"release own fixture");
        let until = Instant::now() + Duration::from_secs(5);
        while self.0.join("started.json").exists()
            && !self.0.join("finished.json").exists()
            && Instant::now() < until
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn wait(path: &Path) {
    let until = Instant::now() + Duration::from_secs(10);
    while !path.is_file() {
        assert!(
            Instant::now() < until,
            "own probe fixture did not reach {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn contend(root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cowork-document-probe-fixture"))
        .arg("--attempt")
        .arg(root)
        .output()
        .unwrap()
}
fn check(phase: &str, exit_code: i32) {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("cp-{}", uuid::Uuid::new_v4().simple())));
    fs::create_dir(&scratch.0).unwrap();
    let root = scratch.0.join("root");
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_cowork-document-probe-fixture"));
    let mut programs = BTreeMap::from([("python".into(), fixture.clone())]);
    if phase == "office" {
        programs.insert("libreoffice".into(), fixture);
    }
    let mut environment: BTreeMap<String, String> = ["SystemRoot", "WINDIR", "TEMP", "TMP"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| (name.into(), value)))
        .collect();
    for (key, value) in [
        (
            "COWORK_PROBE_FIXTURE_CONTROL",
            scratch.0.to_string_lossy().into_owned(),
        ),
        (
            "COWORK_PROBE_FIXTURE_ROOT",
            root.to_string_lossy().into_owned(),
        ),
        ("COWORK_PROBE_FIXTURE_PHASE", phase.into()),
        ("COWORK_PROBE_FIXTURE_EXIT", exit_code.to_string()),
    ] {
        environment.insert(key.into(), value);
    }
    let owner = Toolbox::open(Config {
        root: root.clone(),
        programs,
        environment,
        github_token: None,
    })
    .unwrap();
    assert_eq!(owner.availability().document_status, "probing");
    wait(&scratch.0.join("started.json"));
    let started: serde_json::Value =
        serde_json::from_slice(&fs::read(scratch.0.join("started.json")).unwrap()).unwrap();
    let input = PathBuf::from(started["input"].as_str().unwrap());
    assert!(input.is_file());
    assert!(!scratch.0.join("finished.json").exists());
    // No Toolbox Arc remains. Only the actual availability thread may own the lease.
    drop(owner);
    let second = contend(&root);
    let input_preserved_while_running = input.is_file();
    let profile_preserved_while_running = started["profile"]
        .as_str()
        .is_none_or(|path| Path::new(path).is_dir());
    let still_waiting = !scratch.0.join("finished.json").exists();
    fs::write(scratch.0.join("release"), b"finish own child").unwrap();
    wait(&scratch.0.join("finished.json"));
    let finished: serde_json::Value =
        serde_json::from_slice(&fs::read(scratch.0.join("finished.json")).unwrap()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let reopened = loop {
        let result = contend(&root);
        if result.status.success() {
            break result;
        }
        assert_eq!(
            result.status.code(),
            Some(2),
            "unexpected contender error: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            Instant::now() < until,
            "probe lease was not released after completion"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    println!(
        "{}",
        serde_json::json!({"phase":phase,"child_pid":started["pid"],"probe_exit":exit_code,"second_exit":second.status.code(),"second_stdout":String::from_utf8_lossy(&second.stdout).trim(),"input_preserved_while_running":input_preserved_while_running,"profile_preserved_while_running":profile_preserved_while_running,"child_still_waiting":still_waiting,"finished":finished,"reopened_after_probe":reopened.status.success()})
    );
    assert_eq!(
        second.status.code(),
        Some(2),
        "a second process acquired the root while its original probe was still running"
    );
    assert_eq!(String::from_utf8_lossy(&second.stdout).trim(), "busy");
    assert!(still_waiting && input_preserved_while_running && profile_preserved_while_running);
    assert_eq!(finished["original_input_preserved"], true);
    assert_eq!(finished["original_profile_preserved"], true);
    assert_eq!(String::from_utf8_lossy(&reopened.stdout).trim(), "opened");
}

#[test]
fn python_probe_keeps_original_root_after_last_toolbox_reference() {
    check("python", 0);
}
#[test]
fn office_probe_keeps_original_root_after_last_toolbox_reference() {
    check("office", 0);
}
#[test]
fn failing_office_probe_releases_root_only_after_completion() {
    check("office", 9);
}
