use cowork_broker::FileBroker;
use cowork_contracts::*;
use cowork_domain::{Error, hash, workspace_snapshot_hash};
use serde_json::json;

fn order() -> Order {
    Order {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: "p".into(),
        title: "Workspace fixture".into(),
        draft: String::new(),
        revision: 1,
        created_at: 0,
    }
}
fn run(order: &Order, workspace: &Workspace) -> RunRecord {
    serde_json::from_value(json!({"id":uuid::Uuid::new_v4().to_string(),"spec":{"order_id":order.id,"project_id":"p","prompt":"x",
        "provider":{"id":"p","name":"p","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":8192,"credential_required":false,"credential_ready":true},
        "access_mode":"confirm_mutations","executor":"local-v2","policy_revision":1,"allowed_tools":["write_text_file"],"budgets":{"max_steps":64,"max_seconds":1800,"max_output_tokens":4096},"folders":[workspace.folder]},
        "state":"running","revision":1,"last_sequence":1,"steps":0,"created_at":0,"updated_at":0,"started_at":null,"error_code":null,"usage":{}})).unwrap()
}

#[test]
fn persistent_identity_binary_snapshot_and_independent_branch() {
    let temp = tempfile::tempdir().unwrap();
    let source = order();
    let target = order();
    let broker = FileBroker::open(temp.path(), vec![]).unwrap();
    let workspace = broker.ensure_workspace(&source, None, None).unwrap();
    assert!(broker.grants("p").unwrap().is_empty());
    assert!(matches!(
        broker.validate_grants("p", std::slice::from_ref(&workspace.folder)),
        Err(Error::Forbidden)
    ));
    let files = temp
        .path()
        .join("workspaces")
        .join(&source.id)
        .join("files");
    std::fs::create_dir(files.join("nested")).unwrap();
    let bytes = vec![0u8, 255, 128, 42];
    std::fs::write(files.join("nested/blob.bin"), &bytes).unwrap();
    std::fs::write(files.join("note.txt"), b"first").unwrap();
    let record = run(&source, &workspace);
    let prepared = broker.snapshot_workspace(&record).unwrap();
    assert_eq!(prepared.snapshot.entries.len(), 3);
    assert_eq!(prepared.snapshot.entries[1].sha256, Some(hash(&bytes)));
    let snapshot = prepared.snapshot.clone();
    drop(prepared);
    std::fs::write(files.join("note.txt"), b"later").unwrap();
    let branch = broker
        .ensure_workspace(&target, Some(&snapshot), None)
        .unwrap();
    assert_eq!(
        broker.read_workspace(&branch, "note.txt").unwrap().content,
        "first"
    );
    let branch_files = temp
        .path()
        .join("workspaces")
        .join(&target.id)
        .join("files");
    assert_eq!(
        std::fs::read(branch_files.join("nested/blob.bin")).unwrap(),
        bytes
    );
    std::fs::write(branch_files.join("note.txt"), b"independent").unwrap();
    assert_eq!(std::fs::read(files.join("note.txt")).unwrap(), b"later");
    let mut foreign = record.clone();
    foreign.spec.order_id = target.id.clone();
    assert!(matches!(
        broker.list(
            &foreign,
            &FileLocation {
                folder_id: workspace.folder.id.clone(),
                path: ".".into()
            }
        ),
        Err(Error::Forbidden)
    ));
    drop(broker);
    let reopened = FileBroker::open(temp.path(), vec![]).unwrap();
    assert_eq!(
        reopened
            .ensure_workspace(&source, None, Some(&workspace))
            .unwrap(),
        workspace
    );
    assert_eq!(
        reopened
            .ensure_workspace(&target, Some(&snapshot), Some(&branch))
            .unwrap(),
        branch
    );
    assert_eq!(
        reopened
            .read_workspace(&branch, "note.txt")
            .unwrap()
            .content,
        "independent"
    );
}

#[test]
fn corrupted_missing_or_unsafe_snapshots_never_publish_a_branch() {
    let temp = tempfile::tempdir().unwrap();
    let source = order();
    let broker = FileBroker::open(temp.path(), vec![]).unwrap();
    let workspace = broker.ensure_workspace(&source, None, None).unwrap();
    let path = temp
        .path()
        .join("workspaces")
        .join(&source.id)
        .join("files/note.txt");
    std::fs::write(&path, b"saved").unwrap();
    let snapshot = broker
        .snapshot_workspace(&run(&source, &workspace))
        .unwrap()
        .snapshot
        .clone();
    std::fs::write(
        temp.path().join("versions").join(hash(b"saved")),
        b"corrupt",
    )
    .unwrap();
    let target = order();
    assert!(
        broker
            .ensure_workspace(&target, Some(&snapshot), None)
            .is_err()
    );
    assert!(!temp.path().join("workspaces").join(&target.id).exists());
    let mut unsafe_snapshot = snapshot.clone();
    unsafe_snapshot.entries[0].path = "../escape".into();
    unsafe_snapshot.sha256 = workspace_snapshot_hash(&unsafe_snapshot);
    assert!(
        broker
            .ensure_workspace(&order(), Some(&unsafe_snapshot), None)
            .is_err()
    );
    std::fs::hard_link(&path, path.with_file_name("alias.txt")).unwrap();
    assert!(
        broker
            .snapshot_workspace(&run(&source, &workspace))
            .is_err()
    );
    drop(broker);
    std::fs::rename(
        temp.path().join("workspaces").join(&source.id),
        temp.path().join("removed-workspace"),
    )
    .unwrap();
    let reopened = FileBroker::open(temp.path(), vec![]).unwrap();
    assert!(
        reopened
            .ensure_workspace(&source, None, Some(&workspace))
            .is_err()
    );
    assert!(!temp.path().join("workspaces").join(&source.id).exists());
}

#[test]
fn downloads_are_independent_bytes_with_verified_historical_sources() {
    let temp = tempfile::tempdir().unwrap();
    let source = order();
    let broker = FileBroker::open(temp.path(), vec![]).unwrap();
    let workspace = broker.ensure_workspace(&source, None, None).unwrap();
    let files = temp
        .path()
        .join("workspaces")
        .join(&source.id)
        .join("files");
    let original = [0, 255, 128, 42];
    std::fs::write(files.join("Prüfung.bin"), original).unwrap();
    let input = DownloadFile {
        path: "Prüfung.bin".into(),
        expected_sha256: Some(hash(&original)),
    };
    let download = broker.download_workspace(&workspace, &input).unwrap();
    assert_eq!(download.bytes, original);
    assert_eq!(download.metadata.file_name, "Prüfung.bin");
    assert_eq!(download.metadata.size, 4);
    let record = run(&source, &workspace);
    let snapshot = broker.snapshot_workspace(&record).unwrap().snapshot.clone();
    std::fs::write(files.join("Prüfung.bin"), b"changed").unwrap();
    assert!(matches!(
        broker.download_workspace(&workspace, &input),
        Err(Error::FileConflict)
    ));
    assert_eq!(
        broker.download_snapshot(&snapshot, &input).unwrap().bytes,
        original
    );
    std::fs::remove_file(files.join("Prüfung.bin")).unwrap();
    assert_eq!(
        broker.download_snapshot(&snapshot, &input).unwrap().bytes,
        original
    );
    let mut forged = snapshot.clone();
    forged.entries[0].size += 1;
    forged.sha256 = workspace_snapshot_hash(&forged);
    assert!(broker.download_snapshot(&forged, &input).is_err());
    std::fs::write(temp.path().join("versions").join(hash(&original)), b"bad").unwrap();
    assert!(broker.download_snapshot(&snapshot, &input).is_err());
}

#[test]
fn downloads_reject_scope_escapes_links_large_files_and_active_writers() {
    let temp = tempfile::tempdir().unwrap();
    let source = order();
    let broker = FileBroker::open(temp.path(), vec![]).unwrap();
    let workspace = broker.ensure_workspace(&source, None, None).unwrap();
    let files = temp
        .path()
        .join("workspaces")
        .join(&source.id)
        .join("files");
    std::fs::write(files.join("note.txt"), b"first").unwrap();
    let input = |path: &str| DownloadFile {
        path: path.into(),
        expected_sha256: None,
    };
    for path in [
        "../workspace.json",
        "C:/Windows/file",
        ".cowork-private",
        "note.txt:stream",
        ".",
        "missing",
    ] {
        assert!(
            broker.download_workspace(&workspace, &input(path)).is_err(),
            "{path}"
        );
    }
    let mut foreign = workspace.clone();
    foreign.order_id = order().id;
    assert!(matches!(
        broker.download_workspace(&foreign, &input("note.txt")),
        Err(Error::Forbidden)
    ));
    let mut record = run(&source, &workspace);
    record.spec.access_mode = AccessMode::Autonomous;
    let write = broker.prepare_write(&record, &ToolCall {id:"writer".into(), name:"write_text_file".into(), arguments:json!({"folder_id":workspace.folder.id,"path":"note.txt","content":"later","expected_sha256":hash(b"first")})}).unwrap();
    assert!(matches!(
        broker.download_workspace(&workspace, &input("note.txt")),
        Err(Error::WorkspaceBusy)
    ));
    drop(write);
    std::fs::hard_link(files.join("note.txt"), files.join("alias.txt")).unwrap();
    assert!(matches!(
        broker.download_workspace(&workspace, &input("alias.txt")),
        Err(Error::Forbidden)
    ));
    let large = std::fs::File::create(files.join("large.bin")).unwrap();
    large.set_len(MAX_DOWNLOAD_BYTES + 1).unwrap();
    drop(large);
    assert!(matches!(
        broker.download_workspace(&workspace, &input("large.bin")),
        Err(Error::Forbidden)
    ));
    std::fs::write(files.join("empty.bin"), []).unwrap();
    assert!(
        broker
            .download_workspace(&workspace, &input("empty.bin"))
            .unwrap()
            .bytes
            .is_empty()
    );
}
