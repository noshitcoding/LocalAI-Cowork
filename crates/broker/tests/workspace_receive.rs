use cap_std::{ambient_authority, fs::Dir};
use cowork_broker::workspace_receive::{TransferBinding, WorkspaceReceiver};
use cowork_contracts::*;
use cowork_domain::{Error, hash, workspace_transfer as transfer};
use std::path::Path;

fn dir(path: &Path) -> Dir {
    Dir::open_ambient_dir(path, ambient_authority()).unwrap()
}
fn binding() -> TransferBinding {
    TransferBinding {
        plan_id: "confirmed-plan".into(),
        plan_sha256: hash(b"confirmed-plan"),
        recipient_id: "private-runner".into(),
        recipient_generation: 3,
    }
}
fn manifest(bytes: &[u8]) -> WorkspaceTransferManifest {
    transfer::build(
        vec!["empty-directory".into(), "nested".into()],
        vec![
            WorkspaceTransferFile {
                path: "empty-file".into(),
                size: 0,
                sha256: hash(&[]),
                chunks: vec![],
            },
            WorkspaceTransferFile {
                path: "nested/Prüfung-📄.bin".into(),
                size: bytes.len() as u64,
                sha256: hash(bytes),
                chunks: bytes
                    .chunks(transfer::CHUNK_BYTES as usize)
                    .enumerate()
                    .map(|(i, b)| WorkspaceTransferChunk {
                        offset: i as u64 * transfer::CHUNK_BYTES,
                        size: b.len() as u64,
                        sha256: hash(b),
                    })
                    .collect(),
            },
        ],
    )
    .unwrap()
}
fn request(manifest: &WorkspaceTransferManifest, index: usize) -> ReadWorkspaceTransferChunk {
    let file = &manifest.files[1];
    ReadWorkspaceTransferChunk {
        path: file.path.clone(),
        offset: file.chunks[index].offset,
        expected_sha256: file.chunks[index].sha256.clone(),
    }
}

#[test]
fn durable_out_of_order_resume_has_identical_receipts_and_complete_private_copy() {
    let staging = tempfile::tempdir().unwrap();
    let working = tempfile::tempdir().unwrap();
    let bytes = vec![73; transfer::CHUNK_BYTES as usize + 17];
    let manifest = manifest(&bytes);
    let receiver =
        WorkspaceReceiver::create(dir(staging.path()), binding(), manifest.clone()).unwrap();
    assert_eq!(receiver.missing_chunks().unwrap().len(), 2);
    assert!(receiver.verify().is_err());
    let tail = receiver
        .accept_chunk(
            &request(&manifest, 1),
            &bytes[transfer::CHUNK_BYTES as usize..],
        )
        .unwrap();
    drop(receiver); // Simulates loss of a successful acknowledgement/process.
    let receiver = WorkspaceReceiver::reopen(dir(staging.path()), &binding(), &manifest).unwrap();
    assert_eq!(
        serde_json::to_value(receiver.missing_chunks().unwrap()).unwrap(),
        serde_json::to_value(vec![request(&manifest, 0)]).unwrap()
    );
    assert_eq!(
        tail,
        receiver
            .accept_chunk(
                &request(&manifest, 1),
                &bytes[transfer::CHUNK_BYTES as usize..]
            )
            .unwrap()
    );
    receiver
        .accept_chunk(
            &request(&manifest, 0),
            &bytes[..transfer::CHUNK_BYTES as usize],
        )
        .unwrap();
    let verified = receiver.verify().unwrap();
    assert!(matches!(
        receiver.accept_chunk(
            &request(&manifest, 0),
            &bytes[..transfer::CHUNK_BYTES as usize]
        ),
        Err(Error::WorkspaceBusy)
    ));
    assert!(WorkspaceReceiver::reopen(dir(staging.path()), &binding(), &manifest).is_err());
    let copied = verified.materialize(dir(working.path())).unwrap();
    assert_eq!(copied.manifest_sha256(), manifest.sha256);
    assert_eq!(
        copied
            .into_directory()
            .read("nested/Prüfung-📄.bin")
            .unwrap(),
        bytes
    );
    assert!(working.path().join("empty-directory").is_dir());
    assert_eq!(
        std::fs::read(working.path().join("empty-file")).unwrap(),
        b""
    );
    assert!(receiver.missing_chunks().unwrap().is_empty());
}

#[test]
fn scope_native_identity_and_confirmed_bytes_are_required_after_reopen() {
    let stage = tempfile::tempdir().unwrap();
    let expected = manifest(b"abc");
    let receiver =
        WorkspaceReceiver::create(dir(stage.path()), binding(), expected.clone()).unwrap();
    let request = request(&expected, 0);
    for changed in [
        TransferBinding {
            recipient_generation: 4,
            ..binding()
        },
        TransferBinding {
            plan_sha256: hash(b"other"),
            ..binding()
        },
    ] {
        assert!(WorkspaceReceiver::reopen(dir(stage.path()), &changed, &expected).is_err());
    }
    assert!(
        receiver
            .accept_chunk(
                &ReadWorkspaceTransferChunk {
                    path: "other".into(),
                    ..request.clone()
                },
                b"abc"
            )
            .is_err()
    );
    assert!(
        receiver
            .accept_chunk(
                &ReadWorkspaceTransferChunk {
                    offset: 1,
                    ..request.clone()
                },
                b"abc"
            )
            .is_err()
    );
    assert!(receiver.accept_chunk(&request, b"ab").is_err());
    receiver.accept_chunk(&request, b"abc").unwrap();
    std::fs::write(stage.path().join("payload.pack"), b"bad").unwrap();
    assert!(receiver.missing_chunks().is_err());
    assert!(receiver.verify().is_err());
    assert!(receiver.accept_chunk(&request, b"abc").is_err()); // Never repair acknowledged bytes silently.
    std::fs::rename(
        stage.path().join("payload.pack"),
        stage.path().join("replaced"),
    )
    .unwrap();
    std::fs::write(stage.path().join("payload.pack"), b"abc").unwrap();
    std::fs::remove_file(stage.path().join("replaced")).unwrap();
    assert!(receiver.accept_chunk(&request, b"abc").is_err());
    assert!(WorkspaceReceiver::reopen(dir(stage.path()), &binding(), &expected).is_err());
}

#[test]
fn complete_file_hash_is_checked_independently_of_valid_blocks() {
    let stage = tempfile::tempdir().unwrap();
    let mut expected = manifest(b"abc");
    expected.files[1].sha256 = hash(b"not-abc");
    expected.sha256 = transfer::manifest_hash(&expected);
    transfer::validate(&expected).unwrap();
    let receiver =
        WorkspaceReceiver::create(dir(stage.path()), binding(), expected.clone()).unwrap();
    receiver
        .accept_chunk(&request(&expected, 0), b"abc")
        .unwrap();
    assert!(receiver.missing_chunks().unwrap().is_empty());
    assert!(receiver.verify().is_err());
}

#[test]
fn crash_before_progress_flush_can_resume_but_unknown_journals_and_trees_are_rejected() {
    let stage = tempfile::tempdir().unwrap();
    let working = tempfile::tempdir().unwrap();
    let expected = manifest(b"abc");
    let receiver =
        WorkspaceReceiver::create(dir(stage.path()), binding(), expected.clone()).unwrap();
    // Bytes reached disk, but the acknowledgement did not. Replay is a data write,
    // with no workload or external operation attached to it.
    std::fs::write(stage.path().join("payload.pack"), b"abc").unwrap();
    assert_eq!(receiver.missing_chunks().unwrap().len(), 1);
    receiver
        .accept_chunk(&request(&expected, 0), b"abc")
        .unwrap();
    std::fs::write(working.path().join("preserve"), b"original").unwrap();
    assert!(
        receiver
            .verify()
            .unwrap()
            .materialize(dir(working.path()))
            .is_err()
    );
    assert_eq!(
        std::fs::read(working.path().join("preserve")).unwrap(),
        b"original"
    );
    std::fs::write(stage.path().join("progress.bits"), [255]).unwrap();
    assert!(receiver.verify().is_err()); // Unused progress bits cannot encode extra blocks.

    let foreign = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(stage.path()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), foreign.path().join(entry.file_name())).unwrap();
    }
    assert!(WorkspaceReceiver::reopen(dir(foreign.path()), &binding(), &expected).is_err());

    let partial = tempfile::tempdir().unwrap();
    std::fs::write(partial.path().join("payload.pack"), b"abc").unwrap();
    assert!(WorkspaceReceiver::reopen(dir(partial.path()), &binding(), &expected).is_err());
    assert!(WorkspaceReceiver::create(dir(partial.path()), binding(), expected).is_err());
    assert_eq!(
        std::fs::read(partial.path().join("payload.pack")).unwrap(),
        b"abc"
    );
}

#[test]
fn empty_snapshot_is_distinct_and_materializes_without_any_chunk_acknowledgement() {
    let stage = tempfile::tempdir().unwrap();
    let working = tempfile::tempdir().unwrap();
    let manifest = transfer::build(vec![], vec![]).unwrap();
    let receiver =
        WorkspaceReceiver::create(dir(stage.path()), binding(), manifest.clone()).unwrap();
    assert!(receiver.missing_chunks().unwrap().is_empty());
    let output = receiver
        .verify()
        .unwrap()
        .materialize(dir(working.path()))
        .unwrap();
    assert_eq!(output.manifest_sha256(), manifest.sha256);
    assert_eq!(std::fs::read_dir(working.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn links_cannot_substitute_private_transfer_objects() {
    use std::os::unix::fs::symlink;
    let stage = tempfile::tempdir().unwrap();
    let receiver =
        WorkspaceReceiver::create(dir(stage.path()), binding(), manifest(b"abc")).unwrap();
    let foreign = tempfile::NamedTempFile::new().unwrap();
    std::fs::rename(stage.path().join("payload.pack"), stage.path().join("keep")).unwrap();
    symlink(foreign.path(), stage.path().join("payload.pack")).unwrap();
    assert!(receiver.verify().is_err());
    std::fs::remove_file(stage.path().join("payload.pack")).unwrap();
    std::fs::hard_link(stage.path().join("keep"), stage.path().join("payload.pack")).unwrap();
    assert!(receiver.verify().is_err());
}
