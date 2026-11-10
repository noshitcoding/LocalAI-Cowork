use super::*;
struct Fixture {
    binding: Binding,
    control: SigningKey,
    runner: SigningKey,
    ready: ManagedReadyRequest,
    begin: ManagedBeginRequest,
    ticket: ManagedInputTicket,
}
fn fixture() -> Fixture {
    let (mut binding, _, mut grant, launch, control) = crate::managed_input::tests::fixture();
    let runner = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    binding.runner_public_key = runner.public_key();
    grant.runner.runner_public_key = binding.runner_public_key.clone();
    grant.target = ExecutionTargetRef {
        target_id: cowork_domain::execution_targets::execution_target_id(&grant.runner).unwrap(),
        revision: 9,
        identity_sha256: cowork_domain::execution_targets::runner_identity_hash(&grant.runner)
            .unwrap(),
    };
    let manifest = cowork_domain::workspace_transfer::build(vec![], vec![]).unwrap();
    grant.input_manifest_sha256 = manifest.sha256.clone();
    let ready = ManagedReadyRequest {
        format_version: 1,
        request_id: uuid::Uuid::new_v4().to_string(),
        environment: launch.environment.clone(),
        target: grant.target.clone(),
    };
    let readiness = runner
        .sign(
            "managed_readiness",
            ManagedReadiness {
                format_version: 1,
                request_id: ready.request_id.clone(),
                request_sha256: canonical_hash(&ready),
                runner: grant.runner.clone(),
                environment: ready.environment.clone(),
                target: ready.target.clone(),
                runtime_instance_sha256: "2".repeat(64),
                observed_at: 1000,
                expires_at: 16000,
            },
        )
        .unwrap();
    let begin = ManagedBeginRequest {
        format_version: 1,
        readiness,
        launch: control.sign("launch", launch).unwrap(),
        grant: control.sign("managed_input_grant", grant).unwrap(),
        manifest,
    };
    let ticket = ManagedInputTicket {
        format_version: 1,
        input_id: uuid::Uuid::new_v4().to_string(),
        launch_envelope_sha256: envelope_hash(&begin.launch),
        grant_envelope_sha256: canonical_hash(&begin.grant),
        manifest_sha256: begin.manifest.sha256.clone(),
        readiness_sha256: canonical_hash(&begin.readiness),
        runtime_instance_sha256: begin.readiness.payload.runtime_instance_sha256.clone(),
        expires_at: 21000,
    };
    Fixture {
        binding,
        control,
        runner,
        ready,
        begin,
        ticket,
    }
}
#[test]
fn readiness_requires_exact_nonce_recipient_environment_and_bounded_observation() {
    let f = fixture();
    let signed = &f.begin.readiness;
    signed
        .verify("managed_readiness", &f.binding.runner_public_key)
        .unwrap();
    signed.payload.validate(&f.ready, &f.binding, 2000).unwrap();
    assert!(
        signed
            .verify("environment_observation", &f.binding.runner_public_key)
            .is_err()
    );
    assert!(
        signed
            .verify("managed_readiness", &f.binding.control_public_key)
            .is_err()
    );
    for n in 0..4 {
        let mut expected = f.ready.clone();
        match n {
            0 => expected.request_id = uuid::Uuid::new_v4().to_string(),
            1 => expected.target.revision += 1,
            2 => expected.environment.qualification_sha256 = "3".repeat(64),
            _ => expected.format_version += 1,
        }
        assert!(
            signed
                .payload
                .validate(&expected, &f.binding, 2000)
                .is_err()
        );
    }
    for now in [999, 16000] {
        assert!(signed.payload.validate(&f.ready, &f.binding, now).is_err());
    }
    let mut extended = signed.payload.clone();
    extended.expires_at += 1;
    assert!(extended.validate(&f.ready, &f.binding, 2000).is_err());
}
#[test]
fn signed_ticket_pins_original_launch_grant_manifest_readiness_and_instance() {
    let f = fixture();
    let signed = f
        .runner
        .sign("managed_input_ticket", f.ticket.clone())
        .unwrap();
    signed
        .verify("managed_input_ticket", &f.binding.runner_public_key)
        .unwrap();
    signed.payload.validate(&f.begin, 2000).unwrap();
    for n in 0..5 {
        let mut expected = f.begin.clone();
        match n {
            0 => expected.launch.signature[0] ^= 1,
            1 => expected.grant.signature[0] ^= 1,
            2 => expected.manifest.sha256 = "3".repeat(64),
            3 => expected.readiness.signature[0] ^= 1,
            _ => expected.readiness.payload.runtime_instance_sha256 = "4".repeat(64),
        }
        assert!(signed.payload.validate(&expected, 2000).is_err());
    }
    let mut wrong = f.ticket.clone();
    wrong.input_id = "../../other".into();
    assert!(wrong.validate(&f.begin, 2000).is_err());
    wrong = f.ticket.clone();
    wrong.expires_at += 1;
    assert!(wrong.validate(&f.begin, 2000).is_err());
    assert!(f.ticket.validate(&f.begin, 21000).is_err());
    let mut value = serde_json::to_value(&f.begin).unwrap();
    value["host_path"] = "/home/original".into();
    assert!(serde_json::from_value::<ManagedBeginRequest>(value).is_err());
}
#[test]
fn missing_chunks_are_unique_original_manifest_ranges_not_global_hash_reads() {
    let f = fixture();
    let sha = cowork_domain::hash(b"a");
    let manifest = cowork_domain::workspace_transfer::build(
        vec![],
        vec![cowork_contracts::WorkspaceTransferFile {
            path: "input.txt".into(),
            size: 1,
            sha256: sha.clone(),
            chunks: vec![cowork_contracts::WorkspaceTransferChunk {
                offset: 0,
                size: 1,
                sha256: sha.clone(),
            }],
        }],
    )
    .unwrap();
    let mut ticket = f.ticket;
    ticket.manifest_sha256 = manifest.sha256.clone();
    let ticket = f.runner.sign("managed_input_ticket", ticket).unwrap();
    let chunk = ReadWorkspaceTransferChunk {
        path: "input.txt".into(),
        offset: 0,
        expected_sha256: sha,
    };
    let mut reply = ManagedMissingResponse {
        ticket_sha256: canonical_hash(&ticket),
        missing: vec![chunk.clone()],
    };
    reply.validate(&ticket, &manifest).unwrap();
    reply.missing.push(chunk);
    assert!(reply.validate(&ticket, &manifest).is_err());
    reply.missing.pop();
    reply.missing[0].path = "another.txt".into();
    assert!(reply.validate(&ticket, &manifest).is_err());
    reply.missing.clear();
    reply.ticket_sha256 = "0".repeat(64);
    assert!(reply.validate(&ticket, &manifest).is_err());
}
#[test]
fn result_read_is_separate_fresh_content_authority_without_execution_lease_renewal() {
    let f = fixture();
    let input = &f.begin.grant.payload;
    let request = ManagedResultReadRequest {
        format_version: 1,
        runtime_instance_sha256: f.begin.readiness.payload.runtime_instance_sha256.clone(),
        request_id: uuid::Uuid::new_v4().to_string(),
        actor_id: "owner".into(),
        authorization_sha256: "a".repeat(64),
        execution_plan_id: input.execution_plan_id.clone(),
        execution_plan_sha256: input.execution_plan_sha256.clone(),
        process_attempt_id: input.process_attempt_id.clone(),
        run_id: input.run_id.clone(),
        launch_envelope_sha256: envelope_hash(&f.begin.launch),
        execution_receipt_sha256: "b".repeat(64),
        runner: input.runner.clone(),
        target: input.target.clone(),
        operation: ManagedResultReadOperation::Manifest,
        issued_at: 100000,
        expires_at: 130000,
        context: None,
    };
    // Fresh content rights long after the initial 21s execution lease expired
    // do not produce any launch/lease/dispatch capability.
    request
        .validate(
            &f.binding,
            &input.target,
            &f.begin.readiness.payload.runtime_instance_sha256,
            100001,
        )
        .unwrap();
    assert!(
        request
            .validate(&f.binding, &input.target, &"9".repeat(64), 100001)
            .is_err()
    );
    let signed = f
        .control
        .sign("managed_result_read", request.clone())
        .unwrap();
    signed
        .verify("managed_result_read", &f.binding.control_public_key)
        .unwrap();
    assert!(
        signed
            .verify("launch", &f.binding.control_public_key)
            .is_err()
    );
    assert!(
        signed
            .verify("execution_lease", &f.binding.control_public_key)
            .is_err()
    );
    let mut invalid = request.clone();
    invalid.expires_at += 1;
    assert!(
        invalid
            .validate(
                &f.binding,
                &input.target,
                &f.begin.readiness.payload.runtime_instance_sha256,
                100001
            )
            .is_err()
    );
    assert!(
        request
            .validate(
                &f.binding,
                &input.target,
                &f.begin.readiness.payload.runtime_instance_sha256,
                130000
            )
            .is_err()
    );
    invalid = request;
    invalid.operation = ManagedResultReadOperation::Chunk {
        attestation_sha256: "c".repeat(64),
        manifest_sha256: "d".repeat(64),
        chunk: ReadWorkspaceTransferChunk {
            path: "../host".into(),
            offset: 0,
            expected_sha256: "e".repeat(64),
        },
    };
    assert!(
        invalid
            .validate(
                &f.binding,
                &input.target,
                &f.begin.readiness.payload.runtime_instance_sha256,
                100001
            )
            .is_err()
    );
}
#[test]
fn output_attestation_binds_completed_original_receipt_and_independent_retention() {
    let f = fixture();
    let input = &f.begin.grant.payload;
    let empty = cowork_contracts::ProcessCapture {
        bytes: vec![],
        sha256: cowork_domain::hash(b""),
        total_bytes: 0,
        truncated: false,
    };
    let result = ProcessResult {
        termination: cowork_contracts::ProcessTermination::Exited,
        process_id: Some(1),
        exit_code: Some(0),
        signal: None,
        stdout: empty.clone(),
        stderr: empty,
        duration_ms: 1,
        output_complete: true,
    };
    let receipt = f
        .runner
        .sign(
            "receipt",
            Receipt {
                request: f.begin.launch.payload.clone(),
                request_envelope_sha256: envelope_hash(&f.begin.launch),
                state: ExecutionState::Completed { result },
                revision: 3,
                updated_at: 22000,
            },
        )
        .unwrap();
    let attestation = ManagedOutputAttestation {
        format_version: 1,
        runtime_instance_sha256: f.begin.readiness.payload.runtime_instance_sha256.clone(),
        execution_plan_id: input.execution_plan_id.clone(),
        execution_plan_sha256: input.execution_plan_sha256.clone(),
        process_attempt_id: input.process_attempt_id.clone(),
        run_id: input.run_id.clone(),
        launch_envelope_sha256: envelope_hash(&f.begin.launch),
        input_manifest_sha256: input.input_manifest_sha256.clone(),
        output_manifest_sha256: f.begin.manifest.sha256.clone(),
        execution_receipt_sha256: canonical_hash(&receipt),
        physical_record_sha256: "4".repeat(64),
        runner: input.runner.clone(),
        target: input.target.clone(),
        produced_at: 22000,
        retained_until: 22000 + MANAGED_RESULT_RETENTION_MILLIS,
    };
    attestation
        .validate(
            input,
            &f.begin.launch,
            &receipt,
            &f.begin.manifest,
            &f.begin.readiness.payload.runtime_instance_sha256,
            23000,
        )
        .unwrap();
    assert!(
        attestation
            .validate(
                input,
                &f.begin.launch,
                &receipt,
                &f.begin.manifest,
                &"9".repeat(64),
                23000
            )
            .is_err()
    );
    let signed = f
        .runner
        .sign("managed_output", attestation.clone())
        .unwrap();
    signed
        .verify("managed_output", &f.binding.runner_public_key)
        .unwrap();
    assert!(
        signed
            .verify("managed_result_read", &f.binding.runner_public_key)
            .is_err()
    );
    let mut changed = attestation.clone();
    changed.execution_receipt_sha256 = "5".repeat(64);
    assert!(
        changed
            .validate(
                input,
                &f.begin.launch,
                &receipt,
                &f.begin.manifest,
                &f.begin.readiness.payload.runtime_instance_sha256,
                23000
            )
            .is_err()
    );
    changed = attestation;
    changed.retained_until += 1;
    assert!(
        changed
            .validate(
                input,
                &f.begin.launch,
                &receipt,
                &f.begin.manifest,
                &f.begin.readiness.payload.runtime_instance_sha256,
                23000
            )
            .is_err()
    );
}
