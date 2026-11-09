//! Explicit platform fixture only: never substitute a host temporary directory
//! for the independently reserved small ext4 test volume.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an explicitly supplied private 16-MiB ext4 test volume"]
fn insufficient_reserved_volume_rejects_before_acknowledgement_without_sparse_fallback() {
    use cap_std::{ambient_authority, fs::Dir};
    use cowork_broker::workspace_receive::{TransferBinding, WorkspaceReceiver};
    use cowork_contracts::*;
    use cowork_domain::{hash, workspace_transfer as transfer};
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let volume = std::env::var_os("COWORK_RECEIVER_TEST_VOLUME")
        .expect("COWORK_RECEIVER_TEST_VOLUME must name the separately reserved fixture");
    let volume = std::fs::canonicalize(volume).unwrap();
    let encoded = CString::new(volume.as_os_str().as_bytes()).unwrap();
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    assert_eq!(
        unsafe { libc::statfs(encoded.as_ptr(), stat.as_mut_ptr()) },
        0
    );
    let stat = unsafe { stat.assume_init() };
    assert_eq!(stat.f_type, 0xef53, "fixture must use ext4");
    let capacity = stat.f_blocks * stat.f_bsize as u64;
    assert!(
        (8 * 1024 * 1024..=20 * 1024 * 1024).contains(&capacity),
        "fixture is not independently bounded: {capacity}"
    );
    let stage = tempfile::tempdir_in(&volume).unwrap();
    let bytes = vec![19; 32 * 1024 * 1024];
    let manifest = transfer::build(
        vec![],
        vec![WorkspaceTransferFile {
            path: "too-large.bin".into(),
            size: bytes.len() as u64,
            sha256: hash(&bytes),
            chunks: bytes
                .chunks(transfer::CHUNK_BYTES as usize)
                .enumerate()
                .map(|(i, chunk)| WorkspaceTransferChunk {
                    offset: i as u64 * transfer::CHUNK_BYTES,
                    size: chunk.len() as u64,
                    sha256: hash(chunk),
                })
                .collect(),
        }],
    )
    .unwrap();
    let binding = TransferBinding {
        plan_id: "reserved-volume-proof".into(),
        plan_sha256: hash(b"reserved-volume-proof"),
        recipient_id: "private-proof-runner".into(),
        recipient_generation: 1,
    };
    let result = WorkspaceReceiver::create(
        Dir::open_ambient_dir(stage.path(), ambient_authority()).unwrap(),
        binding.clone(),
        manifest.clone(),
    );
    assert!(
        result.is_err(),
        "a 32-MiB payload must not be admitted on this bounded volume"
    );
    assert!(!stage.path().join("intent.json").exists());
    assert!(!stage.path().join("progress.bits").exists());
    assert!(
        std::fs::metadata(stage.path().join("payload.pack"))
            .unwrap()
            .len()
            < bytes.len() as u64
    );
    assert!(
        WorkspaceReceiver::reopen(
            Dir::open_ambient_dir(stage.path(), ambient_authority()).unwrap(),
            &binding,
            &manifest
        )
        .is_err()
    );
    println!(
        "bounded ext4 capacity={capacity}; oversized payload rejected; no durable intent or acknowledgement"
    );
}
