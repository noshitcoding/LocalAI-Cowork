use cowork_broker::{FileBroker, RetainedContent};
use cowork_contracts::*;
use cowork_domain::{Error, hash};
use serde_json::json;

fn fixture() -> (
    tempfile::TempDir,
    FileBroker,
    RunRecord,
    Workspace,
    FileUpload,
) {
    let temp = tempfile::tempdir().unwrap();
    let broker = FileBroker::open(temp.path(), vec![]).unwrap();
    let order = Order {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: "project".into(),
        title: "Import fixture".into(),
        draft: String::new(),
        revision: 1,
        created_at: 0,
    };
    let workspace = broker.ensure_workspace(&order, None, None).unwrap();
    let run:RunRecord=serde_json::from_value(json!({"id":"run","spec":{"order_id":order.id,"project_id":order.project_id,"prompt":"Import a binary","provider":null,"access_mode":"confirm_mutations","executor":"local-v2","policy_revision":1,"allowed_tools":["import_file"],"budgets":{"max_steps":64,"max_seconds":1800,"max_output_tokens":4096},"folders":[workspace.folder]},"state":"running","revision":1,"last_sequence":1,"steps":0,"created_at":0,"updated_at":0,"started_at":null,"error_code":null,"usage":{}})).unwrap();
    let bytes = [0, 255, 128, 42];
    let upload = FileUpload {
        id: "upload".into(),
        order_id: order.id,
        project_id: order.project_id,
        file: DownloadMetadata {
            file_name: "Prüfung 📄.bin".into(),
            size: 4,
            sha256: hash(&bytes),
        },
        state: UploadState::Ready,
        revision: 2,
        created_at: 0,
    };
    (temp, broker, run, workspace, upload)
}
fn call(workspace: &Workspace, upload: &FileUpload, expected: Option<String>) -> ToolCall {
    ToolCall {
        id: "import".into(),
        name: "import_file".into(),
        arguments: json!(ImportFile {
            folder_id: workspace.folder.id.clone(),
            path: "Prüfung 📄.bin".into(),
            upload_id: upload.id.clone(),
            file: upload.file.clone(),
            expected_sha256: expected
        }),
    }
}

#[test]
fn binary_import_and_replacement_retain_provenance_and_restorable_previous_version() {
    let (temp, broker, run, workspace, upload) = fixture();
    let bytes = [0, 255, 128, 42];
    broker.retain_upload(&upload, &bytes).unwrap();
    assert!(
        broker
            .list_workspace(&workspace, ".")
            .unwrap()
            .entries
            .is_empty()
    );
    let first = broker
        .prepare_import(&run, &call(&workspace, &upload, None), &upload)
        .unwrap();
    let receipt = first.operation.clone();
    first.apply().unwrap();
    drop(first);
    assert_eq!(receipt.kind, FileOperationKind::Import);
    assert_eq!(receipt.imported_from.unwrap().id, upload.id);
    let path = temp
        .path()
        .join("workspaces")
        .join(&workspace.order_id)
        .join("files/Prüfung 📄.bin");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(matches!(
        broker.prepare_import(&run, &call(&workspace, &upload, None), &upload),
        Err(Error::FileConflict)
    ));
    let mut next = upload.clone();
    next.id = "replacement".into();
    next.file.size = 0;
    next.file.sha256 = hash(b"");
    broker.retain_upload(&next, b"").unwrap();
    let second = broker
        .prepare_import(
            &run,
            &call(&workspace, &next, Some(upload.file.sha256.clone())),
            &next,
        )
        .unwrap();
    let receipt = second.operation.clone();
    second.apply().unwrap();
    drop(second);
    assert_eq!(std::fs::read(&path).unwrap(), b"");
    assert_eq!(receipt.before.unwrap().sha256, upload.file.sha256);
    let restore = ToolCall {
        id: "restore".into(),
        name: "restore_file".into(),
        arguments: json!(RestoreFile {
            expected_source_sha256: None,
            folder_id: workspace.folder.id.clone(),
            path: "before.bin".into(),
            source: FileRestoreSource {
                run_id: run.id.clone(),
                tool_call_id: "import".into(),
                version: RetainedVersion::OperationBefore
            }
        }),
    };
    broker
        .prepare_restore(
            &run,
            &restore,
            &RetainedContent::File {
                sha256: upload.file.sha256,
            },
        )
        .unwrap()
        .apply()
        .unwrap();
    assert_eq!(
        std::fs::read(path.with_file_name("before.bin")).unwrap(),
        bytes
    );
}

#[test]
fn upload_integrity_scope_policy_and_stale_replacement_fail_before_publication() {
    let (temp, broker, run, workspace, upload) = fixture();
    for bytes in [&b"bad"[..], &b"nope"[..]] {
        assert!(broker.retain_upload(&upload, bytes).is_err());
    }
    assert!(broker.verify_upload(&upload).is_err());
    broker.retain_upload(&upload, &[0, 255, 128, 42]).unwrap();
    let tool = call(&workspace, &upload, None);
    let mut invalid = run.clone();
    invalid.spec.access_mode = AccessMode::ReadOnly;
    assert!(matches!(
        broker.prepare_import(&invalid, &tool, &upload),
        Err(Error::Forbidden)
    ));
    let mut foreign = upload.clone();
    foreign.order_id = "other".into();
    assert!(matches!(
        broker.prepare_import(&run, &tool, &foreign),
        Err(Error::Forbidden)
    ));
    let mut changed = tool.clone();
    changed.arguments["path"] = json!("../outside");
    assert!(broker.prepare_import(&run, &changed, &upload).is_err());
    let files = temp
        .path()
        .join("workspaces")
        .join(&workspace.order_id)
        .join("files");
    let pending = broker.prepare_import(&run, &tool, &upload).unwrap();
    let stage = std::fs::read_dir(&files)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".cowork-import-")
        })
        .unwrap();
    std::fs::write(stage, b"tampered").unwrap();
    assert!(matches!(pending.apply(), Err(Error::FileConflict)));
    drop(pending);
    assert!(!files.join("Prüfung 📄.bin").exists());
    let pending = broker.prepare_import(&run, &tool, &upload).unwrap();
    std::fs::write(files.join("Prüfung 📄.bin"), b"late external content").unwrap();
    assert!(matches!(pending.apply(), Err(Error::FileConflict)));
    drop(pending);
    assert_eq!(
        std::fs::read(files.join("Prüfung 📄.bin")).unwrap(),
        b"late external content"
    );
    std::fs::write(
        temp.path().join("versions").join(&upload.file.sha256),
        b"corrupt retained bytes",
    )
    .unwrap();
    assert!(
        broker
            .prepare_import(
                &run,
                &call(&workspace, &upload, Some(hash(b"late external content"))),
                &upload
            )
            .is_err()
    );
}

#[test]
fn import_reconciliation_checks_both_exact_states_including_same_content_replacement() {
    let (temp, broker, run, workspace, upload) = fixture();
    broker.retain_upload(&upload, &[0, 255, 128, 42]).unwrap();
    let pending = broker
        .prepare_import(&run, &call(&workspace, &upload, None), &upload)
        .unwrap();
    let receipt = pending.operation.clone();
    pending.apply().unwrap();
    drop(pending);
    assert!(broker.reconcile_operation(&run, &receipt, false).is_err());
    drop(broker.reconcile_operation(&run, &receipt, true).unwrap());
    let next = broker
        .prepare_import(
            &run,
            &call(&workspace, &upload, Some(upload.file.sha256.clone())),
            &upload,
        )
        .unwrap();
    let receipt = next.operation.clone();
    drop(next);
    drop(broker.reconcile_operation(&run, &receipt, false).unwrap());
    assert!(broker.reconcile_operation(&run, &receipt, true).is_err());
    let path = temp
        .path()
        .join("workspaces")
        .join(&workspace.order_id)
        .join("files/Prüfung 📄.bin");
    std::fs::write(path, b"unrelated edit").unwrap();
    assert!(broker.reconcile_operation(&run, &receipt, false).is_err());
    assert!(broker.reconcile_operation(&run, &receipt, true).is_err());
}
