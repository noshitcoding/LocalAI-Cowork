#![cfg(all(target_os = "linux", feature = "containerd"))]
//! Actual containerd/gVisor calls. Generated fixture keys are not product
//! qualification and no HTTP/client route is introduced by these tests.
use cap_std::{ambient_authority, fs::Dir};
use cowork_contracts::*;
use cowork_domain::{canonical_hash, environments::*};
use cowork_managed_runtime::{
    containerd::{
        cgroups::*, deadline_process::FenceExecutable, gvisor::GvisorTools, input::*, profile::*,
        receive::IncomingManagedInput, rootfs::*, tasks::*, *,
    },
    volumes::*,
};
use cowork_runner_protocol::{Admission, Binding, Broker, LaunchRequest, SigningKey};
use std::{
    collections::BTreeMap,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn hash(bytes: &[u8]) -> String {
    cowork_domain::hash(bytes)
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
fn result_recovery(case: &Path, physical: &ManagedTaskOutcome) -> (serde_json::Value, String) {
    use sha2::Digest;
    let control_root = Dir::open_ambient_dir(
        case.join("volumes")
            .join(physical.reserved_control_volume_id())
            .join("root"),
        ambient_authority(),
    )
    .unwrap();
    let mut task_directories = control_root
        .entries()
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|name| name.to_str().unwrap().starts_with("cw-"))
        .collect::<Vec<_>>();
    assert_eq!(task_directories.len(), 1);
    let recovery = control_root
        .open_dir(task_directories.pop().unwrap())
        .unwrap()
        .read("result-snapshot.bin")
        .unwrap();
    assert_eq!(recovery.len(), 8 * 1024 * 1024);
    let size = u32::from_le_bytes(recovery[..4].try_into().unwrap()) as usize;
    assert!(size > 0 && size <= recovery.len() - 36);
    assert_eq!(
        &recovery[4..36],
        &sha2::Sha256::digest(&recovery[36..36 + size])[..]
    );
    assert!(recovery[36 + size..].iter().all(|b| *b == 0));
    let record_sha256 = hash(&recovery[36..36 + size]);
    let recovery: serde_json::Value = serde_json::from_slice(&recovery[36..36 + size]).unwrap();
    (recovery, record_sha256)
}
fn env_path(key: &str) -> PathBuf {
    std::env::var(key).unwrap().into()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Only the explicitly bounded own-generation task fixture; starts actual gVisor tasks"]
async fn real_gvisor_copy_descendants_cancel_deadline_output_and_disk_limits() {
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let base = env_path("COWORK_RUNTIME_VOLUME_PROOF_DIR");
    assert_eq!(base.file_name().unwrap(), "case-storage");
    let input_only = std::env::var_os("COWORK_TEST_INPUT_ONLY").is_some();
    let backend_only = std::env::var_os("COWORK_TEST_BACKEND_ONLY").is_some();
    let rejected_output =
        std::env::var("COWORK_TEST_BACKEND_ONLY").ok().as_deref() == Some("reject");
    assert!(!(input_only && backend_only));
    let owner_crash = std::env::var_os("COWORK_TEST_OWNER_CRASH").is_some();
    let owner_lease = std::env::var("COWORK_TEST_OWNER_CRASH").ok().as_deref() == Some("lease");
    let marker_prefix = if owner_lease { "lease" } else { "owner" };
    let case = base.join(if owner_lease {
        "actual-tasks-owner-lease"
    } else if owner_crash {
        "actual-tasks-owner"
    } else {
        "actual-tasks"
    });
    std::fs::create_dir(&case).unwrap();
    std::fs::set_permissions(&case, std::fs::Permissions::from_mode(0o700)).unwrap();
    if !owner_crash && !input_only && !backend_only {
        reject_replaceable_package_paths(&base);
        let result = std::process::Command::new(env_path("COWORK_TEST_REAPER_BINARY"))
            .args([
                "--ignored",
                "--exact",
                "containerd::deadline_process::tests::delayed_original_fence_is_reaped_after_drop",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "delayed-fence proof failed: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        println!(
            "actual_fence_reaper=paused_beyond_3s_then_resumed_and_reaped; original_runner_alive=true"
        );
    }
    let formatter = Path::new("/usr/sbin/mke2fs").canonicalize().unwrap();
    let pool = VolumePool::initialize(
        Dir::open_ambient_dir(&case, ambient_authority()).unwrap(),
        PoolLimits {
            reserved_bytes: 432 * 1024 * 1024,
            max_volumes: 18,
        },
        PinnedFormatter::open(&formatter, &hash(&std::fs::read(&formatter).unwrap())).unwrap(),
    )
    .unwrap();
    let control = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let runner = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
    let binding = Binding {
        runner_id: "actual-gvisor-fixture".into(),
        generation: 1,
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
        revision: 1,
        identity_sha256: cowork_domain::execution_targets::runner_identity_hash(&identity).unwrap(),
    };
    let gvisor_bytes = std::fs::read(env_path("COWORK_TEST_GVISOR_MANIFEST")).unwrap();
    let rootfs_bytes = std::fs::read(env_path("COWORK_TEST_ROOTFS_MANIFEST")).unwrap();
    let definition = EnvironmentDefinition {
        format_version: 3,
        network: Some(EnvironmentNetwork::Denied),
        artifact: EnvironmentArtifact::ManagedLinuxPackage {
            package_sha256: "b".repeat(64),
            rootfs_manifest_sha256: hash(&rootfs_bytes),
            gvisor_manifest_sha256: hash(&gvisor_bytes),
            maximum_files: if backend_only { 16384 } else { 2048 },
            output_bytes: 262144,
        },
        programs: BTreeMap::from([("probe".into(), "bin/probe".into())]),
        packages: vec![],
        limits: EnvironmentLimits {
            cpu_millis: 1000,
            memory_bytes: 512 * 1024 * 1024,
            processes: 128,
            scratch_bytes: 32 * 1024 * 1024,
        },
    };
    // Fixture signature exercises exact identity/hash contracts, not a released
    // platform qualification or an app-created ExecutionPlan.
    let q = EnvironmentQualification {
        runner_id: binding.runner_id.clone(),
        runner_generation: binding.generation,
        adapter_build_sha256: binding.adapter_build_sha256.clone(),
        artifact_sha256: definition.artifact.sha256().into(),
        definition_sha256: canonical_hash(&definition),
        report_sha256: "c".repeat(64),
        capabilities: definition.required_capabilities(),
        verified_at: now(),
    };
    let reference = ExecutionEnvironmentRef {
        environment_id: "private-fixture".into(),
        version_id: "actual-tools".into(),
        definition_sha256: canonical_hash(&definition),
        qualification_sha256: canonical_hash(&q),
    };
    let signed_qualification = runner.sign("environment_qualification", q).unwrap();
    let installed = cowork_managed_runtime::containerd::bootstrap::InstalledManagedProfile {
        format_version: 1,
        binding: binding.clone(),
        target: target.clone(),
        reference: reference.clone(),
        definition: definition.clone(),
        qualification: signed_qualification.clone(),
    };
    let profile = Arc::new(
        ManagedProfile::from_qualification(
            binding.clone(),
            target.clone(),
            reference.clone(),
            definition,
            signed_qualification,
            RuntimeArtifacts {
                tools: GvisorTools::open(
                    &env_path("COWORK_TEST_GVISOR_DIRECTORY"),
                    &gvisor_bytes,
                    &hash(&gvisor_bytes),
                )
                .unwrap(),
                rootfs: SealedRootfs::open(
                    &env_path("COWORK_TEST_ROOTFS_DIRECTORY"),
                    &rootfs_bytes,
                    &hash(&rootfs_bytes),
                )
                .unwrap(),
                fence: FenceExecutable::open(
                    &env_path("COWORK_TEST_FENCE_BINARY"),
                    &std::env::var("COWORK_TEST_FENCE_SHA256").unwrap(),
                )
                .unwrap(),
                driver: runsc_driver::RunscDriverExecutable::open(
                    &env_path("COWORK_TEST_DRIVER_BINARY"),
                    &std::env::var("COWORK_TEST_DRIVER_SHA256").unwrap(),
                )
                .unwrap(),
            },
        )
        .unwrap(),
    );
    let groups = CgroupParent::create_in(&env_path("COWORK_RUNTIME_CGROUP_PARENT")).unwrap();
    sqlx::any::install_default_drivers();
    let db = sqlx::any::AnyPoolOptions::new()
        .max_connections(1)
        .connect(&format!(
            "sqlite://{}?mode=rwc",
            base.join(if owner_lease {
                "owner-lease-runner.db"
            } else if owner_crash {
                "owner-runner.db"
            } else {
                "runner.db"
            })
            .display()
        ))
        .await
        .unwrap();
    let broker = Broker::open(db, binding.clone(), runner.clone())
        .await
        .unwrap();
    let data = b"approved snapshot bytes\n";
    let mut manifest = WorkspaceTransferManifest {
        format_version: 1,
        directories: vec![],
        files: vec![WorkspaceTransferFile {
            path: "input.txt".into(),
            size: data.len() as u64,
            sha256: hash(data),
            chunks: vec![WorkspaceTransferChunk {
                offset: 0,
                size: data.len() as u64,
                sha256: hash(data),
            }],
        }],
        total_bytes: data.len() as u64,
        sha256: String::new(),
    };
    manifest.sha256 = cowork_domain::workspace_transfer::manifest_hash(&manifest);
    let modes: &[&str] = if input_only {
        &["copy", "input_swap", "input_revoke"]
    } else if backend_only {
        if rejected_output {
            &["symlink_result"]
        } else {
            &["copy"]
        }
    } else if owner_crash {
        &["owner_crash"]
    } else {
        &[
            "copy",
            "background",
            "cancel",
            "deadline",
            "output",
            "disk",
            "exit128",
            "wait_fault",
            "input_deadline",
        ]
    };
    for &mode in modes {
        let wall = now();
        let invocation = ProcessInvocation {
            program: "probe".into(),
            arguments: vec![
                if matches!(
                    mode,
                    "cancel" | "deadline" | "owner_crash" | "wait_fault" | "input_deadline"
                ) {
                    "block"
                } else {
                    mode
                }
                .into(),
                std::env::var("COWORK_TEST_HOST_SENTINEL").unwrap(),
            ],
            timeout_seconds: if owner_lease || mode == "input_deadline" {
                40
            } else if mode == "deadline" {
                // Includes cold bootstrap. Keep enough time for actual guest
                // start so this case measures an active workload's deadline.
                23
            } else {
                25
            },
        };
        let request = LaunchRequest {
            format_version: 1,
            runner_id: binding.runner_id.clone(),
            runner_generation: binding.generation,
            adapter_build_sha256: binding.adapter_build_sha256.clone(),
            process_attempt_id: format!("attempt-{mode}"),
            run_id: format!("run-{mode}"),
            tool_call_id: format!("tool-{mode}"),
            request_sha256: hash(mode.as_bytes()),
            worker_fence_sha256: "d".repeat(64),
            lease_epoch: 1,
            environment: reference.clone(),
            workspace_resource_id: "not-input-authority".into(),
            invocation: invocation.clone(),
            issued_at: wall,
            expires_at: wall + if owner_lease { 25000 } else { 30000 },
            execution_deadline: wall + 60000,
        };
        let raw = ManagedInputGrant {
            format_version: 1,
            execution_plan_id: format!("fixture-confirmed-{mode}"),
            execution_plan_sha256: "e".repeat(64),
            target: target.clone(),
            runner: identity.clone(),
            input_manifest_sha256: manifest.sha256.clone(),
            process_attempt_id: request.process_attempt_id.clone(),
            run_id: request.run_id.clone(),
            request_sha256: request.request_sha256.clone(),
            environment: reference.clone(),
            invocation_sha256: canonical_hash(&invocation),
            worker_fence_sha256: request.worker_fence_sha256.clone(),
            lease_epoch: 1,
            issued_at: wall,
            expires_at: if mode == "input_deadline" {
                wall + 23000
            } else {
                wall + 60000
            },
        };
        let signed = control.sign("launch", request.clone()).unwrap();
        let signed_grant = control.sign("managed_input_grant", raw).unwrap();
        if backend_only {
            use cowork_managed_runtime::containerd::{
                backend::{BeginInput, ManagedOutputStatus},
                bootstrap::{ManagedBootstrap, PinnedRuntimeFile},
            };
            let pinned = |path: PathBuf| PinnedRuntimeFile {
                sha256: hash(&std::fs::read(&path).unwrap()),
                path,
            };
            let installed_path = base.join("installed-profile.json");
            std::fs::write(&installed_path, serde_json::to_vec(&installed).unwrap()).unwrap();
            std::fs::set_permissions(&installed_path, std::fs::Permissions::from_mode(0o600))
                .unwrap();
            let mut bootstrap = ManagedBootstrap {
                format_version: 1,
                installed_profile: pinned(installed_path),
                gvisor_directory: env_path("COWORK_TEST_GVISOR_DIRECTORY"),
                gvisor_manifest: pinned(env_path("COWORK_TEST_GVISOR_MANIFEST")),
                rootfs_directory: env_path("COWORK_TEST_ROOTFS_DIRECTORY"),
                rootfs_manifest: pinned(env_path("COWORK_TEST_ROOTFS_MANIFEST")),
                containerd: pinned(env_path("COWORK_TEST_CONTAINERD_BINARY")),
                fence: pinned(env_path("COWORK_TEST_FENCE_BINARY")),
                runsc_driver: pinned(env_path("COWORK_TEST_DRIVER_BINARY")),
                formatter: pinned(formatter.clone()),
                volume_pool_directory: case.clone(),
                volume_pool_limits: PoolLimits {
                    reserved_bytes: 432 * 1024 * 1024,
                    max_volumes: 18,
                },
                data_filesystem_max_bytes: 64 * 1024 * 1024,
                delegated_cgroup_directory: env_path("COWORK_RUNTIME_CGROUP_PARENT"),
            };
            assert!(
                bootstrap.activate(broker.clone()).is_err(),
                "An oversized data filesystem cannot be silently accepted"
            );
            assert_eq!(pool.usage().unwrap().reserved_bytes, 0);
            bootstrap.data_filesystem_max_bytes = 1024 * 1024 * 1024;
            let mut mismatched = bootstrap.clone();
            mismatched.volume_pool_limits.max_volumes += 1;
            assert!(
                mismatched.activate(broker.clone()).is_err(),
                "Stored pool limits must exactly match operator policy"
            );
            let receiver = bootstrap.activate(broker.clone()).unwrap();
            let before = pool.usage().unwrap().reserved_bytes;
            receiver.ready(&reference, &target).unwrap();
            let mut wrong = target.clone();
            wrong.revision += 1;
            assert!(receiver.ready(&reference, &wrong).is_err());
            assert_eq!(
                pool.usage().unwrap().reserved_bytes,
                before,
                "Readiness must not reserve"
            );
            let BeginInput::Transfer(transfer) = receiver
                .begin(signed.clone(), signed_grant.clone(), manifest.clone())
                .await
                .unwrap()
            else {
                panic!()
            };
            for chunk in transfer.missing_chunks().await.unwrap() {
                assert!(transfer.accept_chunk(&chunk, b"corrupt").await.is_err());
                let first = transfer.accept_chunk(&chunk, data).await.unwrap();
                assert_eq!(first, transfer.accept_chunk(&chunk, data).await.unwrap());
            }
            drop(transfer);
            let BeginInput::Transfer(transfer) = receiver
                .begin(signed.clone(), signed_grant.clone(), manifest.clone())
                .await
                .unwrap()
            else {
                panic!()
            };
            assert!(transfer.missing_chunks().await.unwrap().is_empty());
            assert!(matches!(
                broker.receipt(&request.process_attempt_id).await,
                Err(cowork_runner_protocol::Error::NotFound)
            ));
            let execution = (*transfer).seal_and_start().await.unwrap();
            let completed = tokio::time::timeout(Duration::from_secs(40), execution.wait())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                completed.physical.result.termination,
                ProcessTermination::Exited
            );
            assert_eq!(completed.physical.result.exit_code, Some(0));
            assert!(completed.physical.result.output_complete);
            let (recovery, recovery_sha256) = result_recovery(&case, &completed.physical);
            assert_eq!(
                recovery["execution_receipt_sha256"],
                canonical_hash(&completed.receipt)
            );
            if rejected_output {
                let ManagedOutputStatus::Rejected(rejection) = &completed.output else {
                    panic!("Unportable output must not acquire a snapshot capability")
                };
                assert_eq!(rejection.reason, "result_symlink_unsupported");
                assert_eq!(
                    rejection.record_sha256.as_deref(),
                    Some(recovery_sha256.as_str())
                );
                assert_eq!(recovery["output_state"], "rejected");
                assert!(recovery.get("manifest").is_none());
                let BeginInput::Existing(replay) = receiver
                    .begin(signed, signed_grant, manifest.clone())
                    .await
                    .unwrap()
                else {
                    panic!()
                };
                assert_eq!(*replay, completed.receipt);
                println!(
                    "managed_backend_export_rejected=verified; original_completed_receipt_retained=true; output_snapshot_capability=false; durable_rejection=verified; immutable_replay=true; guest_redispatch=false"
                );
                continue;
            }
            let ManagedOutputStatus::Ready(snapshot) = &completed.output else {
                panic!("Portable output must be available")
            };
            let output = snapshot.manifest().unwrap();
            assert_eq!(recovery["output_state"], "ready");
            assert_eq!(recovery["manifest"], serde_json::to_value(&output).unwrap());
            assert_eq!(
                recovery["physical_record_sha256"],
                completed.physical.physical_record_sha256
            );
            for path in ["input.txt", "result.txt"] {
                let file = output.files.iter().find(|f| f.path == path).unwrap();
                assert_eq!(
                    snapshot
                        .chunk(&ReadWorkspaceTransferChunk {
                            path: path.into(),
                            offset: 0,
                            expected_sha256: file.chunks[0].sha256.clone()
                        })
                        .unwrap(),
                    data
                );
            }
            assert!(
                snapshot
                    .chunk(&ReadWorkspaceTransferChunk {
                        path: "../../host".into(),
                        offset: 0,
                        expected_sha256: hash(data)
                    })
                    .is_err()
            );
            let BeginInput::Existing(replay) = receiver
                .begin(signed, signed_grant, manifest.clone())
                .await
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(*replay, completed.receipt);
            let workspace = completed.physical.workspace().unwrap();
            assert_eq!(
                workspace
                    .open("result.txt")
                    .unwrap()
                    .into_std()
                    .metadata()
                    .unwrap()
                    .uid(),
                0
            );
            workspace.remove_file("result.txt").unwrap();
            workspace.write("result.txt", data).unwrap();
            assert!(
                snapshot.manifest().is_err(),
                "Equal replacement bytes cannot substitute a native result"
            );
            println!(
                "managed_backend_copy=verified; pinned_bootstrap=verified; bounded_data_filesystem=verified; exact_pool_policy=verified; readiness_nonreserving=true; original_transfer_resume=true; predispatch_execution_rows=0; immutable_execution_replay=true; result_chunks_verified=true; preallocated_result_recovery=verified; native_result_substitution=rejected; output_manifest_sha256={}",
                output.sha256
            );
            continue;
        }
        let volume_request = VolumeRequest {
            request_sha256: request.request_sha256.clone(),
            bytes: 32 * 1024 * 1024,
            inodes: 2048,
        };
        let volume = pool.reserve(volume_request.clone()).unwrap();
        let work = volume.data_directory().unwrap();
        let incoming = IncomingManagedInput::create(
            profile.clone(),
            &broker,
            broker.authorize_input_transfer(&signed).await.unwrap(),
            signed_grant.clone(),
            manifest.clone(),
            volume,
        )
        .await
        .unwrap();
        let volume_id = incoming.reserved_volume_id().to_owned();
        let other_pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let other = Broker::open(other_pool, binding.clone(), runner.clone())
            .await
            .unwrap();
        assert!(
            incoming
                .missing_chunks(
                    &other,
                    other.authorize_input_transfer(&signed).await.unwrap()
                )
                .await
                .is_err(),
            "Identical keys and bytes cannot rebind an active receiver to another ledger"
        );
        let chunks = incoming
            .missing_chunks(
                &broker,
                broker.authorize_input_transfer(&signed).await.unwrap(),
            )
            .await
            .unwrap();
        for chunk in &chunks {
            assert!(
                incoming
                    .accept_chunk(
                        &broker,
                        broker.authorize_input_transfer(&signed).await.unwrap(),
                        chunk,
                        b"corrupt"
                    )
                    .await
                    .is_err()
            );
            let first = incoming
                .accept_chunk(
                    &broker,
                    broker.authorize_input_transfer(&signed).await.unwrap(),
                    chunk,
                    data,
                )
                .await
                .unwrap();
            let replay = incoming
                .accept_chunk(
                    &broker,
                    broker.authorize_input_transfer(&signed).await.unwrap(),
                    chunk,
                    data,
                )
                .await
                .unwrap();
            assert_eq!(first, replay);
        }
        drop(incoming);
        let volume = pool.reserve(volume_request.clone()).unwrap();
        assert_eq!(volume.id(), volume_id);
        assert!(
            IncomingManagedInput::reopen(
                profile.clone(),
                &other,
                other.authorize_input_transfer(&signed).await.unwrap(),
                signed_grant.clone(),
                manifest.clone(),
                volume
            )
            .await
            .is_err(),
            "Reopen must retain original Broker custody"
        );
        let volume = pool.reserve(volume_request.clone()).unwrap();
        let incoming = IncomingManagedInput::reopen(
            profile.clone(),
            &broker,
            broker.authorize_input_transfer(&signed).await.unwrap(),
            signed_grant,
            manifest.clone(),
            volume,
        )
        .await
        .unwrap();
        assert!(
            incoming
                .missing_chunks(
                    &broker,
                    broker.authorize_input_transfer(&signed).await.unwrap()
                )
                .await
                .unwrap()
                .is_empty()
        );
        if mode == "input_swap" {
            work.rename("workspace", &work, "detached-workspace")
                .unwrap();
            work.create_dir("workspace").unwrap();
            assert!(
                incoming
                    .seal(
                        &broker,
                        broker.authorize_input_transfer(&signed).await.unwrap()
                    )
                    .await
                    .is_err()
            );
            assert_eq!(
                work.open_dir("detached-workspace")
                    .unwrap()
                    .open(".")
                    .unwrap()
                    .into_std()
                    .metadata()
                    .unwrap()
                    .uid(),
                0,
                "A substituted tree must never become workload-owned"
            );
            assert!(
                pool.reserve(volume_request).is_err(),
                "Uncertain published reservation stays booked"
            );
            println!(
                "managed_snapshot_native_tree_substitution=rejected; uncertain_reservation_retained=true"
            );
            continue;
        }
        let input = incoming
            .seal(
                &broker,
                broker.authorize_input_transfer(&signed).await.unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            work.open_dir("workspace")
                .unwrap()
                .read("input.txt")
                .unwrap(),
            data
        );
        if input_only {
            assert!(
                pool.reserve(volume_request).is_err(),
                "A published snapshot cannot be reopened as an unlaunched transfer"
            );
            assert!(matches!(
                broker.receipt(&request.process_attempt_id).await,
                Err(cowork_runner_protocol::Error::NotFound)
            ));
            assert_eq!(
                work.open_dir("workspace")
                    .unwrap()
                    .open("input.txt")
                    .unwrap()
                    .into_std()
                    .metadata()
                    .unwrap()
                    .uid(),
                1000
            );
            let Admission::Launch(other_permit) = other.admit(signed.clone()).await.unwrap() else {
                panic!()
            };
            let other_dispatch = other.dispatch_owned(other_permit).await.unwrap();
            assert!(
                input
                    .validate_dispatch(&other_dispatch, &other, &binding, &target, now())
                    .is_err(),
                "Sealed input cannot dispatch through another ledger even with identical bytes"
            );
            let original_dispatch = if mode == "copy" {
                let Admission::Launch(permit) = broker.admit(signed.clone()).await.unwrap() else {
                    panic!()
                };
                let dispatch = broker.dispatch_owned(permit).await.unwrap();
                input
                    .validate_dispatch(&dispatch, &broker, &binding, &target, now())
                    .unwrap();
                let mut revised = target.clone();
                revised.revision += 1;
                assert!(
                    input
                        .validate_dispatch(&dispatch, &broker, &binding, &revised, now())
                        .is_err()
                );
                Some(dispatch)
            } else {
                None
            };
            broker
                .fence(
                    control
                        .sign(
                            "fence",
                            cowork_runner_protocol::FenceRequest {
                                runner_id: binding.runner_id.clone(),
                                runner_generation: binding.generation,
                                run_id: request.run_id.clone(),
                                lease_epoch: request.lease_epoch,
                                revoked: true,
                            },
                        )
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(broker.authorize_input_transfer(&signed).await.is_err());
            if let Some(dispatch) = original_dispatch {
                assert!(!broker.execution_allowed(dispatch.request()).await.unwrap());
            } else {
                assert!(broker.admit(signed).await.is_err());
            }
            println!(
                "managed_snapshot_transfer=verified; replay_receipt_identical=true; reopened_original_volume=true; predispatch_execution_rows=0; revoked_dispatch_denied=true; reserved_volume_retained=true; foreign_broker_active_reopen_sealed=rejected"
            );
            continue;
        }
        let Admission::Launch(permit) = broker.admit(signed.clone()).await.unwrap() else {
            panic!("expected one use permit")
        };
        let dispatch = broker.dispatch_owned(permit).await.unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let group = groups.reserve(profile.limits()).unwrap();
        let workload_path = format!("/sys/fs/cgroup{}/workload", group.oci_path());
        let control_volume = pool
            .reserve(TaskResources::control_volume_request(&request.request_sha256).unwrap())
            .unwrap();
        let control_root = control_volume.directory().try_clone().unwrap();
        let task = ManagedTask::prepare(
            profile.clone(),
            dispatch,
            input,
            TaskResources {
                group,
                control: control_volume,
            },
            broker.clone(),
            cancel.clone(),
            &target,
        )
        .unwrap();
        let task_id = task.task_id().to_owned();
        if mode == "wait_fault" {
            // Real filesystem fault, confined to this fixture's private mount
            // namespace. Root pause evidence stays writable/alive; only this
            // fresh workload wait slot becomes read-only. No production flag.
            use std::os::fd::AsRawFd;
            let root_path =
                std::fs::read_link(format!("/proc/self/fd/{}", control_root.as_raw_fd())).unwrap();
            let slot = std::ffi::CString::new(
                root_path
                    .join(&task_id)
                    .join("wait-evidence")
                    .join(format!("{task_id}.wait"))
                    .as_os_str()
                    .as_encoded_bytes(),
            )
            .unwrap();
            assert_eq!(
                unsafe {
                    libc::mount(
                        slot.as_ptr(),
                        slot.as_ptr(),
                        std::ptr::null(),
                        libc::MS_BIND,
                        std::ptr::null(),
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    libc::mount(
                        std::ptr::null(),
                        slot.as_ptr(),
                        std::ptr::null(),
                        libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY,
                        std::ptr::null(),
                    )
                },
                0
            );
        }
        // The daemon's private mount namespace is captured after both reserved
        // volumes exist. There is no unverified host-path mount fallback.
        let mounts_before_daemon = std::fs::read("/proc/thread-self/mountinfo").unwrap();
        let daemon = task
            .start_daemon(
                DaemonExecutable::open(
                    &env_path("COWORK_TEST_CONTAINERD_BINARY"),
                    &std::env::var("COWORK_TEST_CONTAINERD_SHA256").unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read("/proc/thread-self/mountinfo").unwrap(),
            mounts_before_daemon,
            "control mounts must stay in the new supervisor namespace"
        );
        let outcome = {
            let cancel_after_start = async {
                if mode != "cancel" && mode != "owner_crash" {
                    std::future::pending::<()>().await;
                }
                tokio::time::timeout(Duration::from_secs(24), async {
                    loop {
                        if work
                            .open_dir("workspace")
                            .unwrap()
                            .try_exists("started")
                            .unwrap()
                        {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .unwrap();
                if mode == "owner_crash" {
                    let m = std::fs::metadata(&workload_path).unwrap();
                    let marker=serde_json::to_vec(&serde_json::json!({"owner_pid":std::process::id(),"workload":workload_path,"dev":m.dev(),"ino":m.ino(),"guest_started":true,"expires_at":request.expires_at})).unwrap();
                    use std::io::Write;
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(base.join(format!("{marker_prefix}-ready.tmp")))
                        .unwrap();
                    file.write_all(&marker).unwrap();
                    file.sync_all().unwrap();
                    std::fs::rename(
                        base.join(format!("{marker_prefix}-ready.tmp")),
                        base.join(format!("{marker_prefix}-ready.json")),
                    )
                    .unwrap();
                    std::future::pending::<()>().await;
                }
                cancel.store(true, Ordering::Release);
            };
            tokio::pin!(cancel_after_start);
            let execution = task.run(daemon);
            tokio::pin!(execution);
            tokio::select! { result=&mut execution => result, _=&mut cancel_after_start => execution.await }
        };
        if mode == "wait_fault" {
            assert!(
                matches!(outcome, Err(Error::GuestExit)),
                "expected unproven guest status"
            );
            assert!(matches!(
                broker.admit(signed).await.unwrap(),
                Admission::Existing(_)
            ));
            println!(
                "actual_gvisor_mode=wait_fault; guest_exit_unproven=true; no_second_dispatch=true; reservation_retained=true"
            );
            continue;
        }
        let outcome = outcome.unwrap_or_else(|e| panic!("actual mode {mode}: {e:?}"));
        assert!(outcome.control_exit.daemon_process_reaped);
        if !outcome.control_exit.outcome.stderr.bytes.is_empty() {
            eprintln!(
                "own private daemon diagnostics for {mode}: {}",
                String::from_utf8_lossy(&outcome.control_exit.outcome.stderr.bytes)
            );
        }
        assert!(outcome.result.output_complete);
        let receipt = broker
            .complete(
                &request.process_attempt_id,
                &request.request_sha256,
                outcome.result.clone(),
            )
            .await
            .unwrap();
        assert!(matches!(
            receipt.payload.state,
            cowork_runner_protocol::ExecutionState::Completed { .. }
        ));
        match mode {
            "copy" => {
                assert_eq!(outcome.result.exit_code, Some(0));
                assert_eq!(
                    outcome.workspace().unwrap().read("result.txt").unwrap(),
                    data
                );
                assert!(
                    outcome
                        .result
                        .stdout
                        .bytes
                        .starts_with(b"isolated-program-complete\n")
                );
                assert_eq!(outcome.result.stderr.bytes, b"separate-stderr\n");
            }
            "background" => assert_eq!(outcome.result.exit_code, Some(0)),
            "cancel" => assert_eq!(outcome.result.termination, ProcessTermination::Canceled),
            "deadline" | "input_deadline" => {
                assert_eq!(outcome.result.termination, ProcessTermination::TimedOut);
                assert!(outcome.workspace().unwrap().try_exists("started").unwrap());
            }
            "output" => {
                assert!(outcome.output_limit_exceeded);
                assert!(outcome.result.stdout.truncated);
            }
            "disk" => assert_eq!(outcome.result.exit_code, Some(0)),
            "exit128" => assert_eq!(outcome.result.exit_code, Some(128)),
            _ => unreachable!(),
        }
        assert!(matches!(
            broker.admit(signed).await.unwrap(),
            Admission::Existing(_)
        ));
        // Durable physical output is separate from the business receipt.
        println!(
            "actual_gvisor_mode={mode}; task={task_id}; physical_record={}; output_complete=true; reservation_retained=true",
            outcome.physical_record_sha256
        );
    }
    if owner_crash || input_only || backend_only {
        groups.retire_empty().unwrap();
    } else {
        assert!(
            groups.retire_empty().is_err(),
            "uncertain fault boundary must remain booked"
        );
    }
    assert_eq!(
        pool.usage().unwrap().reserved_bytes,
        modes.len() as u64
            * if input_only {
                32 * 1024 * 1024
            } else {
                48 * 1024 * 1024
            }
    );
    if !owner_crash && !input_only && !backend_only {
        prove_owner_sigkill(&base, false);
        prove_owner_sigkill(&base, true);
    }
}

fn prove_owner_sigkill(base: &Path, lease_stop: bool) {
    use std::{
        process::{Command, Stdio},
        time::Instant,
    };
    struct OwnedChild(std::process::Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    let prefix = if lease_stop { "lease" } else { "owner" };
    let stdout = std::fs::File::create(base.join(format!("{prefix}-child.stdout.log"))).unwrap();
    let stderr = std::fs::File::create(base.join(format!("{prefix}-child.stderr.log"))).unwrap();
    let mut child = OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "real_gvisor_copy_descendants_cancel_deadline_output_and_disk_limits",
                "--nocapture",
            ])
            .env(
                "COWORK_TEST_OWNER_CRASH",
                if lease_stop { "lease" } else { "1" },
            )
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .unwrap(),
    );
    let end = Instant::now() + Duration::from_secs(35);
    while !base.join(format!("{prefix}-ready.json")).exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "own runner fixture exited before guest start: {}",
            std::fs::read_to_string(base.join(format!("{prefix}-child.stderr.log"))).unwrap()
        );
        assert!(
            Instant::now() < end,
            "own runner fixture did not report actual guest start"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(base.join(format!("{prefix}-ready.json"))).unwrap())
            .unwrap();
    assert_eq!(
        marker["owner_pid"].as_u64().unwrap(),
        u64::from(child.0.id())
    );
    let path = PathBuf::from(marker["workload"].as_str().unwrap());
    assert!(path.starts_with(env_path("COWORK_RUNTIME_CGROUP_PARENT")));
    let m = std::fs::metadata(&path).unwrap();
    assert_eq!(m.dev(), marker["dev"].as_u64().unwrap());
    assert_eq!(m.ino(), marker["ino"].as_u64().unwrap());
    assert!(
        std::fs::read_to_string(path.join("cgroup.events"))
            .unwrap()
            .lines()
            .any(|s| s == "populated 1")
    );
    let mut frozen_while_stopped = false;
    let end = if lease_stop {
        assert!(
            now() + 1000 < marker["expires_at"].as_i64().unwrap(),
            "guest must start before its initial lease expires"
        );
        assert!(
            !std::fs::read_to_string(path.join("cgroup.events"))
                .unwrap()
                .lines()
                .any(|s| s == "frozen 1")
        );
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, child.0.id(), 0) };
        assert!(fd >= 0);
        let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        assert_eq!(
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd.as_raw_fd(),
                    libc::SIGSTOP,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            },
            0
        );
        let remaining = marker["expires_at"].as_i64().unwrap() - now();
        assert!((0..=30000).contains(&remaining));
        Instant::now() + Duration::from_millis(remaining as u64) + Duration::from_secs(5)
    } else {
        child.0.kill().unwrap();
        let exit = child.0.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(exit.signal(), Some(9));
        Instant::now() + Duration::from_secs(5)
    };
    loop {
        let m = std::fs::metadata(&path).unwrap();
        assert_eq!(m.dev(), marker["dev"].as_u64().unwrap());
        assert_eq!(m.ino(), marker["ino"].as_u64().unwrap());
        let events = std::fs::read_to_string(path.join("cgroup.events")).unwrap();
        if events.lines().any(|s| s == "frozen 1") && events.lines().any(|s| s == "populated 0") {
            if lease_stop {
                let state =
                    std::fs::read_to_string(format!("/proc/{}/status", child.0.id())).unwrap();
                assert!(
                    state
                        .lines()
                        .find(|s| s.starts_with("State:"))
                        .unwrap()
                        .split_whitespace()
                        .nth(1)
                        .is_some_and(|s| s == "T" || s == "t")
                );
                frozen_while_stopped = true;
            }
            break;
        }
        assert!(
            Instant::now() < end,
            "separate fence failed after original runner SIGKILL"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    if lease_stop {
        child.0.kill().unwrap();
        let exit = child.0.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(exit.signal(), Some(9));
    }
    std::fs::write(base.join(format!("{prefix}-observation.json")),serde_json::to_vec(&serde_json::json!({"marker":marker,"owner_sigkill":true,"frozen_while_owner_stopped":frozen_while_stopped,"workload_frozen":true,"workload_population_zero":true,"unknown_reservation_retained":true})).unwrap()).unwrap();
    println!(
        "actual_owner_probe={prefix}; guest_started=true; separate_fence_frozen=true; workload_population_zero=true; frozen_while_owner_stopped={frozen_while_stopped}; unknown_attempt_reservation_retained=true"
    );
}

fn reject_replaceable_package_paths(base: &Path) {
    use std::os::unix::fs::chown;
    let parent = base.join("replaceable-package-parent");
    std::fs::create_dir(&parent).unwrap();
    let driver = parent.join("driver");
    std::fs::copy(env_path("COWORK_TEST_DRIVER_BINARY"), &driver).unwrap();
    std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o555)).unwrap();
    let rootfs = parent.join("rootfs");
    std::fs::create_dir(&rootfs).unwrap();
    std::fs::create_dir(rootfs.join("bin")).unwrap();
    let probe = std::fs::read(env_path("COWORK_TEST_ROOTFS_DIRECTORY").join("bin/probe")).unwrap();
    std::fs::write(rootfs.join("bin/probe"), &probe).unwrap();
    for path in [&rootfs, &rootfs.join("bin"), &rootfs.join("bin/probe")] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o555)).unwrap();
    }
    let manifest = serde_json::to_vec(&RootfsManifest {
        format_version: 1,
        entries: BTreeMap::from([
            ("bin".into(), RootfsEntry::Directory),
            (
                "bin/probe".into(),
                RootfsEntry::File {
                    bytes: probe.len() as u64,
                    sha256: hash(&probe),
                    executable: true,
                },
            ),
        ]),
    })
    .unwrap();
    chown(&parent, Some(1000), Some(1000)).unwrap();
    for mode in [0o755, 0o555] {
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            runsc_driver::RunscDriverExecutable::open(
                &driver,
                &std::env::var("COWORK_TEST_DRIVER_SHA256").unwrap()
            )
            .is_err()
        );
        assert!(SealedRootfs::open(&rootfs, &manifest, &hash(&manifest)).is_err());
    }
    std::fs::write(base.join("package-path-observation.json"), b"{\"driver_uid1000_parent_0755_rejected\":true,\"driver_uid1000_parent_0555_rejected\":true,\"rootfs_uid1000_parent_0755_rejected\":true,\"rootfs_uid1000_parent_0555_rejected\":true}").unwrap();
}
