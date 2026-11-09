use cap_std::{ambient_authority, fs::Dir};
use cowork_broker::{
    FileBroker, FolderConfig,
    workspace_receive::{TransferBinding, WorkspaceReceiver},
};
use cowork_contracts::*;
use cowork_domain::{Error, hash};

fn fixture() -> (tempfile::TempDir, FileBroker, Order, PrepareExecutionInput) {
    let temp = tempfile::tempdir().unwrap();
    for name in ["chosen", "foreign"] {
        std::fs::create_dir(temp.path().join(name)).unwrap();
    }
    std::fs::write(temp.path().join("chosen/proof.bin"), b"before").unwrap();
    std::fs::create_dir(temp.path().join("chosen/empty")).unwrap();
    std::fs::write(temp.path().join("foreign/secret"), b"foreign content").unwrap();
    let configs = vec![
        FolderConfig {
            id: "selected".into(),
            project_id: "p".into(),
            name: "Selected folder".into(),
            path: temp.path().join("chosen"),
            writable: true,
        },
        FolderConfig {
            id: "other".into(),
            project_id: "q".into(),
            name: "Other project".into(),
            path: temp.path().join("foreign"),
            writable: true,
        },
    ];
    let broker = FileBroker::open(&temp.path().join("state"), configs).unwrap();
    let order = Order {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: "p".into(),
        title: "Frozen input".into(),
        draft: String::new(),
        revision: 1,
        created_at: 1,
    };
    let request = PrepareExecutionInput {
        idempotency_key: "selected-once".into(),
        base_revision: 1,
        folder_ids: vec!["selected".into()],
    };
    (temp, broker, order, request)
}

#[test]
fn local_input_retains_private_origin_and_frozen_bytes_then_materializes_on_another_root() {
    let (temp, broker, order, request) = fixture();
    let prepared = broker
        .prepare_execution_input(&order, &request, &uuid::Uuid::new_v4().to_string(), 1)
        .unwrap();
    prepared.capture.validate_request(&request).unwrap();
    let view = serde_json::to_string(&prepared.capture.snapshot).unwrap();
    assert!(
        !view.contains("resource_id")
            && !view.contains("revision\":\"")
            && !view.contains("selected\"")
            && !view.contains(&temp.path().display().to_string())
    );
    assert_eq!(prepared.capture.origins[0].id, "selected");
    assert_eq!(prepared.capture.snapshot.sources[0].alias, "input-0001");
    assert_eq!(
        prepared.capture.snapshot.manifest.files[0].path,
        "input-0001/proof.bin"
    );
    let origin = prepared.capture.origins[0].clone();
    assert!(matches!(
        broker.revoke_native_folder("p", &origin.id, &origin.revision),
        Err(Error::WorkspaceBusy)
    ));
    let capture = prepared.capture.clone();
    drop(prepared);
    std::fs::write(temp.path().join("chosen/proof.bin"), b"changed original").unwrap();
    let stage = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let receiver = WorkspaceReceiver::create(
        Dir::open_ambient_dir(stage.path(), ambient_authority()).unwrap(),
        TransferBinding {
            plan_id: "explicit-fixture-authority".into(),
            plan_sha256: hash(b"fixture-authority"),
            recipient_id: "other-machine".into(),
            recipient_generation: 1,
        },
        capture.snapshot.manifest.clone(),
    )
    .unwrap();
    for chunk in receiver.missing_chunks().unwrap() {
        let bytes = broker.execution_input_chunk(&capture, &chunk).unwrap();
        receiver.accept_chunk(&chunk, &bytes.bytes).unwrap();
    }
    let completed = receiver
        .verify()
        .unwrap()
        .materialize(Dir::open_ambient_dir(work.path(), ambient_authority()).unwrap())
        .unwrap();
    assert_eq!(
        completed.manifest_sha256(),
        capture.snapshot.manifest.sha256
    );
    assert_eq!(
        std::fs::read(work.path().join("input-0001/proof.bin")).unwrap(),
        b"before"
    );
    assert_eq!(
        std::fs::read(temp.path().join("chosen/proof.bin")).unwrap(),
        b"changed original"
    );
    let chunk = &capture.snapshot.manifest.files[0].chunks[0];
    let read = ReadWorkspaceTransferChunk {
        path: "input-0001/proof.bin".into(),
        offset: 0,
        expected_sha256: chunk.sha256.clone(),
    };
    broker
        .revoke_native_folder("p", &origin.id, &origin.revision)
        .unwrap();
    assert!(broker.execution_input_chunk(&capture, &read).is_err());
}

#[test]
fn foreign_duplicate_stale_and_unselected_sources_never_create_a_capture() {
    let (_temp, broker, order, request) = fixture();
    for folder_ids in [
        vec!["other".into()],
        vec!["selected".into(), "selected".into()],
        vec!["missing".into()],
    ] {
        assert!(
            broker
                .prepare_execution_input(
                    &order,
                    &PrepareExecutionInput {
                        folder_ids,
                        ..request.clone()
                    },
                    &uuid::Uuid::new_v4().to_string(),
                    1
                )
                .is_err()
        );
    }
    assert!(
        broker
            .prepare_execution_input(
                &order,
                &PrepareExecutionInput {
                    base_revision: 2,
                    ..request.clone()
                },
                &uuid::Uuid::new_v4().to_string(),
                1
            )
            .is_err()
    );
    let prepared = broker
        .prepare_execution_input(&order, &request, &uuid::Uuid::new_v4().to_string(), 1)
        .unwrap();
    let unselected = ReadWorkspaceTransferChunk {
        path: "input-0002/secret".into(),
        offset: 0,
        expected_sha256: hash(b"foreign content"),
    };
    assert!(
        broker
            .execution_input_chunk(&prepared.capture, &unselected)
            .is_err()
    );
    let invalid_offset = ReadWorkspaceTransferChunk {
        path: "input-0001/proof.bin".into(),
        offset: 1,
        expected_sha256: hash(b"before"),
    };
    assert!(
        broker
            .execution_input_chunk(&prepared.capture, &invalid_offset)
            .is_err()
    );
}

#[test]
fn all_selected_folders_share_one_capture_and_managed_workspaces_keep_their_order_scope() {
    let (temp, broker, order, request) = fixture();
    let workspace = broker.ensure_workspace(&order, None, None).unwrap();
    std::fs::write(
        temp.path()
            .join("state/workspaces")
            .join(&order.id)
            .join("files/managed.txt"),
        b"managed",
    )
    .unwrap();
    let mut selected = vec![workspace.folder.id.clone(), "selected".into()];
    selected.sort();
    let prepared = broker
        .prepare_execution_input(
            &order,
            &PrepareExecutionInput {
                folder_ids: selected,
                ..request.clone()
            },
            &uuid::Uuid::new_v4().to_string(),
            1,
        )
        .unwrap();
    assert_eq!(prepared.capture.origins.len(), 2);
    assert_eq!(prepared.capture.snapshot.manifest.total_bytes, 13);
    let wrong_order = Order {
        id: uuid::Uuid::new_v4().to_string(),
        ..order
    };
    assert!(
        broker
            .prepare_execution_input(
                &wrong_order,
                &PrepareExecutionInput {
                    folder_ids: vec![workspace.folder.id],
                    ..request.clone()
                },
                &uuid::Uuid::new_v4().to_string(),
                1
            )
            .is_err()
    );
    let empty = broker
        .prepare_execution_input(
            &wrong_order,
            &PrepareExecutionInput {
                folder_ids: vec![],
                ..request
            },
            &uuid::Uuid::new_v4().to_string(),
            1,
        )
        .unwrap();
    assert!(empty.capture.snapshot.manifest.files.is_empty());
    assert!(empty.capture.snapshot.manifest.directories.is_empty());
}

#[test]
fn chunk_resume_uses_the_custody_bound_block_hash_and_never_serves_a_corrupt_block() {
    use std::io::{Seek, SeekFrom, Write};
    let (temp, broker, order, request) = fixture();
    let bytes = vec![27; 4 * 1024 * 1024 + 17];
    std::fs::write(temp.path().join("chosen/proof.bin"), &bytes).unwrap();
    let prepared = broker
        .prepare_execution_input(&order, &request, &uuid::Uuid::new_v4().to_string(), 1)
        .unwrap();
    let file = &prepared.capture.snapshot.manifest.files[0];
    let tail = &file.chunks[1];
    let input = ReadWorkspaceTransferChunk {
        path: file.path.clone(),
        offset: tail.offset,
        expected_sha256: tail.sha256.clone(),
    };
    assert_eq!(
        broker
            .execution_input_chunk(&prepared.capture, &input)
            .unwrap()
            .bytes,
        &bytes[4 * 1024 * 1024..]
    );
    let mut retained = std::fs::OpenOptions::new()
        .write(true)
        .open(temp.path().join("state/versions").join(&file.sha256))
        .unwrap();
    retained.seek(SeekFrom::Start(tail.offset)).unwrap();
    retained.write_all(&[99]).unwrap();
    retained.sync_all().unwrap();
    drop(retained);
    assert!(
        broker
            .execution_input_chunk(&prepared.capture, &input)
            .is_err()
    );
    let mut forged = input;
    forged.expected_sha256 = hash(&[99; 17]);
    assert!(
        broker
            .execution_input_chunk(&prepared.capture, &forged)
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn input_capture_rejects_links_instead_of_importing_unselected_data() {
    let (temp, broker, order, request) = fixture();
    std::os::unix::fs::symlink(
        temp.path().join("foreign/secret"),
        temp.path().join("chosen/link"),
    )
    .unwrap();
    assert!(
        broker
            .prepare_execution_input(&order, &request, &uuid::Uuid::new_v4().to_string(), 1)
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn nonportable_input_names_are_rejected_before_retaining_their_bytes() {
    let (temp, broker, order, request) = fixture();
    std::fs::write(temp.path().join("chosen/CON.txt"), b"do not retain").unwrap();
    assert!(
        broker
            .prepare_execution_input(&order, &request, &uuid::Uuid::new_v4().to_string(), 1)
            .is_err()
    );
    assert_eq!(
        std::fs::read_dir(temp.path().join("state/versions"))
            .unwrap()
            .count(),
        0
    );
}

#[cfg(windows)]
#[test]
fn input_capture_rejects_a_windows_junction_to_another_project() {
    let (temp, broker, order, request) = fixture();
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(temp.path().join("chosen").join("junction"))
        .arg(temp.path().join("foreign"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        broker
            .prepare_execution_input(&order, &request, &uuid::Uuid::new_v4().to_string(), 1)
            .is_err()
    );
    assert_eq!(
        std::fs::read(temp.path().join("foreign/secret")).unwrap(),
        b"foreign content"
    );
}
