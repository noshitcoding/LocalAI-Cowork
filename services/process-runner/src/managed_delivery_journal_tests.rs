//! Component fixtures only. These signers certify no workload/platform results.
use super::*;
use cap_std::fs::PermissionsExt as _;

#[test]
fn fixed_frames_distinguish_vacant_slots_corruption_padding_and_bounds() {
    let file = tempfile::tempfile().unwrap();
    file.set_len(SLOT_BYTES as u64).unwrap();
    assert!(read_frame(&file, 0, ACCEPT_BYTES).unwrap().is_none());
    write_frame(&file, 0, ACCEPT_BYTES, b"original").unwrap();
    assert_eq!(
        read_frame(&file, 0, ACCEPT_BYTES).unwrap().unwrap(),
        b"original"
    );
    file.write_all_at(&[1], ACCEPT_BYTES as u64 - 1).unwrap();
    assert!(read_frame(&file, 0, ACCEPT_BYTES).is_err());
    write_frame(&file, 0, ACCEPT_BYTES, b"original").unwrap();
    file.write_all_at(&[0xff], FRAME_HEADER as u64).unwrap();
    assert!(read_frame(&file, 0, ACCEPT_BYTES).is_err());
    assert!(write_frame(&file, 0, 1, b"x").is_err());
    assert!(write_frame(&file, SLOT_BYTES as u64, STATUS_BYTES, b"x").is_err());
    assert_eq!(file.metadata().unwrap().len(), SLOT_BYTES as u64);
}

struct Fixture {
    directory: tempfile::TempDir,
    parent: Dir,
    control: SigningKey,
    runner: SigningKey,
    binding: Binding,
    source: Source,
}
impl Fixture {
    fn new() -> Self {
        assert_eq!(
            unsafe { libc::geteuid() },
            0,
            "Explicit root-only component fixture"
        );
        let directory = tempfile::tempdir().unwrap();
        let parent = Dir::open_ambient_dir(directory.path(), cap_std::ambient_authority()).unwrap();
        let control = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
        let runner = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
        let identity = ExecutionRunnerIdentity {
            runner_id: "native-journal-fixture".into(),
            generation: 7,
            adapter_build_sha256: "a".repeat(64),
            control_public_key: control.public_key(),
            runner_public_key: runner.public_key(),
        };
        let binding = Binding {
            runner_id: identity.runner_id.clone(),
            generation: identity.generation,
            adapter_build_sha256: identity.adapter_build_sha256.clone(),
            control_public_key: control.public_key(),
            runner_public_key: runner.public_key(),
        };
        let target = ExecutionTargetRef {
            target_id: cowork_domain::execution_targets::execution_target_id(&identity).unwrap(),
            revision: 1,
            identity_sha256: cowork_domain::execution_targets::runner_identity_hash(&identity)
                .unwrap(),
        };
        let source = Source {
            runtime_instance_sha256: "b".repeat(64),
            execution_plan_id: "plan-1".into(),
            execution_plan_sha256: "c".repeat(64),
            process_attempt_id: "attempt-1".into(),
            run_id: "run-1".into(),
            launch_envelope_sha256: "d".repeat(64),
            grant_envelope_sha256: "e".repeat(64),
            input_manifest_sha256: "f".repeat(64),
            runner: identity,
            target,
        };
        Self {
            directory,
            parent,
            control,
            runner,
            binding,
            source,
        }
    }
    fn reserve_component_source(&self, journal: &Journal, n: usize, source: &Source) {
        source.validate(&self.binding).unwrap();
        write_frame(
            &journal.slot(n).unwrap(),
            0,
            ACCEPT_BYTES,
            &serde_json::to_vec(&Record::Reserved {
                source: source.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        let mut index = journal.index.lock().unwrap();
        assert!(
            index
                .attempts
                .insert(source.process_attempt_id.clone(), n)
                .is_none()
        );
        index.used.insert(n);
    }
    fn request(&self) -> Signed<ManagedResultDeliveryRequest> {
        self.control
            .sign(
                "managed_result_delivery_request",
                ManagedResultDeliveryRequest {
                    format_version: 1,
                    request_id: uuid::Uuid::new_v4().to_string(),
                    actor_id: "owner".into(),
                    authorization_sha256: "1".repeat(64),
                    runtime_instance_sha256: self.source.runtime_instance_sha256.clone(),
                    execution_plan_id: self.source.execution_plan_id.clone(),
                    execution_plan_sha256: self.source.execution_plan_sha256.clone(),
                    run_id: self.source.run_id.clone(),
                    process_attempt_id: self.source.process_attempt_id.clone(),
                    launch_envelope_sha256: self.source.launch_envelope_sha256.clone(),
                    execution_receipt_sha256: "2".repeat(64),
                    output_attestation_sha256: "3".repeat(64),
                    manifest_sha256: "4".repeat(64),
                    recipient_version_id: uuid::Uuid::new_v4().to_string(),
                    runner: self.source.runner.clone(),
                    target: self.source.target.clone(),
                    issued_at: 1_000,
                    accept_before: 31_000,
                },
            )
            .unwrap()
    }
    fn receipts(
        &self,
        request: Signed<ManagedResultDeliveryRequest>,
    ) -> (
        Signed<ManagedResultDeliveryReceipt>,
        Signed<ManagedResultRetirementReceipt>,
    ) {
        let accepted = self
            .runner
            .sign(
                "managed_result_delivery_receipt",
                ManagedResultDeliveryReceipt {
                    format_version: 1,
                    request,
                    accepted_at: 1_001,
                },
            )
            .unwrap();
        let pending = self
            .runner
            .sign(
                "managed_result_retirement_receipt",
                ManagedResultRetirementReceipt {
                    format_version: 1,
                    delivery_receipt_sha256: canonical_hash(&accepted),
                    process_attempt_id: self.source.process_attempt_id.clone(),
                    run_id: self.source.run_id.clone(),
                    runtime_instance_sha256: self.source.runtime_instance_sha256.clone(),
                    revision: 1,
                    state: ManagedResultRetirementState::Pending,
                    recorded_at: 1_001,
                    physical_completion_sha256: "5".repeat(64),
                    retirement_evidence_sha256: None,
                },
            )
            .unwrap();
        (accepted, pending)
    }
    fn finish(self) {
        drop(self.parent);
        self.directory.close().unwrap();
    }
}

#[test]
#[ignore = "explicit own root-only file/journal fixture; no workload execution"]
fn original_ack_survives_reopen_and_keeps_its_fixed_quota_and_status_chain() {
    let f = Fixture::new();
    let journal = Journal::initialize(&f.parent, &f.binding, 2).unwrap();
    assert!(
        Journal::open(&f.parent, &f.binding).is_err(),
        "Second owner must not steal original journal"
    );
    f.reserve_component_source(&journal, 0, &f.source);
    assert!(!journal.can_start_new_receiver().unwrap());
    let mut second = f.source.clone();
    second.process_attempt_id = "attempt-2".into();
    f.reserve_component_source(&journal, 1, &second);
    assert!(!journal.can_reserve().unwrap());
    let request = f.request();
    assert!(journal.existing(&request).unwrap().is_none());
    let (accepted, pending) = f.receipts(request.clone());
    let first = journal.accept(accepted.clone(), pending.clone()).unwrap();
    assert!(first.newly_accepted);
    let duplicate = journal.accept(accepted.clone(), pending.clone()).unwrap();
    assert!(!duplicate.newly_accepted && duplicate.receipt == accepted);
    let evidence = serde_json::json!({"component_fixture_only":true});
    let mut retired = pending.payload;
    retired.revision = 2;
    retired.recorded_at = 1_002;
    retired.state = ManagedResultRetirementState::Retired;
    retired.retirement_evidence_sha256 = Some(canonical_hash(&evidence));
    let retired = f
        .runner
        .sign("managed_result_retirement_receipt", retired)
        .unwrap();
    journal.record_status(&retired, Some(evidence)).unwrap();
    let mut changed = retired.payload.clone();
    changed.recorded_at += 1;
    let changed = f
        .runner
        .sign("managed_result_retirement_receipt", changed)
        .unwrap();
    assert!(
        journal
            .record_status(
                &changed,
                Some(serde_json::json!({"component_fixture_only":true}))
            )
            .is_err()
    );
    journal
        .record_status(
            &retired,
            Some(serde_json::json!({"component_fixture_only":true})),
        )
        .unwrap();
    let original_bytes = serde_json::to_vec(&accepted).unwrap();
    drop(journal);
    let reopened = Journal::open(&f.parent, &f.binding).unwrap();
    assert_eq!(
        serde_json::to_vec(&reopened.existing(&request).unwrap().unwrap()).unwrap(),
        original_bytes
    );
    let query = f
        .control
        .sign(
            "managed_result_retirement_query",
            ManagedResultRetirementQuery {
                format_version: 1,
                request_id: uuid::Uuid::new_v4().to_string(),
                delivery_receipt_sha256: canonical_hash(&accepted),
                process_attempt_id: f.source.process_attempt_id.clone(),
                runner: f.source.runner.clone(),
                target: f.source.target.clone(),
                issued_at: 50_000,
                expires_at: 80_000,
            },
        )
        .unwrap();
    assert_eq!(reopened.status(&query, 50_001).unwrap(), retired);
    let mut changed = request.payload;
    changed.manifest_sha256 = "0".repeat(64);
    assert!(
        reopened
            .existing(
                &f.control
                    .sign("managed_result_delivery_request", changed)
                    .unwrap()
            )
            .is_err()
    );
    for n in 0..2 {
        assert_eq!(
            reopened.slot(n).unwrap().metadata().unwrap().len(),
            SLOT_BYTES as u64
        );
    }
    drop(reopened);
    f.finish();
}

#[test]
#[ignore = "explicit own root-only file/journal fixture; no workload execution"]
fn replaced_or_corrupt_native_slot_never_becomes_a_vacant_journal() {
    let f = Fixture::new();
    let journal = Journal::initialize(&f.parent, &f.binding, 2).unwrap();
    f.reserve_component_source(&journal, 0, &f.source);
    let request = f.request();
    let (accepted, pending) = f.receipts(request.clone());
    journal.accept(accepted, pending).unwrap();
    let file = journal.slot(0).unwrap();
    file.write_all_at(&[0xff], FRAME_HEADER as u64).unwrap();
    file.sync_all().unwrap();
    drop(file);
    assert!(journal.existing(&request).is_err());
    assert!(journal.can_reserve().is_err());
    drop(journal);
    let readonly = Journal::open(&f.parent, &f.binding).unwrap();
    assert!(readonly.existing(&request).is_err());
    assert!(readonly.can_reserve().is_err());
    assert!(
        Journal::initialize(&f.parent, &f.binding, 2).is_err(),
        "Cannot recreate a damaged existing journal"
    );
    drop(readonly);
    f.finish();

    let f = Fixture::new();
    let journal = Journal::initialize(&f.parent, &f.binding, 1).unwrap();
    journal
        .directory
        .rename(slot_name(0), &f.parent, "original-slot.bin")
        .unwrap();
    let replacement = create_file(&journal.directory, &slot_name(0), SLOT_BYTES).unwrap();
    drop(replacement);
    assert!(journal.slot(0).is_err());
    drop(journal);
    let readonly = Journal::open(&f.parent, &f.binding).unwrap();
    assert!(readonly.can_reserve().is_err());
    drop(readonly);
    f.finish();
}

#[test]
#[ignore = "explicit own root-only file/journal fixture; no workload execution"]
fn renamed_or_replaced_original_journal_cannot_accept_or_classify_expiry() {
    let f = Fixture::new();
    let journal = Journal::initialize(&f.parent, &f.binding, 1).unwrap();
    f.reserve_component_source(&journal, 0, &f.source);
    let request = f.request();
    let (receipt, pending) = f.receipts(request.clone());
    f.parent
        .rename(DIRECTORY, &f.parent, "original-journal")
        .unwrap();
    assert!(
        journal.existing(&request).is_err(),
        "Missing named original is uncertainty"
    );
    f.parent.create_dir(DIRECTORY).unwrap();
    f.parent
        .set_permissions(DIRECTORY, cap_std::fs::Permissions::from_mode(0o700))
        .unwrap();
    assert!(journal.existing(&request).is_err());
    assert!(journal.accept(receipt.clone(), pending.clone()).is_err());
    f.parent.remove_dir(DIRECTORY).unwrap();
    f.parent
        .rename("original-journal", &f.parent, DIRECTORY)
        .unwrap();
    assert!(journal.existing(&request).unwrap().is_none());
    assert!(
        journal
            .accept(receipt.clone(), pending)
            .unwrap()
            .newly_accepted
    );
    assert_eq!(journal.existing(&request).unwrap(), Some(receipt));
    drop(journal);
    f.finish();
}

#[tokio::test]
#[ignore = "explicit own root-only journal race component; no workload execution"]
async fn expiry_reobserves_original_journal_after_provenance_await() {
    let f = Fixture::new();
    let journal = Journal::initialize(&f.parent, &f.binding, 1).unwrap();
    f.reserve_component_source(&journal, 0, &f.source);
    let request = f.request();
    // The first observation really found an original unaccepted reservation.
    assert!(journal.existing(&request).unwrap().is_none());
    let original_parent = f.parent.try_clone().unwrap();
    tokio::spawn(async move {
        original_parent
            .rename(DIRECTORY, &original_parent, "original-journal")
            .unwrap();
    })
    .await
    .unwrap();
    let observed = request.payload.accept_before;
    let result = crate::managed_http::delivery_window_after_origin(&journal, &request, observed);
    assert!(matches!(
        result,
        Err(crate::managed_http::ApiError(
            _,
            "managed_result_delivery_unconfirmed"
        ))
    ));
    f.parent
        .rename("original-journal", &f.parent, DIRECTORY)
        .unwrap();
    let result = crate::managed_http::delivery_window_after_origin(&journal, &request, observed);
    assert!(matches!(
        result,
        Err(crate::managed_http::ApiError(
            _,
            "managed_result_delivery_confirmation_expired"
        ))
    ));
    let (receipt, pending) = f.receipts(request.clone());
    journal.accept(receipt.clone(), pending).unwrap();
    let historical =
        crate::managed_http::delivery_window_after_origin(&journal, &request, observed)
            .ok()
            .flatten();
    assert_eq!(historical, Some(receipt));
    drop(journal);
    f.finish();
}

#[test]
#[ignore = "explicit own root-only journal startup component; no workload execution"]
fn restart_is_history_only_until_all_original_custody_has_final_proof() {
    let f = Fixture::new();
    let journal = Journal::initialize(&f.parent, &f.binding, 1).unwrap();
    assert!(journal.can_start_new_receiver().unwrap());
    f.reserve_component_source(&journal, 0, &f.source);
    assert!(!journal.can_start_new_receiver().unwrap());
    let request = f.request();
    let (accepted, pending) = f.receipts(request.clone());
    journal.accept(accepted.clone(), pending.clone()).unwrap();
    assert!(!journal.can_start_new_receiver().unwrap());
    drop(journal);
    let journal = Journal::open(&f.parent, &f.binding).unwrap();
    assert_eq!(journal.existing(&request).unwrap(), Some(accepted));
    assert!(!journal.can_start_new_receiver().unwrap());
    let mut retired = pending.payload;
    retired.state = ManagedResultRetirementState::Retired;
    retired.revision = 2;
    let evidence = serde_json::json!({"component_fixture_only":true});
    retired.retirement_evidence_sha256 = Some(canonical_hash(&evidence));
    let retired = f
        .runner
        .sign("managed_result_retirement_receipt", retired)
        .unwrap();
    journal.record_status(&retired, Some(evidence)).unwrap();
    assert!(journal.can_start_new_receiver().unwrap());
    drop(journal);
    f.finish();
}

#[test]
#[ignore = "explicit own root-only original status write fixture; no workload execution"]
fn lost_original_status_write_reply_reobserves_same_candidate_without_reopening_admission() {
    let f = Fixture::new();
    let journal = Journal::initialize(&f.parent, &f.binding, 2).unwrap();
    f.reserve_component_source(&journal, 0, &f.source);
    let request = f.request();
    let (accepted, pending) = f.receipts(request.clone());
    journal.accept(accepted.clone(), pending.clone()).unwrap();
    let mut uncertain = pending.payload;
    uncertain.state = ManagedResultRetirementState::Unconfirmed;
    uncertain.revision = 2;
    let uncertain = f
        .runner
        .sign("managed_result_retirement_receipt", uncertain)
        .unwrap();
    journal
        .lose_status_write_reply
        .store(true, Ordering::Release);
    assert!(journal.record_status(&uncertain, None).is_err());
    assert!(journal.failed.load(Ordering::Acquire));
    assert_eq!(journal.status_writes.lock().unwrap().pending.len(), 1);
    let before = std::fs::read(f.directory.path().join(DIRECTORY).join(slot_name(0))).unwrap();
    let mut changed = uncertain.payload.clone();
    changed.recorded_at += 1;
    let changed = f
        .runner
        .sign("managed_result_retirement_receipt", changed)
        .unwrap();
    assert!(journal.record_status(&changed, None).is_err());
    journal.record_status(&uncertain, None).unwrap();
    assert_eq!(
        before,
        std::fs::read(f.directory.path().join(DIRECTORY).join(slot_name(0))).unwrap()
    );
    assert!(journal.status_writes.lock().unwrap().pending.is_empty());
    assert!(
        journal.can_reserve().is_err(),
        "No fresh admission follows status I/O reconciliation"
    );
    assert_eq!(journal.existing(&request).unwrap(), Some(accepted));
    let evidence = serde_json::json!({"component_fixture_only":true});
    let mut retired = uncertain.payload;
    retired.state = ManagedResultRetirementState::Retired;
    retired.revision = 3;
    retired.retirement_evidence_sha256 = Some(canonical_hash(&evidence));
    let retired = f
        .runner
        .sign("managed_result_retirement_receipt", retired)
        .unwrap();
    journal.record_status(&retired, Some(evidence)).unwrap();
    assert_eq!(journal.status_at(0).unwrap(), Some(retired));
    assert!(journal.failed.load(Ordering::Acquire));
    drop(journal);
    f.finish();
}

#[test]
#[ignore = "explicit own root-only status replacement fixture; no workload execution"]
fn retained_status_write_never_repairs_corrupt_or_replaced_original_frames() {
    for replacement in [false, true] {
        let f = Fixture::new();
        let journal = Journal::initialize(&f.parent, &f.binding, 1).unwrap();
        f.reserve_component_source(&journal, 0, &f.source);
        let (accepted, pending) = f.receipts(f.request());
        journal.accept(accepted, pending.clone()).unwrap();
        let mut uncertain = pending.payload;
        uncertain.state = ManagedResultRetirementState::Unconfirmed;
        uncertain.revision = 2;
        let uncertain = f
            .runner
            .sign("managed_result_retirement_receipt", uncertain)
            .unwrap();
        journal
            .lose_status_write_reply
            .store(true, Ordering::Release);
        assert!(journal.record_status(&uncertain, None).is_err());
        assert!(journal.failed.load(Ordering::Acquire));
        if replacement {
            let data =
                std::fs::read(f.directory.path().join(DIRECTORY).join(slot_name(0))).unwrap();
            journal
                .directory
                .rename(slot_name(0), &f.parent, "original-slot.bin")
                .unwrap();
            let foreign = create_file(&journal.directory, &slot_name(0), SLOT_BYTES).unwrap();
            foreign.write_all_at(&data, 0).unwrap();
            foreign.sync_all().unwrap();
        } else {
            let original = journal.slot(0).unwrap();
            original
                .write_all_at(&[0xff], (ACCEPT_BYTES + FRAME_HEADER) as u64)
                .unwrap();
            original.sync_all().unwrap();
        }
        let path = f.directory.path().join(DIRECTORY).join(slot_name(0));
        let before = std::fs::read(&path).unwrap();
        assert!(journal.record_status(&uncertain, None).is_err());
        assert_eq!(
            std::fs::read(path).unwrap(),
            before,
            "No rewrite or foreign frame repair"
        );
        assert!(journal.can_reserve().is_err());
        assert_eq!(journal.status_writes.lock().unwrap().pending.len(), 1);
        drop(journal);
        f.finish();
    }
}

#[test]
#[ignore = "explicit own root-only first acceptance write fixture; no workload execution"]
fn lost_first_accept_write_reply_needs_original_write_custody_before_any_durable_history() {
    let f = Fixture::new();
    let journal = Journal::initialize(&f.parent, &f.binding, 2).unwrap();
    f.reserve_component_source(&journal, 0, &f.source);
    let request = f.request();
    let (accepted, pending) = f.receipts(request.clone());
    journal
        .lose_accept_write_reply
        .store(true, Ordering::Release);
    assert!(journal.accept(accepted.clone(), pending.clone()).is_err());
    assert!(journal.failed.load(Ordering::Acquire));
    assert!(journal.accept_write.lock().unwrap().is_some());
    let path = f.directory.path().join(DIRECTORY).join(slot_name(0));
    let before = std::fs::read(&path).unwrap();
    assert!(
        journal.existing(&request).is_err(),
        "Historical lookup cannot flush/declare a pending own acceptance durable"
    );
    assert!(
        journal.accept(accepted.clone(), pending.clone()).is_err(),
        "Never retry first acceptance from old request time"
    );
    let query = f
        .control
        .sign(
            "managed_result_retirement_query",
            ManagedResultRetirementQuery {
                format_version: 1,
                request_id: uuid::Uuid::new_v4().to_string(),
                delivery_receipt_sha256: canonical_hash(&accepted),
                process_attempt_id: f.source.process_attempt_id.clone(),
                runner: f.source.runner.clone(),
                target: f.source.target.clone(),
                issued_at: 50_000,
                expires_at: 80_000,
            },
        )
        .unwrap();
    assert!(journal.status(&query, 50_001).is_err());
    assert!(matches!(
        crate::managed_http::delivery_window_after_origin(
            &journal,
            &request,
            request.payload.accept_before
        ),
        Err(crate::managed_http::ApiError(
            _,
            "managed_result_delivery_unconfirmed"
        ))
    ));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "Queries changed no original acceptance bytes"
    );
    journal.reobserve_acceptance(&accepted, &pending).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "Own continuation fsyncs/readbacks, never rewrites acceptance"
    );
    assert!(journal.accept_write.lock().unwrap().is_none());
    assert_eq!(journal.existing(&request).unwrap(), Some(accepted.clone()));
    assert_eq!(journal.status(&query, 50_001).unwrap(), pending.clone());
    assert!(
        journal.reobserve_acceptance(&accepted, &pending).is_err(),
        "Historical JSON cannot create another native continuation"
    );
    assert!(journal.can_reserve().is_err());
    let evidence = serde_json::json!({"component_fixture_only":true});
    let mut retired = pending.payload;
    retired.revision = 2;
    retired.state = ManagedResultRetirementState::Retired;
    retired.retirement_evidence_sha256 = Some(canonical_hash(&evidence));
    let retired = f
        .runner
        .sign("managed_result_retirement_receipt", retired)
        .unwrap();
    journal.record_status(&retired, Some(evidence)).unwrap();
    assert_eq!(journal.status(&query, 50_001).unwrap(), retired);
    assert!(journal.failed.load(Ordering::Acquire));
    drop(journal);
    f.finish();
}

#[test]
#[ignore = "explicit own root-only first acceptance replacement fixture; no workload execution"]
fn first_accept_continuation_never_adopts_replaced_or_corrupt_original_slot() {
    for replacement in [false, true] {
        let f = Fixture::new();
        let journal = Journal::initialize(&f.parent, &f.binding, 1).unwrap();
        f.reserve_component_source(&journal, 0, &f.source);
        let (accepted, pending) = f.receipts(f.request());
        journal
            .lose_accept_write_reply
            .store(true, Ordering::Release);
        assert!(journal.accept(accepted.clone(), pending.clone()).is_err());
        assert!(journal.failed.load(Ordering::Acquire));
        let path = f.directory.path().join(DIRECTORY).join(slot_name(0));
        if replacement {
            let before = std::fs::read(&path).unwrap();
            journal
                .directory
                .rename(slot_name(0), &f.parent, "original-slot.bin")
                .unwrap();
            let foreign = create_file(&journal.directory, &slot_name(0), SLOT_BYTES).unwrap();
            foreign.write_all_at(&before, 0).unwrap();
            foreign.sync_all().unwrap();
        } else {
            let original = journal.slot(0).unwrap();
            original.write_all_at(&[0xff], FRAME_HEADER as u64).unwrap();
            original.sync_all().unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        assert!(journal.reobserve_acceptance(&accepted, &pending).is_err());
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert!(journal.accept_write.lock().unwrap().is_some());
        assert!(journal.can_reserve().is_err());
        drop(journal);
        f.finish();
    }
}
