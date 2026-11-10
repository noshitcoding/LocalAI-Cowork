use cowork_broker::FileBroker;
use cowork_contracts::*;
use cowork_domain::{hash, workspace_transfer as transfer};
use serde_json::json;

fn fixture() -> (
    tempfile::TempDir,
    FileBroker,
    WorkspaceSnapshot,
    std::path::PathBuf,
    Vec<u8>,
) {
    let temp = tempfile::tempdir().unwrap();
    let broker = FileBroker::open(temp.path(), vec![]).unwrap();
    let order = Order {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: "p".into(),
        title: "Transfer".into(),
        draft: String::new(),
        revision: 1,
        created_at: 0,
    };
    let workspace = broker.ensure_workspace(&order, None, None).unwrap();
    let path = temp.path().join("workspaces").join(&order.id).join("files");
    std::fs::create_dir(path.join("empty-directory")).unwrap();
    std::fs::write(path.join("empty-file"), []).unwrap();
    let bytes = vec![47; transfer::CHUNK_BYTES as usize + 17];
    std::fs::write(path.join("payload.bin"), &bytes).unwrap();
    let run:RunRecord=serde_json::from_value(json!({"id":uuid::Uuid::new_v4().to_string(),"spec":{"order_id":order.id,"project_id":"p","prompt":"x",
        "provider":{"id":"p","name":"p","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":8192,"credential_required":false,"credential_ready":true},
        "access_mode":"confirm_mutations","executor":"local-v2","policy_revision":1,"allowed_tools":["write_text_file"],"budgets":{"max_steps":64,"max_seconds":1800,"max_output_tokens":4096},"folders":[workspace.folder]},
        "state":"running","revision":1,"last_sequence":1,"steps":0,"created_at":0,"updated_at":0,"started_at":null,"error_code":null,"usage":{}})).unwrap();
    let snapshot = broker.snapshot_workspace(&run).unwrap().snapshot.clone();
    (temp, broker, snapshot, path, bytes)
}

#[test]
fn resumable_chunks_reconstruct_retained_bytes_after_source_changes_and_reopen() {
    let (temp, broker, snapshot, path, bytes) = fixture();
    let manifest = broker.workspace_transfer_manifest(&snapshot).unwrap();
    transfer::validate(&manifest).unwrap();
    let serialized = serde_json::to_string(&manifest).unwrap();
    assert!(
        !serialized.contains("resource_id")
            && !serialized.contains("revision")
            && !serialized.contains(&snapshot.order_id)
    );
    assert_eq!(manifest.directories, vec!["empty-directory"]);
    assert!(manifest.files[0].chunks.is_empty());
    std::fs::write(path.join("payload.bin"), b"changed original").unwrap();
    drop(broker);
    let broker = FileBroker::open(temp.path(), vec![]).unwrap();
    assert_eq!(
        broker.workspace_transfer_manifest(&snapshot).unwrap(),
        manifest
    );
    let file = manifest
        .files
        .iter()
        .find(|file| file.path == "payload.bin")
        .unwrap();
    let read = |index: usize| {
        broker
            .workspace_transfer_chunk(
                &snapshot,
                &ReadWorkspaceTransferChunk {
                    path: file.path.clone(),
                    offset: file.chunks[index].offset,
                    expected_sha256: file.chunks[index].sha256.clone(),
                },
            )
            .unwrap()
    };
    let last = read(1); // Resume may fetch the final chunk first.
    assert_eq!(last.bytes.len(), 17);
    assert_eq!(read(1).bytes, last.bytes);
    let mut restored = read(0).bytes;
    restored.extend(last.bytes);
    assert_eq!(restored, bytes);
    assert_eq!(hash(&restored), file.sha256);
    assert_eq!(
        std::fs::read(path.join("payload.bin")).unwrap(),
        b"changed original"
    );
}

#[test]
fn retained_source_transfers_to_an_independent_private_receiver_without_native_ids() {
    use cap_std::{ambient_authority, fs::Dir};
    use cowork_broker::workspace_receive::{TransferBinding, WorkspaceReceiver};
    let (_source, broker, snapshot, source_path, bytes) = fixture();
    let manifest = broker.workspace_transfer_manifest(&snapshot).unwrap();
    let stage = tempfile::tempdir().unwrap();
    let working = tempfile::tempdir().unwrap();
    let binding = TransferBinding {
        plan_id: "test-plan-binding".into(),
        plan_sha256: hash(b"test-plan-binding"),
        recipient_id: "different-runner".into(),
        recipient_generation: 1,
    };
    let open = |p: &std::path::Path| Dir::open_ambient_dir(p, ambient_authority()).unwrap();
    let receiver =
        WorkspaceReceiver::create(open(stage.path()), binding.clone(), manifest.clone()).unwrap();
    let pending = receiver.missing_chunks().unwrap();
    let last = pending.last().unwrap();
    let content = broker.workspace_transfer_chunk(&snapshot, last).unwrap();
    let receipt = receiver.accept_chunk(last, &content.bytes).unwrap();
    drop(receiver);
    std::fs::write(source_path.join("payload.bin"), b"changed during transfer").unwrap();
    let receiver = WorkspaceReceiver::reopen(open(stage.path()), &binding, &manifest).unwrap();
    assert_eq!(
        receipt,
        receiver.accept_chunk(last, &content.bytes).unwrap()
    );
    for chunk in receiver.missing_chunks().unwrap() {
        let content = broker.workspace_transfer_chunk(&snapshot, &chunk).unwrap();
        receiver.accept_chunk(&chunk, &content.bytes).unwrap();
    }
    let completed = receiver
        .verify()
        .unwrap()
        .materialize(open(working.path()))
        .unwrap();
    assert_eq!(completed.manifest_sha256(), manifest.sha256);
    assert_eq!(
        std::fs::read(working.path().join("payload.bin")).unwrap(),
        bytes
    );
    assert_eq!(
        std::fs::read(source_path.join("payload.bin")).unwrap(),
        b"changed during transfer"
    );
    assert!(working.path().join("empty-directory").is_dir());
}

#[test]
fn chunk_scope_and_integrity_cannot_be_replaced_by_an_arbitrary_digest() {
    let (_temp, broker, snapshot, _, _) = fixture();
    let request = ReadWorkspaceTransferChunk {
        path: "payload.bin".into(),
        offset: 0,
        expected_sha256: "0".repeat(64),
    };
    assert!(
        broker
            .workspace_transfer_chunk(&snapshot, &request)
            .is_err()
    );
    for (path, offset) in [
        ("../versions/secret", 0),
        ("missing", 0),
        ("empty-directory", 0),
        ("empty-file", 0),
        ("payload.bin", 1),
        ("payload.bin", u64::MAX),
    ] {
        assert!(
            broker
                .workspace_transfer_chunk(
                    &snapshot,
                    &ReadWorkspaceTransferChunk {
                        path: path.into(),
                        offset,
                        expected_sha256: hash(b"anything")
                    }
                )
                .is_err()
        );
    }
}

#[test]
fn corrupt_retained_content_is_refused_before_any_transfer() {
    let (temp, broker, snapshot, _, bytes) = fixture();
    std::fs::write(temp.path().join("versions").join(hash(&bytes)), b"corrupt").unwrap();
    assert!(broker.workspace_transfer_manifest(&snapshot).is_err());
    assert!(
        broker
            .workspace_transfer_chunk(
                &snapshot,
                &ReadWorkspaceTransferChunk {
                    path: "payload.bin".into(),
                    offset: 0,
                    expected_sha256: hash(b"corrupt")
                }
            )
            .is_err()
    );
}
