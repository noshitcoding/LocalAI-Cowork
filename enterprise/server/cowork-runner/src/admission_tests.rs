use super::*;

#[tokio::test]
async fn renamed_or_uninspectable_known_ids_never_release_admission_capacity() {
    let fixture = Fixture::new();
    let mut known = reservation();
    known.container_id = Some("f".repeat(64));
    fixture.gate.save(&known).unwrap();
    let run = known.run_id;
    let admission = known.id;
    let resources = known.resources;
    assert!(fixture
        .gate
        .verify_retained_ids(&HashMap::new(), move |id| async move {
            Ok(Some(Container {
                id,
                name: "renamed-outside-scope".into(),
                run_id: run,
                reservation_id: Some(admission),
                resources,
            }))
        })
        .await
        .is_err());
    assert_eq!(fixture.gate.load().unwrap().len(), 1);
    assert!(fixture
        .gate
        .verify_retained_ids(&HashMap::new(), |_| async { bail!("daemon unavailable") })
        .await
        .is_err());
    assert_eq!(fixture.gate.load().unwrap().len(), 1);
    fixture
        .gate
        .verify_retained_ids(&HashMap::new(), |_| async { Ok(None) })
        .await
        .unwrap();
    assert!(fixture
        .gate
        .reconcile(fixture.gate.load().unwrap(), &HashMap::new())
        .unwrap()
        .is_empty());
}

struct Fixture {
    gate: Admission,
}
impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("cowork-admission-test-{}", Uuid::new_v4()));
        Self {
            gate: Admission::new(directory, "cowork-test-".into(), limits()).unwrap(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.gate.directory);
    }
}

fn limits() -> Limits {
    Limits {
        cpu_nanos: 4_000_000_000,
        memory_bytes: 8 * 1024 * 1024 * 1024,
        concurrent_runs: 2,
        containers: 4,
    }
}
fn small() -> Resources {
    Resources {
        cpu_nanos: 1_000_000_000,
        memory_bytes: 1024 * 1024 * 1024,
    }
}
fn reservation() -> Reservation {
    Reservation {
        id: Uuid::new_v4(),
        container_name: "cowork-test-job-test".into(),
        run_id: Uuid::new_v4(),
        resources: small(),
        container_id: None,
    }
}

#[test]
fn aggregate_cpu_memory_runs_and_container_count_are_independent() {
    let run = Uuid::new_v4();
    let other = Uuid::new_v4();
    let usage = [(run, small()), (other, small())];
    assert!(check_capacity(&limits(), &usage, run, small()).is_ok());
    assert_eq!(
        check_capacity(&limits(), &usage, Uuid::new_v4(), small())
            .unwrap_err()
            .status,
        axum::http::StatusCode::TOO_MANY_REQUESTS
    );
    let mut requested = small();
    requested.cpu_nanos = 3_000_000_000;
    assert!(check_capacity(&limits(), &usage, run, requested).is_err());
    requested = small();
    requested.memory_bytes = 7 * 1024 * 1024 * 1024;
    assert!(check_capacity(&limits(), &usage, run, requested).is_err());
    assert!(check_capacity(&limits(), &[(run, small()); 4], run, small()).is_err());
    requested.memory_bytes = 9 * 1024 * 1024 * 1024;
    assert_eq!(
        check_capacity(&limits(), &[], run, requested)
            .unwrap_err()
            .status,
        axum::http::StatusCode::BAD_REQUEST
    );
}

#[test]
fn lost_launch_response_survives_reopening_and_empty_docker_inventory() {
    let fixture = Fixture::new();
    let pending = reservation();
    fixture.gate.save(&pending).unwrap();
    let reopened = Admission::new(
        fixture.gate.directory.clone(),
        "cowork-test-".into(),
        limits(),
    )
    .unwrap();
    let retained = reopened
        .reconcile(reopened.load().unwrap(), &HashMap::new())
        .unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].id, pending.id);
    assert!(retained[0].container_id.is_none());
}

#[test]
fn observed_container_identity_survives_restart_and_only_absence_releases_it() {
    let fixture = Fixture::new();
    let pending = reservation();
    fixture.gate.save(&pending).unwrap();
    let container = Container {
        id: "docker-id".into(),
        name: pending.container_name.clone(),
        run_id: pending.run_id,
        reservation_id: Some(pending.id),
        resources: pending.resources,
    };
    let inventory = HashMap::from([(container.name.clone(), container)]);
    let retained = fixture
        .gate
        .reconcile(fixture.gate.load().unwrap(), &inventory)
        .unwrap();
    assert_eq!(retained[0].container_id.as_deref(), Some("docker-id"));
    assert_eq!(
        fixture.gate.load().unwrap()[0].container_id.as_deref(),
        Some("docker-id")
    );
    assert!(fixture
        .gate
        .reconcile(fixture.gate.load().unwrap(), &HashMap::new())
        .unwrap()
        .is_empty());
    assert!(fixture.gate.load().unwrap().is_empty());
}

#[test]
fn modified_container_caps_or_identity_do_not_release_capacity() {
    let fixture = Fixture::new();
    let pending = reservation();
    fixture.gate.save(&pending).unwrap();
    for variant in 0..4 {
        let mut container = Container {
            id: "docker-id".into(),
            name: pending.container_name.clone(),
            run_id: pending.run_id,
            reservation_id: Some(pending.id),
            resources: pending.resources,
        };
        match variant {
            0 => container.reservation_id = Some(Uuid::new_v4()),
            1 => container.run_id = Uuid::new_v4(),
            2 => container.resources.cpu_nanos += 1,
            _ => container.resources.memory_bytes += 1,
        }
        assert!(fixture
            .gate
            .reconcile(
                fixture.gate.load().unwrap(),
                &HashMap::from([(container.name.clone(), container)])
            )
            .is_err());
        assert_eq!(fixture.gate.load().unwrap().len(), 1);
    }
}

#[test]
fn corrupt_records_and_cross_scope_state_fail_closed() {
    let fixture = Fixture::new();
    assert!(Admission::new(
        fixture.gate.directory.clone(),
        "cowork-other-".into(),
        limits()
    )
    .is_err());
    fs::write(
        fixture
            .gate
            .directory
            .join(format!("{}.json", Uuid::new_v4())),
        b"broken",
    )
    .unwrap();
    assert!(fixture.gate.load().is_err());
    assert!(!fixture.gate.owns("cowork-other-job-a"));
    assert!(!fixture.gate.owns("cowork-test-run-a"));
    assert!(fixture.gate.owns("cowork-test-transfer-a"));
}

#[tokio::test]
async fn acknowledged_creation_is_durable_and_cannot_change_identity() {
    let fixture = Fixture::new();
    let pending = reservation();
    fixture.gate.save(&pending).unwrap();
    let permit = Permit {
        reservation: pending,
    };
    fixture.gate.created(&permit, "a".repeat(64)).await.unwrap();
    assert_eq!(
        fixture.gate.load().unwrap()[0].container_id,
        Some("a".repeat(64))
    );
    assert!(fixture.gate.created(&permit, "b".repeat(64)).await.is_err());
    assert_eq!(
        fixture.gate.load().unwrap()[0].container_id,
        Some("a".repeat(64))
    );
}

#[tokio::test]
async fn separate_runner_objects_share_an_exclusive_persistent_lock() {
    let fixture = Fixture::new();
    let first = fixture.gate.lock().await.unwrap();
    let other = Admission {
        directory: fixture.gate.directory.clone(),
        prefix: "cowork-test-".into(),
        limits: limits(),
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), other.lock())
            .await
            .is_err()
    );
    drop(first);
    assert!(tokio::time::timeout(Duration::from_secs(1), other.lock())
        .await
        .unwrap()
        .is_ok());
}

#[test]
fn existing_unlimited_or_unidentified_containers_block_admission() {
    let value = serde_json::json!({"Id":"docker-id", "Name":"/cowork-test-job-a", "HostConfig":{"NanoCpus":1000000000, "Memory":1073741824}, "Config":{"Labels":{"dev.opencowork.run_id":Uuid::new_v4().to_string()}}});
    assert!(parse_container(&value).is_ok());
    let mut invalid = value.clone();
    invalid["HostConfig"]["Memory"] = 0.into();
    assert!(parse_container(&invalid).is_err());
    invalid = value;
    invalid["Config"]["Labels"] = serde_json::json!({});
    assert!(parse_container(&invalid).is_err());
}
