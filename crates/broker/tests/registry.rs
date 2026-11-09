use cowork_broker::{FileBroker, FolderConfig, NativeFolderSelection};
use cowork_contracts::*;
use cowork_domain::{Error, hash};
use serde_json::json;
use std::path::Path;

fn selection(root: &Path) -> NativeFolderSelection {
    NativeFolderSelection {
        idempotency_key: "choose-once".into(),
        project_id: "p".into(),
        name: "Explicit working folder".into(),
        path: root.join("chosen"),
        expected_resource_id: cowork_broker::inspect_native_folder(&root.join("chosen"))
            .unwrap()
            .1,
        writable: true,
    }
}
fn setup() -> (tempfile::TempDir, FileBroker) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("chosen")).unwrap();
    std::fs::write(temp.path().join("chosen/original.txt"), "original").unwrap();
    let broker = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
    (temp, broker)
}
fn run(grant: FolderGrant) -> RunRecord {
    serde_json::from_value(json!({"id":"run","spec":{"order_id":"order","project_id":"p","prompt":"test","provider":null,
        "access_mode":"confirm_mutations","executor":"local-v2","policy_revision":1,"allowed_tools":["read_text_file","write_text_file"],
        "budgets":{"max_steps":64,"max_seconds":1800,"max_output_tokens":4096},"folders":[grant]},
        "state":"running","revision":1,"last_sequence":1,"steps":0,"created_at":0,"updated_at":0,"started_at":null,"error_code":null,"usage":{}})).unwrap()
}
fn location(run: &RunRecord) -> FileLocation {
    FileLocation {
        folder_id: run.spec.folders[0].id.clone(),
        path: "original.txt".into(),
    }
}
fn write(run: &RunRecord) -> ToolCall {
    ToolCall {
        id: "write".into(),
        name: "write_text_file".into(),
        arguments: json!({"folder_id":run.spec.folders[0].id,"path":"original.txt","content":"changed","expected_sha256":hash(b"original")}),
    }
}

#[test]
fn registration_is_durable_idempotent_scoped_and_revocable_without_restart() {
    let (temp, first) = setup();
    let second = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
    let input = selection(temp.path());
    let registered = first.register_native_folder(&input).unwrap();
    assert!(!registered.revoked);
    assert!(registered.canonical_path.is_absolute());
    assert_eq!(
        first.register_native_folder(&input).unwrap().grant,
        registered.grant
    );
    assert_eq!(second.grants("p").unwrap(), vec![registered.grant.clone()]);
    assert!(second.grants("other").unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(temp.path().join("chosen"))
            .unwrap()
            .count(),
        1
    );
    let mut conflicting = input.clone();
    conflicting.writable = false;
    let conflict = first.register_native_folder(&conflicting);
    assert!(matches!(conflict, Err(Error::Conflict)), "{conflict:?}");
    let run = run(registered.grant.clone());
    assert_eq!(
        second.read(&run, &location(&run)).unwrap().content,
        "original"
    );
    assert!(matches!(
        first.revoke_native_folder("other", &registered.grant.id, &registered.grant.revision),
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        first.revoke_native_folder("p", &registered.grant.id, "stale"),
        Err(Error::Conflict)
    ));
    first
        .revoke_native_folder("p", &registered.grant.id, &registered.grant.revision)
        .unwrap();
    assert!(second.grants("p").unwrap().is_empty());
    assert!(matches!(
        second.read(&run, &location(&run)),
        Err(Error::Forbidden)
    ));
    assert!(second.validate_grants("p", &run.spec.folders).is_err());
    let reopened = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
    assert!(
        reopened
            .registration_for_request("p", "choose-once")
            .unwrap()
            .unwrap()
            .revoked
    );
    assert!(reopened.register_native_folder(&input).unwrap().revoked);
    reopened
        .revoke_native_folder("p", &registered.grant.id, &registered.grant.revision)
        .unwrap();
    assert!(reopened.grants("p").unwrap().is_empty());
    let new = reopened
        .register_native_folder(&NativeFolderSelection {
            idempotency_key: "new-human-selection".into(),
            writable: false,
            ..input
        })
        .unwrap();
    assert_ne!(new.grant.id, registered.grant.id);
    assert!(reopened.read(&run, &location(&run)).is_err());
    let readonly = self::run(new.grant);
    assert!(reopened.read(&readonly, &location(&readonly)).is_ok());
    assert!(matches!(
        reopened.prepare_write(&readonly, &write(&readonly)),
        Err(Error::Forbidden)
    ));
}

#[test]
fn revocation_waits_for_native_io_but_preserves_exact_receipt_reconciliation() {
    let (temp, first) = setup();
    let second = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
    let grant = first
        .register_native_folder(&selection(temp.path()))
        .unwrap()
        .grant;
    let run = run(grant.clone());
    let mut prepared = first.prepare_write(&run, &write(&run)).unwrap();
    assert!(matches!(
        second.revoke_native_folder("p", &grant.id, &grant.revision),
        Err(Error::WorkspaceBusy)
    ));
    prepared.apply().unwrap();
    let receipt = prepared.change.clone();
    drop(prepared);
    second
        .revoke_native_folder("p", &grant.id, &grant.revision)
        .unwrap();
    assert!(first.prepare_write(&run, &write(&run)).is_err());
    assert!(first.read(&run, &location(&run)).is_err());
    let (actual, lock) = first.observe_change(&run, &receipt).unwrap();
    assert_eq!(actual, Some(hash(b"changed")));
    drop(lock);
    let mut foreign = run.clone();
    foreign.spec.project_id = "foreign".into();
    assert!(first.observe_change(&foreign, &receipt).is_err());
    foreign = run.clone();
    foreign.spec.folders.clear();
    assert!(first.observe_change(&foreign, &receipt).is_err());
    assert_eq!(
        std::fs::read(temp.path().join("chosen/original.txt")).unwrap(),
        b"changed"
    );
}

#[test]
fn paths_cannot_overlap_private_or_other_active_roots_and_identity_cannot_silently_change() {
    let (temp, broker) = setup();
    let input = selection(temp.path());
    for path in [
        temp.path().to_path_buf(),
        temp.path().join("state"),
        temp.path().join("state/versions"),
        "relative".into(),
    ] {
        assert!(
            broker
                .register_native_folder(&NativeFolderSelection {
                    path,
                    ..input.clone()
                })
                .is_err()
        );
    }
    let grant = broker.register_native_folder(&input).unwrap().grant;
    std::fs::create_dir(temp.path().join("chosen/child")).unwrap();
    assert!(
        broker
            .register_native_folder(&NativeFolderSelection {
                idempotency_key: "nested".into(),
                path: temp.path().join("chosen/child"),
                ..input.clone()
            })
            .is_err()
    );
    assert!(
        broker
            .register_native_folder(&NativeFolderSelection {
                idempotency_key: "other-project".into(),
                project_id: "other".into(),
                ..input.clone()
            })
            .is_err()
    );
    let conflicting_config = FolderConfig {
        id: "config".into(),
        project_id: "p".into(),
        name: "Overlapping config".into(),
        path: input.path.clone(),
        writable: true,
    };
    assert!(FileBroker::open(&temp.path().join("state"), vec![conflicting_config]).is_err());
    // Every dynamic access reopens and checks the pinned physical directory, not just its path.
    let run = run(grant.clone());
    std::fs::rename(&input.path, temp.path().join("moved-original")).unwrap();
    std::fs::create_dir(&input.path).unwrap();
    std::fs::write(input.path.join("original.txt"), "other directory").unwrap();
    assert!(matches!(
        broker.read(&run, &location(&run)),
        Err(Error::Forbidden)
    ));
    // Revocation does not need to access a missing or replaced resource.
    broker
        .revoke_native_folder("p", &grant.id, &grant.revision)
        .unwrap();
    assert_eq!(
        std::fs::read(input.path.join("original.txt")).unwrap(),
        b"other directory"
    );
}

#[test]
fn configured_grant_revocation_survives_configuration_reopen_and_corruption_fails_closed() {
    let (temp, initial) = setup();
    drop(initial);
    let config = FolderConfig {
        id: "configured".into(),
        project_id: "p".into(),
        name: "Configured fixture".into(),
        path: temp.path().join("chosen"),
        writable: true,
    };
    let broker = FileBroker::open(&temp.path().join("state"), vec![config.clone()]).unwrap();
    let grant = broker.grants("p").unwrap().remove(0);
    broker
        .revoke_native_folder("p", &grant.id, &grant.revision)
        .unwrap();
    let reopened = FileBroker::open(&temp.path().join("state"), vec![config.clone()]).unwrap();
    assert!(reopened.grants("p").unwrap().is_empty());
    let run = run(grant);
    assert!(reopened.read(&run, &location(&run)).is_err());
    std::fs::write(temp.path().join("state/folder-registry.json"), b"{broken").unwrap();
    assert!(reopened.grants("p").is_err());
    assert!(reopened.read(&run, &location(&run)).is_err());
    assert!(FileBroker::open(&temp.path().join("state"), vec![config]).is_err());
}

#[test]
fn concurrent_registrations_do_not_lose_updates() {
    let (temp, initial) = setup();
    drop(initial);
    for i in 0..8 {
        std::fs::create_dir(temp.path().join(format!("folder-{i}"))).unwrap();
    }
    std::thread::scope(|scope| {
        for i in 0..8 {
            let root = temp.path();
            scope.spawn(move || {
                let broker = FileBroker::open(&root.join("state"), vec![]).unwrap();
                let input = NativeFolderSelection {
                    idempotency_key: format!("select-{i}"),
                    project_id: "p".into(),
                    name: format!("Folder {i}"),
                    path: root.join(format!("folder-{i}")),
                    expected_resource_id: cowork_broker::inspect_native_folder(
                        &root.join(format!("folder-{i}")),
                    )
                    .unwrap()
                    .1,
                    writable: false,
                };
                for _ in 0..100 {
                    match broker.register_native_folder(&input) {
                        Ok(_) => return,
                        Err(Error::WorkspaceBusy) => {
                            std::thread::sleep(std::time::Duration::from_millis(5))
                        }
                        Err(error) => panic!("Unexpected registration error: {error}"),
                    }
                }
                panic!("Registration lock never became available");
            });
        }
    });
    let reopened = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
    assert_eq!(reopened.grants("p").unwrap().len(), 8);
}

#[test]
fn killed_native_writer_releases_revocation_fence_and_new_process_observes_tombstone() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let (temp, broker) = setup();
    let grant = broker
        .register_native_folder(&selection(temp.path()))
        .unwrap()
        .grant;
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "registry_process_child",
            "--nocapture",
        ])
        .env("COWORK_REGISTRY_FIXTURE", temp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "Child did not acquire writer lock"
        );
        if line.contains("REGISTRY_LOCK_HELD") {
            break;
        }
    }
    let blocked = broker.revoke_native_folder("p", &grant.id, &grant.revision);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(matches!(blocked, Err(Error::WorkspaceBusy)));
    broker
        .revoke_native_folder("p", &grant.id, &grant.revision)
        .unwrap();
    let mut reader = Command::new(std::env::current_exe().unwrap());
    reader
        .args([
            "--ignored",
            "--exact",
            "registry_process_child",
            "--nocapture",
        ])
        .env("COWORK_REGISTRY_FIXTURE", temp.path())
        .env("COWORK_REGISTRY_READ_REVOKED", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        reader.creation_flags(0x08000000);
    }
    let result = reader.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("REGISTRY_REVOKED"));
}

#[test]
#[ignore = "Invoked only by the parent process test with an isolated temporary directory"]
fn registry_process_child() {
    use std::io::Write;
    let root = std::path::PathBuf::from(
        std::env::var_os("COWORK_REGISTRY_FIXTURE").expect("parent fixture"),
    );
    let broker = FileBroker::open(&root.join("state"), vec![]).unwrap();
    let record = broker
        .registration_for_request("p", "choose-once")
        .unwrap()
        .unwrap();
    let run = run(record.grant);
    if std::env::var_os("COWORK_REGISTRY_READ_REVOKED").is_some() {
        assert!(record.revoked);
        assert!(broker.read(&run, &location(&run)).is_err());
        println!("REGISTRY_REVOKED");
        return;
    }
    let _prepared = broker.prepare_write(&run, &write(&run)).unwrap();
    println!("REGISTRY_LOCK_HELD");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
}
