use cowork_broker::{FileBroker, FolderConfig, RetainedContent, relative_path};
use cowork_contracts::*;
use cowork_domain::{Error, hash};
use serde_json::json;

fn fixture(writable: bool) -> (tempfile::TempDir, FileBroker, RunRecord) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("allowed")).unwrap();
    std::fs::create_dir(temp.path().join("outside")).unwrap();
    let config = FolderConfig {
        id: "folder".into(),
        project_id: "project".into(),
        name: "Explicit fixture".into(),
        path: temp.path().join("allowed"),
        writable,
    };
    let broker = FileBroker::open(&temp.path().join("broker"), vec![config]).unwrap();
    let run:RunRecord=serde_json::from_value(json!({"id":"run","spec":{"order_id":"order","project_id":"project","prompt":"x",
      "provider":{"id":"p","name":"p","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":8192,"credential_required":false,"credential_ready":true},
      "access_mode":"confirm_mutations","executor":"local-v2","policy_revision":1,"allowed_tools":["read_text_file","list_files","write_text_file"],
      "budgets":{"max_steps":64,"max_seconds":1800,"max_output_tokens":4096},"folders":broker.grants("project").unwrap()},
      "state":"running","revision":1,"last_sequence":1,"steps":0,"created_at":0,"updated_at":0,"started_at":null,"error_code":null,"usage":{}})).unwrap();
    (temp, broker, run)
}
fn location(path: &str) -> FileLocation {
    FileLocation {
        folder_id: "folder".into(),
        path: path.into(),
    }
}
fn write(path: &str, content: &str, expected: Option<&str>) -> ToolCall {
    ToolCall {
        id: "call".into(),
        name: "write_text_file".into(),
        arguments: json!({"folder_id":"folder","path":path,"content":content,"expected_sha256":expected}),
    }
}

#[test]
fn rejects_traversal_windows_aliases_and_hidden_broker_paths() {
    for path in [
        "",
        "/etc/passwd",
        "../secret",
        "a/../b",
        "a//b",
        "a\\b",
        "C:/secret",
        "C:secret",
        "//host/share",
        "file:stream",
        "NUL",
        "con.txt",
        "COM1.txt",
        "COM¹.txt",
        "a.",
        "a ",
        ".cowork-stage",
        ".COWORK-stage",
        "a/./b",
    ] {
        assert!(relative_path(path, false).is_err(), "accepted {path}");
    }
    assert!(relative_path(".", true).is_ok());
    assert!(relative_path("Ordner/Äpfel.txt", false).is_ok());
}

#[test]
fn grants_cannot_expose_or_modify_private_application_state() {
    let temp = tempfile::tempdir().unwrap();
    let private = temp.path().join("private");
    std::fs::create_dir_all(private.join("file-broker")).unwrap();
    for path in [
        temp.path().to_path_buf(),
        private.clone(),
        private.join("file-broker"),
    ] {
        let config = FolderConfig {
            id: "root".into(),
            project_id: "p".into(),
            name: "Unsafe grant".into(),
            path,
            writable: false,
        };
        assert!(
            FileBroker::open_with_protected(
                &private.join("file-broker"),
                vec![config],
                std::slice::from_ref(&private)
            )
            .is_err()
        );
    }
}

#[test]
fn exact_grants_readonly_and_expected_hash_are_enforced() {
    let (temp, broker, mut run) = fixture(true);
    let path = temp.path().join("allowed/note.txt");
    std::fs::write(&path, "old").unwrap();
    let original = broker.read(&run, &location("note.txt")).unwrap();
    assert_eq!(original.sha256, hash(b"old"));
    assert!(matches!(
        broker.prepare_write(&run, &write("note.txt", "bad", None)),
        Err(Error::FileConflict)
    ));
    run.spec.access_mode = AccessMode::ReadOnly;
    assert!(matches!(
        broker.prepare_write(&run, &write("note.txt", "bad", Some(&original.sha256))),
        Err(Error::Forbidden)
    ));
    run.spec.access_mode = AccessMode::ConfirmMutations;
    let mut prepared = broker
        .prepare_write(&run, &write("note.txt", "new", Some(&original.sha256)))
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "old"); // preparation is not the effect
    assert!(matches!(
        broker.read(&run, &location("note.txt")),
        Err(Error::WorkspaceBusy)
    ));
    prepared.apply().unwrap();
    drop(prepared);
    assert_eq!(
        broker.read(&run, &location("note.txt")).unwrap().content,
        "new"
    );
    assert_eq!(broker.read_version(&original.sha256).unwrap(), "old");
    assert_eq!(broker.read_version(&hash(b"new")).unwrap(), "new");
    run.spec.folders[0].revision = "stale".into();
    assert!(matches!(
        broker.read(&run, &location("note.txt")),
        Err(Error::Forbidden)
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
}

#[test]
fn narrowed_readonly_grant_keeps_reads_but_cannot_write_or_change_identity() {
    let (temp, broker, mut run) = fixture(true);
    std::fs::write(temp.path().join("allowed/AGENTS.md"), "Folder rules").unwrap();
    run.spec.folders[0].writable = false;
    assert_eq!(
        broker
            .read_context(&run.spec, &location("AGENTS.md"))
            .unwrap()
            .content,
        "Folder rules"
    );
    assert!(matches!(
        broker.prepare_write(&run, &write("new.txt", "no", None)),
        Err(Error::Forbidden)
    ));
    run.spec.folders[0].revision = "foreign".into();
    assert!(matches!(
        broker.read_context(&run.spec, &location("AGENTS.md")),
        Err(Error::Forbidden)
    ));
    run.spec.folders.clear();
    assert!(matches!(
        broker.read_context(&run.spec, &location("AGENTS.md")),
        Err(Error::Forbidden)
    ));
}

#[test]
fn creating_and_listing_files_does_not_grant_other_projects() {
    let (_temp, broker, mut run) = fixture(true);
    let mut prepared = broker
        .prepare_write(&run, &write("created.txt", "content", None))
        .unwrap();
    prepared.apply().unwrap();
    drop(prepared);
    let listing = broker.list(&run, &location(".")).unwrap();
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].name, "created.txt");
    run.spec.project_id = "other".into();
    assert!(matches!(
        broker.read(&run, &location("created.txt")),
        Err(Error::Forbidden)
    ));
}

#[test]
fn rejects_hard_links_and_external_changes_after_preparation() {
    let (temp, broker, run) = fixture(true);
    std::fs::write(temp.path().join("outside/secret.txt"), "outside").unwrap();
    std::fs::hard_link(
        temp.path().join("outside/secret.txt"),
        temp.path().join("allowed/link.txt"),
    )
    .unwrap();
    assert!(broker.read(&run, &location("link.txt")).is_err());
    assert!(
        broker
            .prepare_write(&run, &write("link.txt", "bad", Some(&hash(b"outside"))))
            .is_err()
    );
    let mut prepared = broker
        .prepare_write(&run, &write("new.txt", "proposed", None))
        .unwrap();
    std::fs::write(temp.path().join("allowed/new.txt"), "external").unwrap();
    assert!(matches!(prepared.apply(), Err(Error::FileConflict)));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("allowed/new.txt")).unwrap(),
        "external"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("outside/secret.txt")).unwrap(),
        "outside"
    );
}

#[test]
fn independent_brokers_share_os_writer_lock_and_readonly_grants() {
    let (temp, broker, run) = fixture(true);
    let config = FolderConfig {
        id: "folder".into(),
        project_id: "project".into(),
        name: "Explicit fixture".into(),
        path: temp.path().join("allowed"),
        writable: true,
    };
    let second = FileBroker::open(&temp.path().join("broker"), vec![config.clone()]).unwrap();
    let permit = broker
        .prepare_write(&run, &write("new.txt", "first", None))
        .unwrap();
    assert!(matches!(
        second.prepare_write(&run, &write("new.txt", "second", None)),
        Err(Error::WorkspaceBusy)
    ));
    drop(permit);
    assert!(
        second
            .prepare_write(&run, &write("new.txt", "second", None))
            .is_ok()
    );
    let readonly = FileBroker::open(
        &temp.path().join("broker"),
        vec![FolderConfig {
            writable: false,
            ..config
        }],
    )
    .unwrap();
    let mut run = run;
    run.spec.folders = readonly.grants("project").unwrap();
    assert!(matches!(
        readonly.prepare_write(&run, &write("new.txt", "bad", None)),
        Err(Error::Forbidden)
    ));
}

#[cfg(unix)]
#[test]
fn symlinks_and_special_files_cannot_escape_or_block() {
    use std::os::unix::fs::symlink;
    let (temp, broker, run) = fixture(true);
    std::fs::write(temp.path().join("outside/secret.txt"), "secret").unwrap();
    symlink(
        temp.path().join("outside"),
        temp.path().join("allowed/link"),
    )
    .unwrap();
    symlink(
        temp.path().join("outside/secret.txt"),
        temp.path().join("allowed/file.txt"),
    )
    .unwrap();
    assert!(broker.read(&run, &location("link/secret.txt")).is_err());
    assert!(broker.read(&run, &location("file.txt")).is_err());
    assert!(
        broker
            .prepare_write(&run, &write("link/new.txt", "bad", None))
            .is_err()
    );
    let fifo = std::ffi::CString::new(
        temp.path()
            .join("allowed/pipe")
            .as_os_str()
            .as_encoded_bytes(),
    )
    .unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(broker.read(&run, &location("pipe")).is_err());
    assert!(!temp.path().join("outside/new.txt").exists());
}

#[cfg(windows)]
#[test]
fn windows_junction_cannot_escape_the_granted_directory() {
    use std::os::windows::process::CommandExt;
    let (temp, broker, run) = fixture(true);
    std::fs::write(temp.path().join("outside/secret.txt"), "secret").unwrap();
    let link = temp.path().join("allowed/junction");
    let status=std::process::Command::new("powershell.exe").creation_flags(0x08000000)
        .args(["-NoProfile","-NonInteractive","-Command","$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:COWORK_TEST_LINK -Target $env:COWORK_TEST_TARGET | Out-Null"])
        .env("COWORK_TEST_LINK",&link).env("COWORK_TEST_TARGET",temp.path().join("outside")).status().unwrap();
    assert!(status.success(), "Could not create a test junction");
    let read = broker.read(&run, &location("junction/secret.txt"));
    let write = broker.prepare_write(&run, &write("junction/new.txt", "bad", None));
    std::fs::remove_dir(&link).unwrap(); // Remove only this junction, never recurse through its target.
    assert!(read.is_err());
    assert!(write.is_err());
    assert!(!temp.path().join("outside/new.txt").exists());
}

#[test]
fn directory_file_operations_retain_versions_and_reconcile_exact_state() {
    let (temp, broker, run) = fixture(true);
    let directory = ToolCall {
        id: "mkdir".into(),
        name: "create_directory".into(),
        arguments: json!({"folder_id":"folder","path":"documents"}),
    };
    let created = broker.prepare_operation(&run, &directory).unwrap();
    assert!(!temp.path().join("allowed/documents").exists());
    created.apply().unwrap();
    let receipt = created.operation.clone();
    drop(created);
    assert!(broker.reconcile_operation(&run, &receipt, true).is_ok());
    assert!(broker.reconcile_operation(&run, &receipt, false).is_err());
    let bytes = vec![255u8, 0, 128, 42];
    std::fs::write(temp.path().join("allowed/documents/input.bin"), &bytes).unwrap();
    let stat = broker
        .stat(&run, &location("documents"))
        .unwrap()
        .node
        .unwrap();
    let rename = ToolCall {
        id: "rename".into(),
        name: "rename_file".into(),
        arguments: json!({"folder_id":"folder","path":"documents","destination":"renamed","expected_sha256":stat.sha256}),
    };
    let prepared = broker.prepare_operation(&run, &rename).unwrap();
    assert_eq!(prepared.operation.entries.len(), 1);
    prepared.apply().unwrap();
    let receipt = prepared.operation.clone();
    drop(prepared);
    assert!(broker.reconcile_operation(&run, &receipt, true).is_ok());
    assert!(!temp.path().join("allowed/documents").exists());
    assert_eq!(
        std::fs::read(temp.path().join("allowed/renamed/input.bin")).unwrap(),
        bytes
    );
    let remove = ToolCall {
        id: "remove".into(),
        name: "remove_file".into(),
        arguments: json!({"folder_id":"folder","path":"renamed","expected_sha256":stat.sha256}),
    };
    let prepared = broker.prepare_operation(&run, &remove).unwrap();
    prepared.apply().unwrap();
    let receipt = prepared.operation.clone();
    drop(prepared);
    assert!(broker.reconcile_operation(&run, &receipt, true).is_ok());
    assert!(broker.reconcile_operation(&run, &receipt, false).is_err());
    assert!(
        broker
            .list(&run, &location("."))
            .unwrap()
            .entries
            .is_empty()
    );
    assert_eq!(
        std::fs::read(temp.path().join("broker/versions").join(hash(&bytes))).unwrap(),
        bytes
    );
}

#[test]
fn renaming_never_replaces_a_late_target_and_readonly_denies_mutations() {
    let (temp, broker, mut run) = fixture(true);
    std::fs::write(temp.path().join("allowed/source.txt"), "original").unwrap();
    let tool = ToolCall {
        id: "rename".into(),
        name: "rename_file".into(),
        arguments: json!({"folder_id":"folder","path":"source.txt","destination":"target.txt","expected_sha256":hash(b"original")}),
    };
    let prepared = broker.prepare_operation(&run, &tool).unwrap();
    std::fs::write(temp.path().join("allowed/target.txt"), "unrelated").unwrap();
    assert!(matches!(prepared.apply(), Err(Error::FileConflict)));
    drop(prepared);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("allowed/target.txt")).unwrap(),
        "unrelated"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("allowed/source.txt")).unwrap(),
        "original"
    );
    assert!(broker.prepare_operation(&run, &tool).is_err());
    run.spec.access_mode = AccessMode::ReadOnly;
    for name in ["create_directory", "remove_file", "rename_file"] {
        let mut action = tool.clone();
        action.name = name.into();
        action.arguments = match name {
            "create_directory" => json!({"folder_id":"folder","path":"new"}),
            "remove_file" => {
                json!({"folder_id":"folder","path":"source.txt","expected_sha256":hash(b"original")})
            }
            _ => action.arguments,
        };
        assert!(matches!(
            broker.prepare_operation(&run, &action),
            Err(Error::Forbidden)
        ));
    }
}

#[test]
fn removal_requires_the_full_current_tree_and_preserves_changed_sources() {
    let (temp, broker, run) = fixture(true);
    std::fs::create_dir(temp.path().join("allowed/tree")).unwrap();
    std::fs::write(temp.path().join("allowed/tree/a"), "one").unwrap();
    let before = broker.stat(&run, &location("tree")).unwrap().node.unwrap();
    let tool = ToolCall {
        id: "delete".into(),
        name: "remove_file".into(),
        arguments: json!({"folder_id":"folder","path":"tree","expected_sha256":before.sha256}),
    };
    let prepared = broker.prepare_operation(&run, &tool).unwrap();
    std::fs::write(temp.path().join("allowed/tree/a"), "two").unwrap();
    assert!(matches!(prepared.apply(), Err(Error::FileConflict)));
    drop(prepared);
    assert!(matches!(
        broker.prepare_operation(&run, &tool),
        Err(Error::FileConflict)
    ));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("allowed/tree/a")).unwrap(),
        "two"
    );
}

#[test]
fn retained_tree_restoration_uses_verified_independent_files_and_no_replace() {
    let (temp, broker, run) = fixture(true);
    let tree = temp.path().join("allowed/tree");
    std::fs::create_dir_all(tree.join("empty")).unwrap();
    let bytes = [0, 255, 19, 0, 42];
    std::fs::write(tree.join("binary.dat"), bytes).unwrap();
    let node = broker.stat(&run, &location("tree")).unwrap().node.unwrap();
    let remove = ToolCall {
        id: "remove".into(),
        name: "remove_file".into(),
        arguments: json!({"folder_id":"folder","path":"tree","expected_sha256":node.sha256}),
    };
    let removed = broker.prepare_operation(&run, &remove).unwrap();
    removed.apply().unwrap();
    let receipt = removed.operation.clone();
    drop(removed);
    let content = RetainedContent::Directory {
        sha256: node.sha256,
        entries: receipt.entries,
    };
    let tool = ToolCall {
        id: "restore".into(),
        name: "restore_file".into(),
        arguments: json!({"folder_id":"folder","path":"restored","source":{"run_id":"run","tool_call_id":"remove","version":"operation_before"}}),
    };
    let prepared = broker.prepare_restore(&run, &tool, &content).unwrap();
    let restored = prepared.operation.clone();
    assert!(!temp.path().join("allowed/restored").exists());
    prepared.apply().unwrap();
    drop(prepared);
    assert_eq!(
        std::fs::read(temp.path().join("allowed/restored/binary.dat")).unwrap(),
        bytes
    );
    assert!(temp.path().join("allowed/restored/empty").is_dir());
    assert_ne!(
        restored.after.as_ref().unwrap().resource_id,
        node.resource_id
    );
    assert_eq!(
        restored.restored_from.as_ref().unwrap().tool_call_id,
        "remove"
    );
    let guard = broker.reconcile_operation(&run, &restored, true).unwrap();
    drop(guard);
    assert!(matches!(
        broker.reconcile_operation(&run, &restored, false),
        Err(Error::FileConflict)
    ));
    assert!(matches!(
        broker.prepare_restore(&run, &tool, &content),
        Err(Error::FileConflict)
    ));
    let mut changed = tool.clone();
    changed.arguments["path"] = json!("late");
    let staged = broker.prepare_restore(&run, &changed, &content).unwrap();
    std::fs::write(temp.path().join("allowed/late"), "unrelated").unwrap();
    assert!(matches!(staged.apply(), Err(Error::FileConflict)));
    drop(staged);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("allowed/late")).unwrap(),
        "unrelated"
    );
    changed.arguments["path"] = json!("corrupt");
    std::fs::write(
        temp.path().join("broker/versions").join(hash(&bytes)),
        "damaged",
    )
    .unwrap();
    assert!(broker.prepare_restore(&run, &changed, &content).is_err());
    assert!(!temp.path().join("allowed/corrupt").exists());
}

#[test]
fn retained_restoration_rejects_forged_manifests_paths_and_readonly_grants() {
    let (_temp, broker, mut run) = fixture(true);
    let mut tool = ToolCall {
        id: "restore".into(),
        name: "restore_file".into(),
        arguments: json!({"folder_id":"folder","path":"new","source":{"run_id":"run","tool_call_id":"old","version":"operation_before"}}),
    };
    for path in [
        "../escape",
        "nested/without-parent",
        "dup",
        ".cowork-private",
    ] {
        let entries = vec![
            WorkspaceEntry {
                path: path.into(),
                sha256: None,
                size: 0,
            },
            WorkspaceEntry {
                path: path.into(),
                sha256: None,
                size: 0,
            },
        ];
        let content = RetainedContent::Directory {
            sha256: cowork_domain::canonical_hash(&entries),
            entries,
        };
        assert!(broker.prepare_restore(&run, &tool, &content).is_err());
    }
    let empty = RetainedContent::Directory {
        sha256: cowork_domain::canonical_hash(&Vec::<WorkspaceEntry>::new()),
        entries: vec![],
    };
    tool.arguments["path"] = json!("../escape");
    assert!(matches!(
        broker.prepare_restore(&run, &tool, &empty),
        Err(Error::Forbidden)
    ));
    tool.arguments["path"] = json!("new");
    run.spec.access_mode = AccessMode::ReadOnly;
    assert!(matches!(
        broker.prepare_restore(&run, &tool, &empty),
        Err(Error::Forbidden)
    ));
}
