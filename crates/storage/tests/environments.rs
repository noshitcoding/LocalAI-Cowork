use cowork_contracts::CreateProject;
use cowork_domain::{Error, canonical_hash, environments::*};
use cowork_storage::{Store, now};
use std::collections::BTreeMap;

fn definition(release: char) -> EnvironmentDefinition {
    EnvironmentDefinition {
        format_version: 1,
        network: None,
        artifact: EnvironmentArtifact::LinuxImage {
            reference: format!(
                "registry.example.test/cowork/tools@sha256:{}",
                release.to_string().repeat(64)
            ),
        },
        programs: BTreeMap::from([("python".into(), "usr/bin/python3".into())]),
        packages: vec![EnvironmentPackage {
            name: "python".into(),
            version: "3.12.10".into(),
            source_sha256: "a".repeat(64),
        }],
        limits: EnvironmentLimits {
            cpu_millis: 1000,
            memory_bytes: 256 * 1024 * 1024,
            processes: 64,
            scratch_bytes: 64 * 1024 * 1024,
        },
    }
}
fn qualification(d: &EnvironmentDefinition) -> EnvironmentQualification {
    // Synthetic adapter evidence proves storage/admission behavior, not sandbox isolation.
    EnvironmentQualification {
        runner_id: "test-runner".into(),
        runner_generation: 3,
        adapter_build_sha256: "d".repeat(64),
        artifact_sha256: d.artifact.sha256().into(),
        definition_sha256: canonical_hash(d),
        report_sha256: "e".repeat(64),
        capabilities: d.required_capabilities(),
        verified_at: now(),
    }
}
fn observation(q: &EnvironmentQualification) -> EnvironmentObservation {
    EnvironmentObservation {
        runner_id: q.runner_id.clone(),
        runner_generation: q.runner_generation,
        adapter_build_sha256: q.adapter_build_sha256.clone(),
        artifact_sha256: q.artifact_sha256.clone(),
        definition_sha256: q.definition_sha256.clone(),
        capabilities: q.capabilities.clone(),
        observed_at: now(),
    }
}
async fn setup(store: &Store) -> EnvironmentRecord {
    let p = store
        .create_project(CreateProject {
            name: "Environment fixture".into(),
        })
        .await
        .unwrap();
    store
        .create_environment(&p.id, "Reviewable tools", "create")
        .await
        .unwrap()
}
async fn change(
    store: &Store,
    e: &mut EnvironmentRecord,
    key: &str,
    c: EnvironmentChange,
) -> EnvironmentReceipt {
    let result = store
        .change_environment(&e.project_id, &e.id, e.revision, key, &c)
        .await
        .unwrap();
    *e = store.environment(&e.project_id, &e.id).await.unwrap();
    result
}
async fn install(
    store: &Store,
    e: &mut EnvironmentRecord,
    release: char,
    key: &str,
) -> (String, EnvironmentQualification) {
    let d = definition(release);
    let receipt = change(
        store,
        e,
        &format!("stage-{key}"),
        EnvironmentChange::Stage {
            definition: d.clone(),
        },
    )
    .await;
    let v = receipt.version_id.unwrap();
    let q = qualification(&d);
    change(
        store,
        e,
        &format!("qualify-{key}"),
        EnvironmentChange::Qualify {
            version_id: v.clone(),
            qualification: q.clone(),
        },
    )
    .await;
    change(
        store,
        e,
        &format!("activate-{key}"),
        EnvironmentChange::Activate {
            version_id: v.clone(),
            observation: observation(&q),
        },
    )
    .await;
    (v, q)
}

async fn shared_contract(store: &Store) -> EnvironmentPin {
    let mut e = setup(store).await;
    assert_eq!(
        store
            .create_environment(&e.project_id, &e.name, "create")
            .await
            .unwrap()
            .id,
        e.id
    );
    assert!(matches!(
        store
            .create_environment(&e.project_id, "Different", "create")
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store.create_environment("missing-project", "x", "x").await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store.environment("foreign-project", &e.id).await,
        Err(Error::NotFound)
    ));
    let d = definition('a');
    let staged = change(
        store,
        &mut e,
        "first",
        EnvironmentChange::Stage {
            definition: d.clone(),
        },
    )
    .await;
    let first = staged.version_id.clone().unwrap();
    let q = qualification(&d);
    assert!(
        store
            .pin_environment(&e.project_id, &e.id, &observation(&q))
            .await
            .is_err()
    );
    let snapshot = e.clone();
    let activate = EnvironmentChange::Activate {
        version_id: first.clone(),
        observation: observation(&q),
    };
    assert!(
        store
            .change_environment(&e.project_id, &e.id, e.revision, "too-early", &activate)
            .await
            .is_err()
    );
    assert_eq!(
        store.environment(&e.project_id, &e.id).await.unwrap(),
        snapshot
    );
    let mut incomplete = q.clone();
    incomplete
        .capabilities
        .remove(&IsolationCapability::LinuxGvisor);
    assert!(
        store
            .change_environment(
                &e.project_id,
                &e.id,
                e.revision,
                "unqualified",
                &EnvironmentChange::Qualify {
                    version_id: first.clone(),
                    qualification: incomplete
                }
            )
            .await
            .is_err()
    );
    change(
        store,
        &mut e,
        "qualified",
        EnvironmentChange::Qualify {
            version_id: first.clone(),
            qualification: q.clone(),
        },
    )
    .await;
    let activate_revision = e.revision;
    let activated = change(store, &mut e, "activated", activate.clone()).await;
    let pin = store
        .pin_environment(&e.project_id, &e.id, &observation(&q))
        .await
        .unwrap();
    assert_eq!(pin.definition, d);
    let (second, q2) = install(store, &mut e, 'b', "second").await;
    assert_eq!(e.active_version_id.as_ref(), Some(&second));
    assert_ne!(
        pin.definition_sha256,
        store
            .pin_environment(&e.project_id, &e.id, &observation(&q2))
            .await
            .unwrap()
            .definition_sha256
    );
    // An old confirmed request is a receipt lookup, never an activation rollback.
    assert_eq!(
        store
            .change_environment(
                &e.project_id,
                &e.id,
                activate_revision,
                "activated",
                &activate
            )
            .await
            .unwrap(),
        activated
    );
    assert_eq!(store.environment(&e.project_id, &e.id).await.unwrap(), e);
    store
        .check_environment_pin(&e.project_id, &pin, &observation(&q))
        .await
        .unwrap();
    assert!(matches!(
        store
            .check_environment_pin("foreign-project", &pin, &observation(&q))
            .await,
        Err(Error::Forbidden)
    ));
    let mut forged = pin.clone();
    forged.definition.limits.memory_bytes *= 2;
    assert!(
        store
            .check_environment_pin(&e.project_id, &forged, &observation(&q))
            .await
            .is_err()
    );
    // A failed installation does not modify the working version or a prior pin.
    let rejected = change(
        store,
        &mut e,
        "third",
        EnvironmentChange::Stage {
            definition: definition('c'),
        },
    )
    .await
    .version_id
    .unwrap();
    change(
        store,
        &mut e,
        "third-failed",
        EnvironmentChange::Reject {
            version_id: rejected.clone(),
            reason_code: "installation_failed".into(),
        },
    )
    .await;
    assert_eq!(e.active_version_id.as_ref(), Some(&second));
    assert!(
        store
            .change_environment(
                &e.project_id,
                &e.id,
                e.revision,
                "resurrect",
                &EnvironmentChange::Qualify {
                    version_id: rejected,
                    qualification: qualification(&definition('c'))
                }
            )
            .await
            .is_err()
    );
    // Revocation affects old pins even after another environment version is active.
    change(
        store,
        &mut e,
        "revoke-first",
        EnvironmentChange::RevokeVersion { version_id: first },
    )
    .await;
    assert!(
        store
            .check_environment_pin(&e.project_id, &pin, &observation(&q))
            .await
            .is_err()
    );
    let second_pin = store
        .pin_environment(&e.project_id, &e.id, &observation(&q2))
        .await
        .unwrap();
    change(store, &mut e, "revoke-all", EnvironmentChange::Revoke).await;
    assert!(
        store
            .pin_environment(&e.project_id, &e.id, &observation(&q2))
            .await
            .is_err()
    );
    assert!(
        store
            .check_environment_pin(&e.project_id, &second_pin, &observation(&q2))
            .await
            .is_err()
    );
    assert!(
        store
            .change_environment(
                &e.project_id,
                &e.id,
                e.revision,
                "late-stage",
                &EnvironmentChange::Stage {
                    definition: definition('d')
                }
            )
            .await
            .is_err()
    );
    second_pin
}

#[tokio::test]
async fn sqlite_environment_contract_survives_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("env.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let pin = shared_contract(&store).await;
    let before = store
        .environment(&pin.project_id, &pin.environment_id)
        .await
        .unwrap();
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        store
            .environment(&pin.project_id, &pin.environment_id)
            .await
            .unwrap(),
        before
    );
}

#[tokio::test]
#[ignore = "Requires a dedicated PostgreSQL database in COWORK_TEST_POSTGRES"]
async fn postgres_environment_contract() {
    let store = Store::connect(&std::env::var("COWORK_TEST_POSTGRES").unwrap())
        .await
        .unwrap();
    shared_contract(&store).await;
    concurrent_contract(&store, &store).await;
    store.close().await;
}

async fn concurrent_contract(a: &Store, b: &Store) {
    let mut e = setup(a).await;
    let (created_one, created_two) = tokio::join!(
        a.create_environment(&e.project_id, "Concurrent registration", "parallel-create"),
        b.create_environment(&e.project_id, "Concurrent registration", "parallel-create")
    );
    assert_eq!(created_one.unwrap(), created_two.unwrap());
    let c = EnvironmentChange::Stage {
        definition: definition('a'),
    };
    let (one, two) = tokio::join!(
        a.change_environment(&e.project_id, &e.id, 1, "same", &c),
        b.change_environment(&e.project_id, &e.id, 1, "same", &c)
    );
    assert_eq!(one.unwrap(), two.unwrap());
    e = a.environment(&e.project_id, &e.id).await.unwrap();
    assert_eq!(e.versions.len(), 1);
    assert_eq!(e.receipts.len(), 1);
    let (one, two) = tokio::join!(
        a.change_environment(&e.project_id, &e.id, 2, "competing-one", &c),
        b.change_environment(&e.project_id, &e.id, 2, "competing-two", &c)
    );
    assert!(matches!(
        (&one, &two),
        (Ok(_), Err(Error::Conflict)) | (Err(Error::Conflict), Ok(_))
    ));
    e = a.environment(&e.project_id, &e.id).await.unwrap();
    assert_eq!(e.versions.len(), 2);
    assert_eq!(e.receipts.len(), 2);
    assert_eq!(e.revision, 3);
    let altered = EnvironmentChange::Stage {
        definition: definition('b'),
    };
    assert!(matches!(
        a.change_environment(&e.project_id, &e.id, 1, "same", &altered)
            .await,
        Err(Error::Conflict)
    ));
}
#[tokio::test]
async fn independent_sqlite_connections_serialize_and_deduplicate_changes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("env.sqlite3");
    let a = Store::sqlite(&path).await.unwrap();
    let b = Store::sqlite(&path).await.unwrap();
    concurrent_contract(&a, &b).await;
}
#[tokio::test]
async fn failed_database_commit_keeps_active_definition_and_transition_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("env.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let mut e = setup(&store).await;
    let (_, q) = install(&store, &mut e, 'a', "initial").await;
    let pin = store
        .pin_environment(&e.project_id, &e.id, &observation(&q))
        .await
        .unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    sqlx::query("CREATE TRIGGER reject_environment_update BEFORE UPDATE OF payload ON environments BEGIN SELECT RAISE(ABORT,'fixture commit rejected'); END").execute(&pool).await.unwrap();
    assert!(
        store
            .change_environment(
                &e.project_id,
                &e.id,
                e.revision,
                "unconfirmed",
                &EnvironmentChange::Stage {
                    definition: definition('b')
                }
            )
            .await
            .is_err()
    );
    assert_eq!(store.environment(&e.project_id, &e.id).await.unwrap(), e);
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    store
        .check_environment_pin(&e.project_id, &pin, &observation(&q))
        .await
        .unwrap();
    assert_eq!(store.environment(&e.project_id, &e.id).await.unwrap(), e);
    pool.close().await;
}
#[tokio::test]
async fn altered_manifest_or_index_is_rejected_without_repairing_history() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("env.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let mut e = setup(&store).await;
    install(&store, &mut e, 'a', "initial").await;
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let original = serde_json::to_string(&e).unwrap();
    e.versions[0].definition.limits.processes += 1;
    let corrupt = serde_json::to_string(&e).unwrap();
    sqlx::query("UPDATE environments SET payload=$1 WHERE id=$2")
        .bind(&corrupt)
        .bind(&e.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.environment(&e.project_id, &e.id).await.is_err());
    assert!(
        store
            .change_environment(
                &e.project_id,
                &e.id,
                e.revision,
                "blocked",
                &EnvironmentChange::Revoke
            )
            .await
            .is_err()
    );
    let after: String = sqlx::query_scalar("SELECT payload FROM environments WHERE id=$1")
        .bind(&e.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, corrupt);
    sqlx::query("UPDATE environments SET payload=$1,revision=revision+1 WHERE id=$2")
        .bind(original)
        .bind(&e.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.environment(&e.project_id, &e.id).await.is_err());
    pool.close().await;
}

#[tokio::test]
async fn launch_recheck_rejects_stale_observation_runner_changes_and_lost_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&temp.path().join("env.sqlite3"))
        .await
        .unwrap();
    let mut e = setup(&store).await;
    let (_, q) = install(&store, &mut e, 'a', "initial").await;
    let base = observation(&q);
    let pin = store
        .pin_environment(&e.project_id, &e.id, &base)
        .await
        .unwrap();
    let mut variants = vec![];
    let mut o = base.clone();
    o.runner_id = "different-runner".into();
    variants.push(o);
    let mut o = base.clone();
    o.runner_generation += 1;
    variants.push(o);
    let mut o = base.clone();
    o.adapter_build_sha256 = "1".repeat(64);
    variants.push(o);
    let mut o = base.clone();
    o.artifact_sha256 = "2".repeat(64);
    variants.push(o);
    let mut o = base.clone();
    o.definition_sha256 = "3".repeat(64);
    variants.push(o);
    let mut o = base.clone();
    o.observed_at -= 31_000;
    variants.push(o);
    let mut o = base.clone();
    o.observed_at += 31_000;
    variants.push(o);
    for c in &base.capabilities {
        let mut o = base.clone();
        o.capabilities.remove(c);
        variants.push(o);
    }
    for bad in variants {
        assert!(
            store
                .check_environment_pin(&e.project_id, &pin, &bad)
                .await
                .is_err()
        );
    }
    store
        .check_environment_pin(&e.project_id, &pin, &observation(&q))
        .await
        .unwrap();
}

#[test]
fn manifests_are_pinned_bounded_and_cannot_supply_host_paths_or_secrets() {
    let good = definition('a');
    good.validate().unwrap();
    let mut bad = good.clone();
    bad.format_version = 2;
    assert!(bad.validate().is_err());
    for r in [
        "busybox:latest",
        "https://user:secret@example.test/image",
        "repo@sha256:bad",
        "/repo@sha256:bad",
    ] {
        let mut bad = good.clone();
        bad.artifact = EnvironmentArtifact::LinuxImage {
            reference: r.into(),
        };
        assert!(bad.validate().is_err());
    }
    for path in [
        "/usr/bin/python",
        "../python",
        "C:\\Windows\\cmd.exe",
        "bin/../python",
        "bin//python",
        "bin/python:stream",
        "NUL.exe",
        "bin/python --option",
        "bin/con.txt",
        "bin/python.",
    ] {
        let mut bad = good.clone();
        bad.programs.insert("python".into(), path.into());
        assert!(bad.validate().is_err(), "{path}");
    }
    let mut bad = good.clone();
    bad.limits.processes = 0;
    assert!(bad.validate().is_err());
    let mut bad = good.clone();
    bad.packages.push(bad.packages[0].clone());
    assert!(bad.validate().is_err());
    let mut wire = serde_json::to_value(&good).unwrap();
    wire["environment"] = serde_json::json!({"API_KEY":"synthetic"});
    assert!(serde_json::from_value::<EnvironmentDefinition>(wire).is_err());
    let mut win = good;
    win.artifact = EnvironmentArtifact::WindowsBundle {
        sha256: "b".repeat(64),
    };
    win.validate().unwrap();
    for cap in [
        IsolationCapability::WindowsRestrictedIdentity,
        IsolationCapability::WindowsPrivateDesktop,
        IsolationCapability::WindowsAclRecovery,
        IsolationCapability::WindowsAclSerialization,
    ] {
        assert!(win.required_capabilities().contains(&cap));
    }
    assert!(
        !win.required_capabilities()
            .contains(&IsolationCapability::LinuxGvisor)
    );
}

#[tokio::test]
#[ignore = "Synthetic child; invoked by confirmed_environment_survives_hard_process_end"]
async fn environment_process_fixture() {
    use std::io::Write;
    let directory = std::path::PathBuf::from(
        std::env::var_os("COWORK_ENVIRONMENT_FIXTURE").expect("parent fixture directory"),
    );
    let store = Store::sqlite(&directory.join("env.sqlite3")).await.unwrap();
    let mut e = setup(&store).await;
    install(&store, &mut e, 'a', "process").await;
    let mut marker = std::fs::File::create(directory.join("confirmed.json")).unwrap();
    marker.write_all(&serde_json::to_vec(&e).unwrap()).unwrap();
    marker.sync_all().unwrap();
    drop(marker);
    // Keep the database process open until the parent simulates a crash.
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
}

#[test]
fn confirmed_environment_survives_hard_process_end() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "environment_process_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("COWORK_ENVIRONMENT_FIXTURE", temp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Child(command.spawn().unwrap());
    let until = Instant::now() + Duration::from_secs(10);
    let confirmed: EnvironmentRecord = loop {
        if let Ok(bytes) = std::fs::read(temp.path().join("confirmed.json"))
            && let Ok(record) = serde_json::from_slice(&bytes)
        {
            break record;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child exited before acknowledgement"
        );
        assert!(
            Instant::now() < until,
            "child did not acknowledge activation"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let store = Store::sqlite(&temp.path().join("env.sqlite3"))
            .await
            .unwrap();
        let actual = store
            .environment(&confirmed.project_id, &confirmed.id)
            .await
            .unwrap();
        assert_eq!(actual, confirmed);
        assert!(actual.active_version_id.is_some());
        assert_eq!(actual.receipts.len(), 3);
        store.close().await;
    });
}

#[tokio::test]
async fn full_history_and_clock_rollback_cannot_prevent_terminal_revocation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("env.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let mut e = setup(&store).await;
    let (_, q) = install(&store, &mut e, 'a', "initial").await;
    let pin = store
        .pin_environment(&e.project_id, &e.id, &observation(&q))
        .await
        .unwrap();
    // Prepare the bounded journal's limit directly; every fixture entry is a valid
    // activation of the already-qualified version. No user data is involved.
    let template = e.receipts.last().unwrap().clone();
    for i in e.receipts.len()..2048 {
        let mut receipt = template.clone();
        receipt.key = format!("capacity-{i}");
        receipt.revision = i as i64 + 2;
        receipt.request_sha256 = canonical_hash(&("synthetic activation", i));
        e.receipts.push(receipt);
    }
    e.revision = e.receipts.len() as i64 + 1;
    e.validate().unwrap();
    let mut corrupt = e.clone();
    corrupt.active_version_id = None;
    assert!(corrupt.validate().is_err());
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    sqlx::query("UPDATE environments SET revision=$1,payload=$2 WHERE id=$3")
        .bind(e.revision)
        .bind(serde_json::to_string(&e).unwrap())
        .bind(&e.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        store
            .change_environment(
                &e.project_id,
                &e.id,
                e.revision,
                "full",
                &EnvironmentChange::Stage {
                    definition: definition('b')
                }
            )
            .await
            .is_err()
    );
    let mut clock_copy = e.clone();
    let previous_time = e.receipts.last().unwrap().created_at - 1;
    assert!(
        clock_copy
            .apply(
                e.revision,
                "clock",
                &EnvironmentChange::Stage {
                    definition: definition('b')
                },
                "new-version",
                previous_time
            )
            .is_err()
    );
    clock_copy
        .apply(
            e.revision,
            "stop",
            &EnvironmentChange::Revoke,
            "unused",
            previous_time,
        )
        .unwrap();
    assert!(clock_copy.revoked);
    change(&store, &mut e, "emergency-stop", EnvironmentChange::Revoke).await;
    assert_eq!(e.receipts.len(), 2049);
    assert!(
        store
            .check_environment_pin(&e.project_id, &pin, &observation(&q))
            .await
            .is_err()
    );
    let mut corrupt = e.clone();
    corrupt.revoked = false;
    assert!(corrupt.validate().is_err());
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(store.environment(&e.project_id, &e.id).await.unwrap(), e);
    pool.close().await;
}
