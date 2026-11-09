use super::*;

#[test]
fn lifecycle_authority_is_bound_to_purpose_runner_and_deadline() {
    let control = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let runner = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let binding = Binding {
        runner_id: "runner".into(),
        generation: 1,
        adapter_build_sha256: "a".repeat(64),
        control_public_key: control.public_key(),
        runner_public_key: runner.public_key(),
    };
    let mut request = SandboxLifecycleRequest {
        request_id: "intent".into(),
        runner_id: "runner".into(),
        runner_generation: 1,
        adapter_build_sha256: binding.adapter_build_sha256.clone(),
        project_id: "project".into(),
        workspace_resource_id: "workspace".into(),
        environment: ExecutionEnvironmentRef {
            environment_id: "environment".into(),
            version_id: "version".into(),
            definition_sha256: "b".repeat(64),
            qualification_sha256: "c".repeat(64),
        },
        operation: SandboxLifecycleOperation::Suspend,
        expected_sandbox_uid: Some("sandbox-uid".into()),
        issued_at: 1000,
        expires_at: 31_000,
    };
    request.validate(&binding, 1001).unwrap();
    let signed = control.sign("sandbox_lifecycle", request.clone()).unwrap();
    signed
        .verify("sandbox_lifecycle", &control.public_key())
        .unwrap();
    assert!(signed.verify("launch", &control.public_key()).is_err());
    assert!(
        signed
            .verify("sandbox_lifecycle", &runner.public_key())
            .is_err()
    );
    assert!(request.validate(&binding, 31_000).is_err());
    request.runner_generation = 2;
    assert!(request.validate(&binding, 1001).is_err());
    request.runner_generation = 1;
    request.expires_at = 31_001;
    assert!(request.validate(&binding, 1001).is_err());
}

#[test]
fn preview_cannot_expose_runtime_control_ports() {
    let control = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let binding = Binding {
        runner_id: "runner".into(),
        generation: 1,
        adapter_build_sha256: "a".repeat(64),
        control_public_key: control.public_key(),
        runner_public_key: control.public_key(),
    };
    let mut request = SandboxPreviewRequest {
        request_id: "preview".into(),
        runner_id: "runner".into(),
        runner_generation: 1,
        adapter_build_sha256: binding.adapter_build_sha256.clone(),
        project_id: "project".into(),
        workspace_resource_id: "workspace".into(),
        environment: ExecutionEnvironmentRef {
            environment_id: "environment".into(),
            version_id: "version".into(),
            definition_sha256: "b".repeat(64),
            qualification_sha256: "c".repeat(64),
        },
        expected_session_uid: "owned-session".into(),
        port: 8081,
        issued_at: 1000,
        expires_at: 31_000,
    };
    request.validate(&binding, 1001).unwrap();
    for port in [0, 8080, 9090] {
        request.port = port;
        assert!(request.validate(&binding, 1001).is_err());
    }
}
