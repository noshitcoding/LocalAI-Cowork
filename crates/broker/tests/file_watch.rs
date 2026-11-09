use cowork_broker::{FileBroker, NativeFolderSelection};
use cowork_contracts::*;
use cowork_domain::Error;

fn setup() -> (tempfile::TempDir, FileBroker, AutomationFileWatch) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("inbox");
    std::fs::create_dir(&root).unwrap();
    let broker = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
    let folder = broker
        .register_native_folder(&NativeFolderSelection {
            idempotency_key: "select".into(),
            project_id: "p".into(),
            name: "Inbox".into(),
            path: root.clone(),
            expected_resource_id: cowork_broker::inspect_native_folder(&root).unwrap().1,
            writable: true,
        })
        .unwrap()
        .grant;
    let mut readonly = folder;
    readonly.writable = false;
    let watch = AutomationFileWatch {
        folder: readonly,
        path: ".".into(),
        recursive: true,
        changes: vec![
            FileWatchChange::Created,
            FileWatchChange::Modified,
            FileWatchChange::Removed,
        ],
        extensions: vec![".pdf".into()],
        exclude_paths: vec!["processed".into()],
        include_existing: false,
        debounce_ms: 1000,
        poll_interval_ms: 5000,
        max_files: 100,
        max_file_bytes: 1024,
        max_scan_bytes: 4096,
    };
    (temp, broker, watch)
}

#[test]
fn scanner_hashes_only_selected_regular_files_and_excludes_output_paths() {
    let (temp, broker, watch) = setup();
    let root = temp.path().join("inbox");
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::create_dir(root.join("processed")).unwrap();
    for (path, content) in [
        ("a.pdf", "invoice"),
        ("sub/b.PDF", "other invoice"),
        ("notes.txt", "not selected"),
        ("processed/result.pdf", "output"),
        (".cowork-private.pdf", "private"),
    ] {
        std::fs::write(root.join(path), content).unwrap();
    }
    let scan = broker.prepare_file_watch_scan("p", &watch).unwrap();
    assert_eq!(
        scan.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec!["a.pdf", "sub/b.PDF"]
    );
    assert_eq!(scan.files[0].sha256, cowork_domain::hash(b"invoice"));
    assert_eq!(scan.files[0].size, 7);
    assert_eq!(scan.ignored_entries, 2);
}

#[test]
fn committed_revocation_cannot_overtake_an_uncommitted_scan() {
    let (temp, broker, watch) = setup();
    std::fs::write(temp.path().join("inbox/a.pdf"), "invoice").unwrap();
    let second = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
    let prepared = broker.prepare_file_watch_scan("p", &watch).unwrap();
    assert!(matches!(
        second.revoke_native_folder("p", &watch.folder.id, &watch.folder.revision),
        Err(Error::WorkspaceBusy)
    ));
    drop(prepared);
    second
        .revoke_native_folder("p", &watch.folder.id, &watch.folder.revision)
        .unwrap();
    assert!(matches!(
        broker.prepare_file_watch_scan("p", &watch),
        Err(Error::Forbidden)
    ));
}

#[test]
fn hardlinks_are_not_hashed_and_other_project_capabilities_are_rejected() {
    let (temp, broker, watch) = setup();
    let secret = temp.path().join("outside.pdf");
    std::fs::write(&secret, "outside").unwrap();
    std::fs::hard_link(&secret, temp.path().join("inbox/linked.pdf")).unwrap();
    let scan = broker.prepare_file_watch_scan("p", &watch).unwrap();
    assert!(scan.files.is_empty());
    assert_eq!(scan.ignored_entries, 1);
    assert_eq!(scan.unavailable_paths, vec!["linked.pdf"]);
    assert!(matches!(
        broker.prepare_file_watch_scan("other", &watch),
        Err(Error::Forbidden)
    ));
    assert_eq!(std::fs::read(&secret).unwrap(), b"outside");
}

#[test]
fn incomplete_scans_are_errors_instead_of_partial_snapshots() {
    let (temp, broker, mut watch) = setup();
    let root = temp.path().join("inbox");
    std::fs::write(root.join("a.pdf"), "123456").unwrap();
    std::fs::write(root.join("b.pdf"), "123456").unwrap();
    watch.max_file_bytes = 8;
    watch.max_scan_bytes = 10;
    assert!(
        matches!(broker.prepare_file_watch_scan("p",&watch),Err(Error::Validation(reason)) if reason=="automation_watch_byte_limit")
    );
    watch.max_scan_bytes = 32;
    watch.max_files = 1;
    assert!(
        matches!(broker.prepare_file_watch_scan("p",&watch),Err(Error::Validation(reason)) if reason=="automation_watch_file_limit")
    );
    watch.max_files = 100;
    watch.max_file_bytes = 4;
    assert!(
        matches!(broker.prepare_file_watch_scan("p",&watch),Err(Error::Validation(reason)) if reason=="automation_watch_file_too_large")
    );
}

#[cfg(unix)]
#[test]
fn symbolic_links_and_linked_directories_never_expand_the_scan_scope() {
    let (temp, broker, watch) = setup();
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret.pdf"), "secret").unwrap();
    std::os::unix::fs::symlink(&outside, temp.path().join("inbox/linked-dir")).unwrap();
    std::os::unix::fs::symlink(
        outside.join("secret.pdf"),
        temp.path().join("inbox/linked-file.pdf"),
    )
    .unwrap();
    let scan = broker.prepare_file_watch_scan("p", &watch).unwrap();
    assert!(scan.files.is_empty());
    assert_eq!(scan.ignored_entries, 2);
    let mut unavailable = scan.unavailable_paths.clone();
    unavailable.sort();
    assert_eq!(unavailable, vec!["linked-dir", "linked-file.pdf"]);
}

#[cfg(windows)]
#[test]
fn windows_directory_junctions_never_expand_the_scan_scope() {
    let (temp, broker, watch) = setup();
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret.pdf"), "outside").unwrap();
    let link = temp.path().join("inbox").join("junction");
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "Creating the owned junction fixture failed: link={link:?}; target={outside:?}; stdout={}; stderr={}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );
    let scan = broker.prepare_file_watch_scan("p", &watch).unwrap();
    assert!(scan.files.is_empty());
    assert_eq!(scan.ignored_entries, 1);
    assert_eq!(scan.unavailable_paths, vec!["junction"]);
    assert_eq!(
        std::fs::read(outside.join("secret.pdf")).unwrap(),
        b"outside"
    );
}
