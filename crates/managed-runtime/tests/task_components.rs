#![cfg(all(target_os = "linux", feature = "containerd"))]
//! Private physical component tests, not workload/platform qualification.
use cowork_contracts::*;
use cowork_managed_runtime::containerd::{
    cgroups::*, gvisor::GvisorTools, input::*, output::BoundedOutput, rootfs::*,
};
use cowork_runner_protocol::{Binding, SigningKey};
use std::{
    collections::BTreeMap,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn hash(bytes: &[u8]) -> String {
    cowork_domain::hash(bytes)
}
fn keys() -> (Binding, SigningKey, SigningKey, ExecutionTargetRef) {
    let control = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let runner = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let binding = Binding {
        runner_id: "actual-fixture-recipient".into(),
        generation: 7,
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
        revision: 42,
        identity_sha256: cowork_domain::execution_targets::runner_identity_hash(&identity).unwrap(),
    };
    (binding, control, runner, target)
}
fn invocation() -> ProcessInvocation {
    ProcessInvocation {
        program: "shell".into(),
        arguments: vec!["-c".into(), "approved".into()],
        timeout_seconds: 5,
    }
}
fn grant(
    binding: &Binding,
    target: &ExecutionTargetRef,
    now: i64,
    manifest_sha: String,
    request_sha: String,
) -> ManagedInputGrant {
    ManagedInputGrant {
        format_version: 1,
        execution_plan_id: "confirmed-fixture-plan".into(),
        execution_plan_sha256: "b".repeat(64),
        target: target.clone(),
        runner: ExecutionRunnerIdentity {
            runner_id: binding.runner_id.clone(),
            generation: binding.generation,
            adapter_build_sha256: binding.adapter_build_sha256.clone(),
            control_public_key: binding.control_public_key.clone(),
            runner_public_key: binding.runner_public_key.clone(),
        },
        input_manifest_sha256: manifest_sha,
        process_attempt_id: "attempt-once".into(),
        run_id: "run-once".into(),
        request_sha256: request_sha,
        environment: ExecutionEnvironmentRef {
            environment_id: "env".into(),
            version_id: "version".into(),
            definition_sha256: "c".repeat(64),
            qualification_sha256: "d".repeat(64),
        },
        invocation_sha256: cowork_domain::canonical_hash(&invocation()),
        worker_fence_sha256: "e".repeat(64),
        lease_epoch: 3,
        issued_at: now,
        expires_at: now + 300000,
    }
}
#[test]
fn signed_grant_rejects_wrong_purpose_key_generation_revision_and_expiry() {
    let (binding, control, runner, target) = keys();
    let now = 1234567;
    let original = grant(&binding, &target, now, "f".repeat(64), "1".repeat(64));
    let signed = control
        .sign("managed_input_grant", original.clone())
        .unwrap();
    let verified = VerifiedInputGrant::verify(signed.clone(), &binding, &target, now).unwrap();
    assert_eq!(verified.transfer_binding().recipient_generation, 7);
    assert_eq!(
        verified.transfer_binding().plan_id,
        original.execution_plan_id
    );
    assert!(VerifiedInputGrant::verify(signed.clone(), &binding, &target, now - 1).is_err());
    assert!(VerifiedInputGrant::verify(signed.clone(), &binding, &target, now + 300000).is_err());
    assert!(
        VerifiedInputGrant::verify(
            control.sign("launch", original.clone()).unwrap(),
            &binding,
            &target,
            now
        )
        .is_err()
    );
    assert!(
        VerifiedInputGrant::verify(
            runner
                .sign("managed_input_grant", original.clone())
                .unwrap(),
            &binding,
            &target,
            now
        )
        .is_err()
    );
    let mut next = target.clone();
    next.revision += 1;
    assert!(VerifiedInputGrant::verify(signed.clone(), &binding, &next, now).is_err());
    let mut next = binding.clone();
    next.generation += 1;
    assert!(VerifiedInputGrant::verify(signed.clone(), &next, &target, now).is_err());
    for mutate in [0, 1, 2] {
        let mut next = original.clone();
        match mutate {
            0 => next.expires_at += 1,
            1 => next.input_manifest_sha256 = "bad".into(),
            _ => next.lease_epoch = 0,
        };
        assert!(
            VerifiedInputGrant::verify(
                control.sign("managed_input_grant", next).unwrap(),
                &binding,
                &target,
                now
            )
            .is_err()
        );
    }
}
fn private_dir() -> tempfile::TempDir {
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}
#[test]
#[ignore = "Explicit private root component fixture only"]
fn fifo_capture_preserves_original_bytes_and_rejects_permission_changes() {
    let dir = private_dir();
    let output = BoundedOutput::create(dir.path(), 4, 65536).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(output.stdout_path())
        .unwrap()
        .write_all(b"ab\xf0\x9f\x98\x80")
        .unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(output.stderr_path())
        .unwrap()
        .write_all(b"err")
        .unwrap();
    std::fs::set_permissions(output.stdout_path(), std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(output.verify().is_err());
    std::fs::set_permissions(output.stdout_path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    output.verify().unwrap();
    let capture = output.finish().unwrap();
    assert!(capture.complete);
    assert_eq!(capture.stdout.total_bytes, 6);
    assert_eq!(capture.stdout.bytes, b"ab\xf0\x9f");
    assert!(capture.stdout.truncated && capture.stdout.invalid_utf8);
    assert_eq!(capture.stderr.bytes, b"err");
}
#[test]
#[ignore = "Explicit private root component fixture; only its newly spawned trusted Python producer is stopped"]
fn fifo_budget_applies_backpressure_and_drains_after_owned_producer_is_reaped() {
    let dir = private_dir();
    let output = BoundedOutput::create(dir.path(), 4096, 8192).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let child_cancel = cancel.clone();
    let path = output.stdout_path();
    let child = std::thread::spawn(move || {
        cowork_executor::supervise(
            &cowork_executor::TrustedLaunch {
                program: "/usr/bin/python3".into(),
                arguments: vec![
                    "-c".into(),
                    "import sys;open(sys.argv[1],'wb').write(b'x'*1048576)".into(),
                    path.to_string_lossy().into_owned(),
                ],
                directory: PathBuf::from("/tmp"),
                environment: BTreeMap::new(),
                timeout: Duration::from_secs(10),
                capture_bytes_per_stream: 4096,
            },
            &child_cancel,
        )
    });
    let start = Instant::now();
    while !output.limit_exceeded() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let exceeded = output.limit_exceeded();
    cancel.store(true, Ordering::Release);
    let outcome = child.join().unwrap().unwrap();
    assert_eq!(outcome.termination, cowork_executor::Termination::Canceled);
    assert!(exceeded);
    let capture = output.finish().unwrap();
    assert!(capture.limit_exceeded && capture.complete);
    assert_eq!(capture.stdout.bytes.len(), 4096);
    assert!(capture.stdout.truncated);
    assert!(capture.stdout.total_bytes > 8192 && capture.stdout.total_bytes < 1048576);
}
#[test]
#[ignore = "Explicit private root component fixture; creates only fresh owned cgroup children"]
fn actual_cgroup_v2_controls_reject_mutation_without_changing_host_controllers() {
    let before = std::fs::read("/sys/fs/cgroup/cgroup.subtree_control").unwrap();
    let parent = CgroupParent::create().unwrap();
    let task = parent
        .reserve(HardLimits {
            cpu_millis: 250,
            memory_bytes: 128 * 1024 * 1024,
            processes: 64,
        })
        .unwrap();
    let path = Path::new("/sys/fs/cgroup").join(task.oci_path().trim_start_matches('/'));
    task.verify().unwrap();
    assert!(task.is_empty().unwrap());
    assert_eq!(
        std::fs::read_to_string(path.join("cpu.max"))
            .unwrap()
            .trim(),
        "25000 100000"
    );
    std::fs::write(path.join("pids.max"), "65").unwrap();
    let rejected = task.verify().is_err();
    std::fs::write(path.join("pids.max"), "64").unwrap();
    task.retire_unlaunched().unwrap();
    parent.retire_empty().unwrap();
    assert!(rejected);
    assert!(!path.exists());
    assert_eq!(
        std::fs::read("/sys/fs/cgroup/cgroup.subtree_control").unwrap(),
        before
    );
}
#[test]
#[ignore = "Exact complete root-owned immutable upstream toolset fixture required"]
fn complete_gvisor_toolset_is_pinned_and_rejects_metadata_replacement() {
    let path = PathBuf::from(std::env::var("COWORK_TEST_GVISOR_DIRECTORY").unwrap());
    let manifest = std::fs::read(std::env::var("COWORK_TEST_GVISOR_MANIFEST").unwrap()).unwrap();
    let expected = std::env::var("COWORK_TEST_GVISOR_MANIFEST_SHA256").unwrap();
    let tools = GvisorTools::open(&path, &manifest, &expected).unwrap();
    assert_eq!(tools.manifest_sha256(), expected);
    tools.verify().unwrap();
    assert!(GvisorTools::open(&path, &manifest, &"0".repeat(64)).is_err());
    std::fs::set_permissions(path.join("runsc"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let rejected = tools.verify().is_err();
    std::fs::set_permissions(path.join("runsc"), std::fs::Permissions::from_mode(0o555)).unwrap();
    assert!(rejected);
    assert!(
        tools.verify().is_err(),
        "A former immutable binding must not silently adopt changed ctime"
    );
    GvisorTools::open(&path, &manifest, &expected).unwrap();
}
#[test]
#[ignore = "Explicit private root component fixture"]
fn sealed_rootfs_guest_links_do_not_follow_host_paths_and_extra_bytes_are_rejected() {
    let parent = private_dir();
    let path = parent.path().join("rootfs");
    std::fs::create_dir(&path).unwrap();
    std::fs::create_dir(path.join("bin")).unwrap();
    let data = b"#!/bin/sh\nprintf package\n";
    std::fs::write(path.join("bin/tool"), data).unwrap();
    std::fs::set_permissions(
        path.join("bin/tool"),
        std::fs::Permissions::from_mode(0o555),
    )
    .unwrap();
    std::os::unix::fs::symlink("/bin/tool", path.join("bin/alias")).unwrap();
    std::fs::set_permissions(path.join("bin"), std::fs::Permissions::from_mode(0o555)).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o555)).unwrap();
    let manifest = RootfsManifest {
        format_version: 1,
        entries: BTreeMap::from([
            ("bin".into(), RootfsEntry::Directory),
            (
                "bin/alias".into(),
                RootfsEntry::Symlink {
                    target: "/bin/tool".into(),
                },
            ),
            (
                "bin/tool".into(),
                RootfsEntry::File {
                    bytes: data.len() as u64,
                    sha256: hash(data),
                    executable: true,
                },
            ),
        ]),
    };
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let root = SealedRootfs::open(&path, &bytes, &hash(&bytes)).unwrap();
    assert_eq!(root.executable("bin/alias").unwrap(), "/bin/alias");
    assert!(root.executable("../../bin/sh").is_err());
    root.verify().unwrap();
    std::fs::write(path.join("unexpected"), b"not approved").unwrap();
    assert!(root.verify().is_err());
    assert!(SealedRootfs::open(&path, &bytes, &hash(&bytes)).is_err());
}
