//! Actual private filesystem custody only; not actor/read or process authority.
use cowork_broker::{FileBroker, ManagedResultStageBinding, ManagedResultStageRecord};
use cowork_contracts::*;
use cowork_domain::{Error, hash, workspace_transfer as transfer};

fn manifest(bytes: &[u8]) -> WorkspaceTransferManifest {
    transfer::build(
        vec!["copy".into(), "empty".into()],
        vec![
            WorkspaceTransferFile {
                path: "copy/result.bin".into(),
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
            WorkspaceTransferFile {
                path: "empty-file".into(),
                size: 0,
                sha256: hash(b""),
                chunks: vec![],
            },
        ],
    )
    .unwrap()
}
fn binding(broker: &FileBroker, attempt: &str) -> ManagedResultStageBinding {
    ManagedResultStageBinding {
        plan_id: "original-plan".into(),
        plan_sha256: hash(b"plan"),
        run_id: "original-run".into(),
        attempt_id: attempt.into(),
        input_manifest_sha256: hash(b"input"),
        receipt_sha256: hash(b"original-receipt"),
        attestation_sha256: hash(b"output-attestation"),
        native_origin_sha256: broker.local_controller_origin().unwrap(),
    }
}
fn chunk(m: &WorkspaceTransferManifest, i: usize) -> ReadWorkspaceTransferChunk {
    let f = &m.files[0];
    ReadWorkspaceTransferChunk {
        path: f.path.clone(),
        offset: f.chunks[i].offset,
        expected_sha256: f.chunks[i].sha256.clone(),
    }
}
fn broker(path: &std::path::Path) -> FileBroker {
    FileBroker::open(path, vec![]).unwrap()
}

#[test]
fn partial_result_resumes_exact_receipt_and_reads_only_after_full_verification() {
    let temp = tempfile::tempdir().unwrap();
    let owner = broker(temp.path());
    let bytes = vec![73; transfer::CHUNK_BYTES as usize + 17];
    let m = manifest(&bytes);
    let stage = owner
        .create_managed_result_stage(binding(&owner, "attempt-one"), m.clone())
        .unwrap();
    assert!(stage.verify().is_err());
    let tail = stage
        .accept_chunk(&chunk(&m, 1), &bytes[transfer::CHUNK_BYTES as usize..])
        .unwrap();
    let record = stage.record().clone();
    drop(stage);
    drop(owner);
    let owner = broker(temp.path());
    let stage = owner.reopen_managed_result_stage(&record).unwrap();
    assert_eq!(stage.missing_chunks().unwrap().len(), 1);
    assert_eq!(
        stage
            .accept_chunk(&chunk(&m, 1), &bytes[transfer::CHUNK_BYTES as usize..])
            .unwrap(),
        tail
    );
    assert!(stage.verify().is_err());
    stage
        .accept_chunk(&chunk(&m, 0), &bytes[..transfer::CHUNK_BYTES as usize])
        .unwrap();
    let mut verified = stage.verify().unwrap();
    assert_eq!(verified.record(), &record);
    assert_eq!(verified.read_chunk(&chunk(&m, 1)).unwrap(), vec![73; 17]);
    let mut undeclared = chunk(&m, 0);
    undeclared.path = "original.txt".into();
    assert!(verified.read_chunk(&undeclared).is_err());
    assert_eq!(stage.record().manifest().directories, vec!["copy", "empty"]);
}

#[test]
fn unfinished_and_finished_packs_remain_charged_after_drop_and_broker_restart() {
    let temp = tempfile::tempdir().unwrap();
    let owner = broker(temp.path());
    let m = manifest(b"small");
    let mut records = Vec::new();
    for i in 0..8 {
        let stage = owner
            .create_managed_result_stage(binding(&owner, &format!("attempt-{i}")), m.clone())
            .unwrap();
        if i % 2 == 0 {
            stage.accept_chunk(&chunk(&m, 0), b"small").unwrap();
            stage.verify().unwrap();
        }
        records.push(stage.record().clone());
    }
    drop(owner);
    let owner = broker(temp.path());
    assert!(
        matches!(owner.create_managed_result_stage(binding(&owner,"attempt-nine"),m.clone()),Err(Error::Validation(message)) if message=="managed_result_stage_capacity_exceeded")
    );
    for record in &records {
        let stage = owner.reopen_managed_result_stage(record).unwrap();
        assert_eq!(stage.record(), record);
    }
    assert_eq!(
        std::fs::read_dir(temp.path().join("managed-results"))
            .unwrap()
            .count(),
        8
    );
}

#[test]
fn orphaned_create_stays_charged_and_is_never_discovered_as_complete() {
    let temp = tempfile::tempdir().unwrap();
    let owner = broker(temp.path());
    let m = manifest(b"small");
    let stage = owner
        .create_managed_result_stage(binding(&owner, "attempt-one"), m.clone())
        .unwrap();
    drop(stage);
    for i in 1..8 {
        std::fs::create_dir(
            temp.path()
                .join("managed-results")
                .join(hash(format!("orphan-{i}").as_bytes())),
        )
        .unwrap();
    }
    drop(owner);
    let owner = broker(temp.path());
    assert!(matches!(
        owner.create_managed_result_stage(binding(&owner, "attempt-nine"), m),
        Err(Error::Validation(_))
    ));
    assert_eq!(
        std::fs::read_dir(temp.path().join("managed-results"))
            .unwrap()
            .count(),
        8
    );
}

#[test]
fn another_broker_cannot_take_active_receiver_or_foreign_installation_custody() {
    let temp = tempfile::tempdir().unwrap();
    let owner = broker(temp.path());
    let stage = owner
        .create_managed_result_stage(binding(&owner, "attempt-one"), manifest(b"result"))
        .unwrap();
    let record = stage.record().clone();
    let other = broker(temp.path());
    assert!(matches!(
        other.reopen_managed_result_stage(&record),
        Err(Error::WorkspaceBusy)
    ));
    drop(stage);
    other.reopen_managed_result_stage(&record).unwrap();
    let foreign = tempfile::tempdir().unwrap();
    let foreign_owner = broker(foreign.path());
    assert!(matches!(
        foreign_owner.reopen_managed_result_stage(&record),
        Err(Error::Forbidden)
    ));
    assert!(!foreign.path().join("managed-results").exists());
}

#[test]
fn copied_same_bytes_do_not_replace_original_native_container() {
    let temp = tempfile::tempdir().unwrap();
    let owner = broker(temp.path());
    let m = manifest(b"result");
    let stage = owner
        .create_managed_result_stage(binding(&owner, "attempt-one"), m.clone())
        .unwrap();
    stage.accept_chunk(&chunk(&m, 0), b"result").unwrap();
    let record = stage.record().clone();
    drop(stage);
    let original = temp.path().join("managed-results").join(record.id());
    let moved = temp.path().join("retained-original");
    // Both absolute move targets are inside this test's owned temporary root.
    assert!(
        original.is_absolute()
            && original.starts_with(temp.path())
            && moved.is_absolute()
            && moved.starts_with(temp.path())
    );
    std::fs::rename(&original, &moved).unwrap();
    std::fs::create_dir(&original).unwrap();
    std::fs::create_dir(original.join("transfer")).unwrap();
    std::fs::copy(moved.join("record.json"), original.join("record.json")).unwrap();
    for name in [
        "intent.json",
        "payload.pack",
        "progress.bits",
        "transfer.lock",
    ] {
        std::fs::copy(
            moved.join("transfer").join(name),
            original.join("transfer").join(name),
        )
        .unwrap();
    }
    assert!(matches!(
        owner.reopen_managed_result_stage(&record),
        Err(Error::FileConflict)
    ));
}

#[test]
fn corrupted_acknowledged_bytes_and_changed_expected_binding_have_no_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let owner = broker(temp.path());
    let m = manifest(b"result");
    let stage = owner
        .create_managed_result_stage(binding(&owner, "attempt-one"), m.clone())
        .unwrap();
    stage.accept_chunk(&chunk(&m, 0), b"result").unwrap();
    let record = stage.record().clone();
    drop(stage);
    let mut value = serde_json::to_value(&record).unwrap();
    value["binding"]["receipt_sha256"] = hash(b"foreign-receipt").into();
    let changed: ManagedResultStageRecord = serde_json::from_value(value).unwrap();
    assert!(owner.reopen_managed_result_stage(&changed).is_err());
    std::fs::write(
        temp.path()
            .join("managed-results")
            .join(record.id())
            .join("transfer/payload.pack"),
        b"broken",
    )
    .unwrap();
    let stage = owner.reopen_managed_result_stage(&record).unwrap();
    assert!(stage.missing_chunks().is_err());
    assert!(stage.verify().is_err());
    assert!(stage.accept_chunk(&chunk(&m, 0), b"result").is_err());
}

#[test]
fn complete_chunk_acknowledgements_do_not_substitute_for_whole_file_hash() {
    let temp = tempfile::tempdir().unwrap();
    let owner = broker(temp.path());
    let mut m = manifest(b"result");
    m.files[0].sha256 = hash(b"different whole-file content");
    m.sha256 = transfer::manifest_hash(&m);
    transfer::validate(&m).unwrap();
    let stage = owner
        .create_managed_result_stage(binding(&owner, "attempt-one"), m.clone())
        .unwrap();
    stage.accept_chunk(&chunk(&m, 0), b"result").unwrap();
    assert!(stage.missing_chunks().unwrap().is_empty());
    assert!(stage.verify().is_err());
}

#[test]
fn failed_capacity_and_unknown_custody_do_not_allocate_lockfiles() {
    let temp = tempfile::tempdir().unwrap();
    let owner = broker(temp.path());
    let m = manifest(b"result");
    let mut record = None;
    for i in 0..8 {
        let stage = owner
            .create_managed_result_stage(binding(&owner, &format!("attempt-{i}")), m.clone())
            .unwrap();
        record = Some(stage.record().clone());
    }
    let before = std::fs::read_dir(temp.path().join("locks"))
        .unwrap()
        .count();
    for i in 8..40 {
        assert!(
            owner
                .create_managed_result_stage(binding(&owner, &format!("attempt-{i}")), m.clone())
                .is_err()
        );
        let mut forged = serde_json::to_value(record.as_ref().unwrap()).unwrap();
        forged["binding"]["attempt_id"] = format!("missing-{i}").into();
        // Supply a structurally valid, separately expected record for a stage
        // that does not exist. A digest is not proof of native custody.
        forged["id"] = cowork_domain::canonical_hash(&serde_json::json!({
            "format":"owner-managed-result-stage/1", "binding":forged["binding"],
            "manifest_sha256":m.sha256
        }))
        .into();
        let expected: ManagedResultStageRecord = serde_json::from_value(forged).unwrap();
        assert!(owner.reopen_managed_result_stage(&expected).is_err());
    }
    assert_eq!(
        std::fs::read_dir(temp.path().join("locks"))
            .unwrap()
            .count(),
        before
    );
    assert_eq!(
        std::fs::read_dir(temp.path().join("managed-results"))
            .unwrap()
            .count(),
        8
    );
}

#[test]
fn exact_create_intent_replay_recovers_original_preparation_receipt_at_full_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let owner = broker(temp.path());
    let m = manifest(b"result");
    let original_binding = binding(&owner, "attempt-one");
    let stage = owner
        .create_managed_result_stage(original_binding.clone(), m.clone())
        .unwrap();
    let record = stage.record().clone();
    stage.accept_chunk(&chunk(&m, 0), b"result").unwrap();
    drop(stage);
    for i in 1..8 {
        owner
            .create_managed_result_stage(binding(&owner, &format!("other-{i}")), m.clone())
            .unwrap();
    }
    drop(owner);
    let owner = broker(temp.path());
    let before = std::fs::read_dir(temp.path().join("locks"))
        .unwrap()
        .count();
    let recovered = owner
        .create_managed_result_stage(original_binding, m.clone())
        .unwrap();
    assert_eq!(recovered.record(), &record);
    assert!(recovered.missing_chunks().unwrap().is_empty());
    assert_eq!(
        recovered
            .verify()
            .unwrap()
            .read_chunk(&chunk(&m, 0))
            .unwrap(),
        b"result"
    );
    assert_eq!(
        std::fs::read_dir(temp.path().join("locks"))
            .unwrap()
            .count(),
        before
    );
    assert_eq!(
        std::fs::read_dir(temp.path().join("managed-results"))
            .unwrap()
            .count(),
        8
    );
}

#[test]
fn create_intent_replay_never_completes_a_partial_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let owner = broker(temp.path());
    let m = manifest(b"result");
    let original_binding = binding(&owner, "attempt-one");
    let stage = owner
        .create_managed_result_stage(original_binding.clone(), m.clone())
        .unwrap();
    let record = stage.record().clone();
    stage.accept_chunk(&chunk(&m, 0), b"result").unwrap();
    drop(stage);
    let directory = temp.path().join("managed-results").join(record.id());
    let marker = directory.join("record.json");
    assert!(marker.is_absolute() && marker.starts_with(temp.path()));
    std::fs::remove_file(marker).unwrap();
    let pack = std::fs::read(directory.join("transfer/payload.pack")).unwrap();
    assert!(
        owner
            .create_managed_result_stage(original_binding, m)
            .is_err()
    );
    assert!(!directory.join("record.json").exists());
    assert_eq!(
        std::fs::read(directory.join("transfer/payload.pack")).unwrap(),
        pack
    );
    assert_eq!(
        std::fs::read_dir(temp.path().join("managed-results"))
            .unwrap()
            .count(),
        1
    );
}
