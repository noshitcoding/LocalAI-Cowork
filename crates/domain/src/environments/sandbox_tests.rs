use super::*;

fn definition() -> EnvironmentDefinition {
    EnvironmentDefinition {
        format_version: 1,
        network: None,
        artifact: EnvironmentArtifact::LinuxImage {
            reference: format!("registry.test/tools@sha256:{}", "a".repeat(64)),
        },
        programs: BTreeMap::from([("python".into(), "usr/bin/python3".into())]),
        packages: vec![],
        limits: EnvironmentLimits {
            cpu_millis: 1000,
            memory_bytes: 256 * 1024 * 1024,
            processes: 64,
            scratch_bytes: 64 * 1024 * 1024,
        },
    }
}

#[test]
fn historical_network_omission_and_gvisor_requirement_are_preserved() {
    let d = definition();
    d.validate().unwrap();
    let value = serde_json::to_value(&d).unwrap();
    assert!(value.get("network").is_none());
    let decoded: EnvironmentDefinition = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(canonical_hash(&decoded), canonical_hash(&value));
    assert!(
        d.required_capabilities()
            .contains(&IsolationCapability::LinuxGvisor)
    );
    assert!(
        !d.required_capabilities()
            .contains(&IsolationCapability::LinuxKata)
    );
}

#[test]
fn managed_packages_require_distinct_v3_manifests_and_hard_limits() {
    let old = definition();
    let old_bytes = serde_json::to_vec(&old).unwrap();
    let previous = EnvironmentQualification {
        runner_id: "runner".into(),
        runner_generation: 1,
        adapter_build_sha256: "b".repeat(64),
        artifact_sha256: old.artifact.sha256().into(),
        definition_sha256: canonical_hash(&old),
        report_sha256: "c".repeat(64),
        capabilities: old.required_capabilities(),
        verified_at: 1,
    };
    let mut managed = old.clone();
    managed.artifact = EnvironmentArtifact::ManagedLinuxPackage {
        package_sha256: "d".repeat(64),
        rootfs_manifest_sha256: "e".repeat(64),
        gvisor_manifest_sha256: "f".repeat(64),
        maximum_files: 2048,
        output_bytes: 1048576,
    };
    assert!(managed.validate().is_err());
    managed.format_version = 2;
    managed.network = Some(EnvironmentNetwork::Denied);
    assert!(managed.validate().is_err());
    managed.format_version = 3;
    managed.validate().unwrap();
    assert!(
        managed
            .required_capabilities()
            .contains(&IsolationCapability::LinuxGvisor)
    );
    assert!(
        managed
            .required_capabilities()
            .contains(&IsolationCapability::ResourceLimits)
    );
    assert!(managed.check_qualification(&previous).is_err());
    managed.network = Some(EnvironmentNetwork::WebProxy {
        policy_id: "web".into(),
    });
    assert!(managed.validate().is_err());
    managed.network = Some(EnvironmentNetwork::Denied);
    if let EnvironmentArtifact::ManagedLinuxPackage { maximum_files, .. } = &mut managed.artifact {
        *maximum_files = 2049;
    }
    assert!(managed.validate().is_err());
    if let EnvironmentArtifact::ManagedLinuxPackage {
        maximum_files,
        rootfs_manifest_sha256,
        ..
    } = &mut managed.artifact
    {
        *maximum_files = 2048;
        *rootfs_manifest_sha256 = "0".into();
    }
    assert!(managed.validate().is_err());
    let decoded: EnvironmentDefinition = serde_json::from_slice(&old_bytes).unwrap();
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), old_bytes);
    decoded.check_qualification(&previous).unwrap();
}

#[test]
fn kata_cannot_be_qualified_with_a_gvisor_report() {
    let mut d = definition();
    d.artifact = EnvironmentArtifact::LinuxKataImage {
        reference: format!("registry.test/tools@sha256:{}", "a".repeat(64)),
    };
    d.validate().unwrap();
    let mut q = EnvironmentQualification {
        runner_id: "runner".into(),
        runner_generation: 1,
        adapter_build_sha256: "b".repeat(64),
        artifact_sha256: d.artifact.sha256().into(),
        definition_sha256: canonical_hash(&d),
        report_sha256: "c".repeat(64),
        capabilities: d.required_capabilities(),
        verified_at: 1,
    };
    d.check_qualification(&q).unwrap();
    q.capabilities.remove(&IsolationCapability::LinuxKata);
    q.capabilities.insert(IsolationCapability::LinuxGvisor);
    assert!(d.check_qualification(&q).is_err());
}

#[test]
fn proxy_is_explicit_v2_operator_policy_and_never_a_url() {
    let mut d = definition();
    d.artifact = EnvironmentArtifact::LinuxKataImage {
        reference: format!("registry.test/tools@sha256:{}", "a".repeat(64)),
    };
    d.network = Some(EnvironmentNetwork::WebProxy {
        policy_id: "public-web".into(),
    });
    assert!(d.validate().is_err());
    d.format_version = 2;
    d.validate().unwrap();
    let caps = d.required_capabilities();
    assert!(caps.contains(&IsolationCapability::NetworkRestricted));
    assert!(!caps.contains(&IsolationCapability::NetworkDenied));
    d.network = Some(EnvironmentNetwork::WebProxy {
        policy_id: "https://proxy.test?token=secret".into(),
    });
    assert!(d.validate().is_err());
    d.network = None;
    assert!(d.validate().is_err());
}

#[test]
fn local_docker_has_explicit_distinct_storage_and_kernel_contract() {
    let mut d = definition();
    d.artifact = EnvironmentArtifact::LocalLinuxImage {
        reference: format!("registry.test/tools@sha256:{}", "a".repeat(64)),
    };
    d.validate().unwrap();
    let caps = d.required_capabilities();
    assert!(caps.contains(&IsolationCapability::LinuxLocalContainer));
    assert!(caps.contains(&IsolationCapability::LocalContainerResourceLimits));
    assert!(!caps.contains(&IsolationCapability::ResourceLimits));
    assert!(!caps.contains(&IsolationCapability::LinuxGvisor));
    assert!(!caps.contains(&IsolationCapability::LinuxKata));
    d.format_version = 2;
    d.network = Some(EnvironmentNetwork::WebProxy {
        policy_id: "public-web".into(),
    });
    assert!(d.validate().is_err());
}

#[test]
fn immutable_bare_image_id_is_local_only() {
    let mut d = definition();
    d.artifact = EnvironmentArtifact::LocalLinuxImage {
        reference: format!("sha256:{}", "a".repeat(64)),
    };
    d.validate().unwrap();
    assert_eq!(d.artifact.sha256(), "a".repeat(64));
    for artifact in [
        EnvironmentArtifact::LinuxImage {
            reference: format!("sha256:{}", "a".repeat(64)),
        },
        EnvironmentArtifact::LinuxKataImage {
            reference: format!("sha256:{}", "a".repeat(64)),
        },
    ] {
        d.artifact = artifact;
        assert!(d.validate().is_err());
    }
}
