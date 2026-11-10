//! Controller transport boundary tests, with fresh test-only signing keys.
use super::*;
use cowork_contracts::{
    ExecutionRunnerIdentity, ExecutionTargetRef, ProcessInvocation, WorkspaceTransferChunk,
    WorkspaceTransferFile,
};
use cowork_domain::canonical_hash;
use cowork_runner_protocol::ManagedInputGrant;

#[derive(Clone)]
struct Fixture {
    binding: Binding,
    control: SigningKey,
    runner: SigningKey,
    request: Signed<LaunchRequest>,
    source: ManagedInputManifest,
}

mod tls;

fn fixture() -> Fixture {
    let control = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let runner = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let binding = Binding {
        runner_id: "managed-transport-fixture".into(),
        generation: 3,
        adapter_build_sha256: "a".repeat(64),
        control_public_key: control.public_key(),
        runner_public_key: runner.public_key(),
    };
    let identity = ExecutionRunnerIdentity {
        runner_id: binding.runner_id.clone(),
        generation: binding.generation,
        adapter_build_sha256: binding.adapter_build_sha256.clone(),
        control_public_key: binding.control_public_key.clone(),
        runner_public_key: binding.runner_public_key.clone(),
    };
    let target = ExecutionTargetRef {
        target_id: cowork_domain::execution_targets::execution_target_id(&identity).unwrap(),
        revision: 5,
        identity_sha256: cowork_domain::execution_targets::runner_identity_hash(&identity).unwrap(),
    };
    let manifest = workspace_transfer::build(
        vec!["copy".into()],
        vec![WorkspaceTransferFile {
            path: "copy/input.txt".into(),
            size: 5,
            sha256: hash(b"input"),
            chunks: vec![WorkspaceTransferChunk {
                offset: 0,
                size: 5,
                sha256: hash(b"input"),
            }],
        }],
    )
    .unwrap();
    let issued = now().unwrap();
    let request = LaunchRequest {
        format_version: 1,
        runner_id: binding.runner_id.clone(),
        runner_generation: binding.generation,
        adapter_build_sha256: binding.adapter_build_sha256.clone(),
        process_attempt_id: "attempt-original".into(),
        run_id: "run-original".into(),
        tool_call_id: "tool-original".into(),
        request_sha256: "b".repeat(64),
        worker_fence_sha256: "c".repeat(64),
        lease_epoch: 2,
        environment: ExecutionEnvironmentRef {
            environment_id: "env-original".into(),
            version_id: "version-original".into(),
            definition_sha256: "d".repeat(64),
            qualification_sha256: "e".repeat(64),
        },
        workspace_resource_id: "label-is-not-a-path".into(),
        invocation: ProcessInvocation {
            program: "copy".into(),
            arguments: vec![],
            timeout_seconds: 5,
        },
        issued_at: issued,
        expires_at: issued + 30000,
        execution_deadline: issued + 60000,
    };
    let grant = ManagedInputGrant {
        format_version: 1,
        execution_plan_id: "plan-original".into(),
        execution_plan_sha256: "f".repeat(64),
        target,
        runner: identity,
        input_manifest_sha256: manifest.sha256.clone(),
        process_attempt_id: request.process_attempt_id.clone(),
        run_id: request.run_id.clone(),
        request_sha256: request.request_sha256.clone(),
        environment: request.environment.clone(),
        invocation_sha256: canonical_hash(&request.invocation),
        worker_fence_sha256: request.worker_fence_sha256.clone(),
        lease_epoch: request.lease_epoch,
        issued_at: issued,
        expires_at: issued + 30000,
    };
    Fixture {
        binding,
        request: control.sign("launch", request).unwrap(),
        source: ManagedInputManifest {
            grant: control.sign("managed_input_grant", grant).unwrap(),
            manifest,
        },
        control,
        runner,
    }
}

fn chunk() -> ReadWorkspaceTransferChunk {
    ReadWorkspaceTransferChunk {
        path: "copy/input.txt".into(),
        offset: 0,
        expected_sha256: hash(b"input"),
    }
}

#[test]
fn signed_source_must_bind_original_launch_and_manifest_before_transport() {
    let f = fixture();
    validate_source(&f.binding, &f.request, &f.source, now().unwrap()).unwrap();
    let mut source = f.source.clone();
    source.manifest.files[0].sha256 = hash(b"other");
    source.manifest.sha256 = workspace_transfer::manifest_hash(&source.manifest);
    assert!(validate_source(&f.binding, &f.request, &source, now().unwrap()).is_err());
    let mut other = f.request.payload.clone();
    other.workspace_resource_id.push_str("-replacement");
    let other = f.control.sign("launch", other).unwrap();
    // Workspace labels are part of the signed launch, not the input grant. The
    // ticket must therefore also pin the full original envelope separately.
    assert_ne!(envelope_hash(&other), envelope_hash(&f.request));
    let mut other = f.request.payload.clone();
    other.invocation.arguments.push("changed".into());
    assert!(
        validate_source(
            &f.binding,
            &f.control.sign("launch", other).unwrap(),
            &f.source,
            now().unwrap()
        )
        .is_err()
    );
    let mut wrong = f.source.clone();
    wrong.grant = f.control.sign("launch", wrong.grant.payload).unwrap();
    assert!(validate_source(&f.binding, &f.request, &wrong, now().unwrap()).is_err());
}

#[test]
fn native_missing_list_cannot_request_undeclared_or_duplicate_chunks() {
    let f = fixture();
    validate_missing(&f.source.manifest, &[chunk()]).unwrap();
    assert!(validate_missing(&f.source.manifest, &[chunk(), chunk()]).is_err());
    let mut c = chunk();
    c.path = "../original.txt".into();
    assert!(validate_missing(&f.source.manifest, &[c]).is_err());
    let mut c = chunk();
    c.offset = 1;
    assert!(validate_missing(&f.source.manifest, &[c]).is_err());
    let mut c = chunk();
    c.expected_sha256 = hash(b"other");
    assert!(validate_missing(&f.source.manifest, &[c]).is_err());
    validate_chunk(&f.source.manifest, &chunk(), b"input").unwrap();
    assert!(validate_chunk(&f.source.manifest, &chunk(), b"other").is_err());
    assert!(validate_chunk(&f.source.manifest, &chunk(), b"input-extra").is_err());
}

#[test]
fn signed_history_from_another_attempt_or_envelope_is_not_a_launch_receipt() {
    let f = fixture();
    let value = Receipt {
        request: f.request.payload.clone(),
        request_envelope_sha256: envelope_hash(&f.request),
        state: ExecutionState::Admitted,
        revision: 1,
        updated_at: now().unwrap(),
    };
    verify_receipt(
        &f.binding,
        &f.request,
        f.runner.sign("receipt", value.clone()).unwrap(),
    )
    .unwrap();
    for change in [0, 1, 2, 3] {
        let mut v = value.clone();
        match change {
            0 => v.request.process_attempt_id.push('x'),
            1 => v.request_envelope_sha256 = "0".repeat(64),
            2 => v.revision = 3,
            _ => v.updated_at = f.request.payload.issued_at - 1,
        }
        assert!(
            verify_receipt(&f.binding, &f.request, f.runner.sign("receipt", v).unwrap()).is_err()
        );
    }
    assert!(
        verify_receipt(
            &f.binding,
            &f.request,
            f.control.sign("receipt", value).unwrap()
        )
        .is_err()
    );
}

#[test]
fn monotonic_deadline_cannot_be_reanchored_by_later_freshness_checks() {
    let f = fixture();
    let mut input = Input::new(&f.binding, &f.request, f.source.clone()).unwrap();
    input.fresh(&f.binding, &f.request).unwrap();
    input.deadline = Instant::now();
    assert!(input.fresh(&f.binding, &f.request).is_err());
    assert!(input.fresh(&f.binding, &f.request).is_err());
    let mut source = f.source;
    source.grant.payload.expires_at += 1;
    source.grant = f
        .control
        .sign("managed_input_grant", source.grant.payload)
        .unwrap();
    assert!(Input::new(&f.binding, &f.request, source).is_err());
}
