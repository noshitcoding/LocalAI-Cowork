//! Stop authority is durable before Docker I/O. Unknown creates retain capacity.
use super::*;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Execution {
    pub version: u8,
    pub attempt_id: Uuid,
    pub lease_token: Uuid,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Fixture {
        gate: Admission,
    }
    impl Fixture {
        fn new() -> Self {
            Self {
                gate: Admission::new(
                    std::env::temp_dir().join(format!("cowork-stop-test-{}", Uuid::new_v4())),
                    "cowork-test-".into(),
                    Limits {
                        cpu_nanos: 4_000_000_000,
                        memory_bytes: 8 * 1024 * 1024 * 1024,
                        concurrent_runs: 2,
                        containers: 4,
                    },
                )
                .unwrap(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.gate.directory);
        }
    }

    fn execution() -> Execution {
        Execution {
            version: 1,
            attempt_id: Uuid::new_v4(),
            lease_token: Uuid::new_v4(),
        }
    }
    fn request(run_id: Uuid, execution: &Execution, lease: Option<Uuid>) -> StopRequest {
        StopRequest {
            version: 1,
            run_id,
            lease_token: lease,
            cancel_lease: None,
            attempts: vec![ExpectedAttempt {
                attempt_id: execution.attempt_id,
                lease_token: execution.lease_token,
                request_sha256: hex::encode(Sha256::digest(b"body")),
                workspace_id: None,
            }],
        }
    }
    fn pending(run_id: Uuid) -> Reservation {
        Reservation {
            id: Uuid::new_v4(),
            container_name: format!("cowork-test-job-{}", run_id.simple()),
            run_id,
            resources: Resources {
                cpu_nanos: 1_000_000_000,
                memory_bytes: 1024 * 1024 * 1024,
            },
            container_id: None,
        }
    }
    fn inventory(reservation: &Reservation) -> HashMap<String, Container> {
        HashMap::from([(
            reservation.container_name.clone(),
            Container {
                id: reservation.container_id.clone().unwrap(),
                name: reservation.container_name.clone(),
                run_id: reservation.run_id,
                reservation_id: Some(reservation.id),
                resources: reservation.resources,
            },
        )])
    }
    async fn stop_fake(
        gate: &Admission,
        request: &StopRequest,
        live: Arc<Mutex<HashMap<String, Container>>>,
        removed: Arc<Mutex<Vec<String>>>,
        confirm: bool,
    ) -> Result<serde_json::Value, RunnerError> {
        let listed = live.clone();
        let exact = live.clone();
        gate.stop_execution_with(
            request,
            move || {
                let listed = listed.clone();
                async move {
                    // Move snapshots out and reconstruct without making production
                    // Container clonable merely for a fake daemon.
                    let live = listed.lock().unwrap();
                    Ok(live
                        .iter()
                        .filter(|(name, _)| name.starts_with("cowork-test-"))
                        .map(|(name, container)| {
                            (
                                name.clone(),
                                Container {
                                    id: container.id.clone(),
                                    name: container.name.clone(),
                                    run_id: container.run_id,
                                    reservation_id: container.reservation_id,
                                    resources: container.resources,
                                },
                            )
                        })
                        .collect())
                }
            },
            move |id| {
                let live = live.clone();
                let removed = removed.clone();
                async move {
                    removed.lock().unwrap().push(id.clone());
                    if confirm {
                        live.lock()
                            .unwrap()
                            .retain(|_, container| container.id != id);
                    }
                    Ok(())
                }
            },
            move |id| {
                let exact = exact.clone();
                async move {
                    Ok(exact
                        .lock()
                        .unwrap()
                        .values()
                        .find(|container| container.id == id)
                        .map(|container| Container {
                            id: container.id.clone(),
                            name: container.name.clone(),
                            run_id: container.run_id,
                            reservation_id: container.reservation_id,
                            resources: container.resources,
                        }))
                }
            },
        )
        .await
    }

    #[tokio::test]
    async fn delayed_post_and_create_are_fenced_before_a_no_start_stop_receipt() {
        let fixture = Fixture::new();
        let run = Uuid::new_v4();
        let execution = execution();
        let request = request(run, &execution, None);
        let receipt = stop_fake(
            &fixture.gate,
            &request,
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(Vec::new())),
            true,
        )
        .await
        .unwrap();
        assert_eq!(receipt["execution_stopped"], true);
        assert!(fixture
            .gate
            .begin_execution(run, None, &execution, b"body")
            .await
            .is_err());
        // This production reserve fails before consulting Docker.
        assert!(fixture
            .gate
            .reserve_execution(
                pending(run).container_name,
                run,
                pending(run).resources,
                Some(&execution)
            )
            .await
            .is_err());
        let reopened = Admission::new(
            fixture.gate.directory.clone(),
            fixture.gate.prefix.clone(),
            fixture.gate.limits.clone(),
        )
        .unwrap();
        assert!(reopened
            .check_execution_open(run, Some(Uuid::new_v4()))
            .is_err());
    }

    #[tokio::test]
    async fn unknown_create_never_mints_stop_proof_or_releases_reservation() {
        let fixture = Fixture::new();
        let run = Uuid::new_v4();
        let execution = execution();
        fixture
            .gate
            .begin_execution(run, None, &execution, b"body")
            .await
            .unwrap();
        let reservation = pending(run);
        fixture.gate.save(&reservation).unwrap();
        fixture
            .gate
            .link_execution(&execution, &reservation)
            .unwrap();
        let removed = Arc::new(Mutex::new(Vec::new()));
        assert!(stop_fake(
            &fixture.gate,
            &request(run, &execution, Some(execution.lease_token)),
            Arc::new(Mutex::new(HashMap::new())),
            removed.clone(),
            true
        )
        .await
        .is_err());
        assert!(removed.lock().unwrap().is_empty());
        assert!(fixture.gate.load().unwrap()[0].container_id.is_none());
        // A delayed create acknowledgment is recorded but cannot start.
        let permit = Permit {
            reservation: reservation.clone(),
        };
        assert!(fixture.gate.created(&permit, "a".repeat(64)).await.is_err());
        let acknowledged = fixture.gate.load().unwrap().remove(0);
        assert_eq!(
            acknowledged.container_id.as_deref(),
            Some("a".repeat(64).as_str())
        );
        let receipt = stop_fake(
            &fixture.gate,
            &request(run, &execution, Some(execution.lease_token)),
            Arc::new(Mutex::new(inventory(&acknowledged))),
            removed.clone(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(receipt["containers"][0]["container_id"], "a".repeat(64));
        assert_eq!(removed.lock().unwrap().as_slice(), ["a".repeat(64)]);
        assert!(fixture.gate.load().unwrap().is_empty());
    }

    #[tokio::test]
    async fn exact_stop_requires_physical_absence_and_remains_idempotent_after_restart() {
        let fixture = Fixture::new();
        let run = Uuid::new_v4();
        let execution = execution();
        fixture
            .gate
            .begin_execution(run, None, &execution, b"body")
            .await
            .unwrap();
        let mut reservation = pending(run);
        fixture.gate.save(&reservation).unwrap();
        fixture
            .gate
            .link_execution(&execution, &reservation)
            .unwrap();
        fixture
            .gate
            .created(
                &Permit {
                    reservation: reservation.clone(),
                },
                "b".repeat(64),
            )
            .await
            .unwrap();
        reservation.container_id = Some("b".repeat(64));
        let live = Arc::new(Mutex::new(inventory(&reservation)));
        let removed = Arc::new(Mutex::new(Vec::new()));
        let request = request(run, &execution, None);
        assert!(stop_fake(
            &fixture.gate,
            &request,
            live.clone(),
            removed.clone(),
            false
        )
        .await
        .is_err());
        assert_eq!(fixture.gate.load().unwrap().len(), 1);
        let first = stop_fake(&fixture.gate, &request, live.clone(), removed.clone(), true)
            .await
            .unwrap();
        let reopened = Admission::new(
            fixture.gate.directory.clone(),
            fixture.gate.prefix.clone(),
            fixture.gate.limits.clone(),
        )
        .unwrap();
        let again = stop_fake(&reopened, &request, live, removed.clone(), true)
            .await
            .unwrap();
        assert_eq!(first, again);
        assert_eq!(
            removed.lock().unwrap().len(),
            2,
            "idempotent retry must not issue another rm after absence"
        );
    }

    #[tokio::test]
    async fn old_lease_fence_does_not_stop_successor_and_changed_container_identity_is_refused() {
        let fixture = Fixture::new();
        let run = Uuid::new_v4();
        let old = execution();
        let successor = execution();
        fixture
            .gate
            .begin_execution(run, None, &old, b"body")
            .await
            .unwrap();
        fixture
            .gate
            .begin_execution(run, None, &successor, b"body")
            .await
            .unwrap();
        let mut reservation = pending(run);
        fixture.gate.save(&reservation).unwrap();
        fixture
            .gate
            .link_execution(&successor, &reservation)
            .unwrap();
        fixture
            .gate
            .created(
                &Permit {
                    reservation: reservation.clone(),
                },
                "c".repeat(64),
            )
            .await
            .unwrap();
        reservation.container_id = Some("c".repeat(64));
        let live = Arc::new(Mutex::new(inventory(&reservation)));
        let removed = Arc::new(Mutex::new(Vec::new()));
        stop_fake(
            &fixture.gate,
            &request(run, &old, Some(old.lease_token)),
            live.clone(),
            removed.clone(),
            true,
        )
        .await
        .unwrap();
        assert!(removed.lock().unwrap().is_empty());
        assert_eq!(live.lock().unwrap().len(), 1);
        assert!(fixture
            .gate
            .check_execution_open(run, Some(old.lease_token))
            .is_err());
        assert!(fixture
            .gate
            .check_execution_open(run, Some(successor.lease_token))
            .is_ok());
        live.lock()
            .unwrap()
            .get_mut(&reservation.container_name)
            .unwrap()
            .id = "d".repeat(64);
        assert!(stop_fake(
            &fixture.gate,
            &request(run, &successor, Some(successor.lease_token)),
            live,
            removed.clone(),
            true
        )
        .await
        .is_err());
        assert!(removed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn renamed_known_id_requires_daemon_wide_physical_absence() {
        let fixture = Fixture::new();
        let run = Uuid::new_v4();
        let execution = execution();
        fixture
            .gate
            .begin_execution(run, None, &execution, b"body")
            .await
            .unwrap();
        let mut reservation = pending(run);
        fixture.gate.save(&reservation).unwrap();
        fixture
            .gate
            .link_execution(&execution, &reservation)
            .unwrap();
        fixture
            .gate
            .created(
                &Permit {
                    reservation: reservation.clone(),
                },
                "e".repeat(64),
            )
            .await
            .unwrap();
        reservation.container_id = Some("e".repeat(64));
        let mut live = inventory(&reservation);
        let mut container = live.remove(&reservation.container_name).unwrap();
        container.name = "renamed-outside-runner-prefix".into();
        live.insert(container.name.clone(), container);
        let live = Arc::new(Mutex::new(live));
        let removed = Arc::new(Mutex::new(Vec::new()));
        let request = request(run, &execution, None);
        assert!(stop_fake(
            &fixture.gate,
            &request,
            live.clone(),
            removed.clone(),
            false
        )
        .await
        .is_err());
        assert_eq!(
            fixture.gate.load().unwrap().len(),
            1,
            "missing scoped name must not release the known ID"
        );
        assert_eq!(live.lock().unwrap().len(), 1);
        // The immutable ID and own Run/Reservation labels permit exact removal
        // despite renaming; only the second exact read proves it gone.
        let receipt = stop_fake(&fixture.gate, &request, live.clone(), removed.clone(), true)
            .await
            .unwrap();
        assert_eq!(receipt["containers"][0]["container_id"], "e".repeat(64));
        assert!(live.lock().unwrap().is_empty());
        assert!(fixture.gate.load().unwrap().is_empty());
    }

    #[tokio::test]
    async fn stopping_one_run_preserves_unrelated_renamed_and_unknown_reservations() {
        let fixture = Fixture::new();
        let run = Uuid::new_v4();
        let execution = execution();
        fixture
            .gate
            .begin_execution(run, None, &execution, b"body")
            .await
            .unwrap();
        let mut unrelated = pending(Uuid::new_v4());
        unrelated.container_id = Some("f".repeat(64));
        fixture.gate.save(&unrelated).unwrap();
        let unknown = pending(Uuid::new_v4());
        fixture.gate.save(&unknown).unwrap();
        let mut live = inventory(&unrelated);
        let mut container = live.remove(&unrelated.container_name).unwrap();
        container.name = "renamed-unrelated".into();
        live.insert(container.name.clone(), container);
        let live = Arc::new(Mutex::new(live));
        let removed = Arc::new(Mutex::new(Vec::new()));
        stop_fake(
            &fixture.gate,
            &request(run, &execution, None),
            live.clone(),
            removed.clone(),
            true,
        )
        .await
        .unwrap();
        assert!(removed.lock().unwrap().is_empty());
        assert_eq!(live.lock().unwrap().len(), 1);
        let held = fixture.gate.load().unwrap();
        assert_eq!(held.len(), 2);
        assert!(held.iter().any(|entry| entry.id == unrelated.id));
        assert!(held.iter().any(|entry| entry.id == unknown.id));
    }

    #[tokio::test]
    async fn acknowledged_id_is_journaled_before_removal_even_after_partial_create_persistence() {
        let fixture = Fixture::new();
        let run = Uuid::new_v4();
        let execution = execution();
        fixture
            .gate
            .begin_execution(run, None, &execution, b"body")
            .await
            .unwrap();
        let mut reservation = pending(run);
        fixture.gate.save(&reservation).unwrap();
        fixture
            .gate
            .link_execution(&execution, &reservation)
            .unwrap();
        // Crash after created saved the reservation, before attempt-ID update.
        reservation.container_id = Some("a".repeat(64));
        fixture.gate.save(&reservation).unwrap();
        let live = Arc::new(Mutex::new(inventory(&reservation)));
        let removed = Arc::new(Mutex::new(Vec::new()));
        let request = request(run, &execution, None);
        let first = stop_fake(&fixture.gate, &request, live.clone(), removed.clone(), true)
            .await
            .unwrap();
        let reopened = Admission::new(
            fixture.gate.directory.clone(),
            fixture.gate.prefix.clone(),
            fixture.gate.limits.clone(),
        )
        .unwrap();
        let again = stop_fake(&reopened, &request, live, removed.clone(), true)
            .await
            .unwrap();
        assert_eq!(first, again);
        assert_eq!(removed.lock().unwrap().len(), 1);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedAttempt {
    pub attempt_id: Uuid,
    pub lease_token: Uuid,
    pub request_sha256: String,
    pub workspace_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StopRequest {
    pub version: u8,
    pub run_id: Uuid,
    pub lease_token: Option<Uuid>,
    pub cancel_lease: Option<Uuid>,
    pub attempts: Vec<ExpectedAttempt>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    run_id: Uuid,
    expected: ExpectedAttempt,
    reservation_id: Option<Uuid>,
    container_id: Option<String>,
    container_name: Option<String>,
}

impl Admission {
    fn execution_directory(&self) -> Result<PathBuf> {
        let path = self.directory.join("executions");
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    fn fence_path(&self, run_id: Uuid, lease: Option<Uuid>) -> PathBuf {
        self.directory.join(format!(
            "stop-{run_id}-{}",
            lease
                .map(|value| value.to_string())
                .unwrap_or_else(|| "run".into())
        ))
    }

    pub(super) fn check_execution_open(&self, run_id: Uuid, lease: Option<Uuid>) -> Result<()> {
        if self.fence_path(run_id, None).exists()
            || lease.is_some_and(|token| self.fence_path(run_id, Some(token)).exists())
        {
            bail!("run execution is durably stopped");
        }
        Ok(())
    }

    fn attempts(&self, run_id: Uuid) -> Result<Vec<Attempt>> {
        let mut records = Vec::new();
        for entry in fs::read_dir(self.execution_directory()?)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let record: Attempt = serde_json::from_slice(&fs::read(&path)?)?;
            if path.file_stem().and_then(|value| value.to_str())
                != Some(&record.expected.attempt_id.to_string())
            {
                bail!("invalid runner execution journal identity");
            }
            if record.run_id == run_id {
                records.push(record);
            }
        }
        Ok(records)
    }

    fn save_attempt(&self, attempt: &Attempt) -> Result<()> {
        let directory = self.execution_directory()?;
        let temporary = directory.join(format!("{}.tmp", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(attempt)?)?;
        file.sync_all()?;
        fs::rename(
            temporary,
            directory.join(format!("{}.json", attempt.expected.attempt_id)),
        )?;
        sync_directory(&directory)
    }

    pub(crate) async fn begin_execution(
        &self,
        run_id: Uuid,
        workspace_id: Option<Uuid>,
        execution: &Execution,
        body: &[u8],
    ) -> Result<(), RunnerError> {
        if execution.version != 1 {
            return Err(RunnerError::bad_request(
                "unsupported private execution version",
            ));
        }
        let _lock = self
            .lock()
            .await
            .map_err(RunnerError::admission_unavailable)?;
        self.check_execution_open(run_id, Some(execution.lease_token))
            .map_err(RunnerError::admission_unavailable)?;
        let path = self
            .execution_directory()
            .map_err(RunnerError::admission_unavailable)?
            .join(format!("{}.json", execution.attempt_id));
        // Never replay Docker create, even if the first HTTP result was lost.
        if path.exists() {
            return Err(RunnerError::resource_conflict(
                "execution attempt was already received",
            ));
        }
        self.save_attempt(&Attempt {
            run_id,
            expected: ExpectedAttempt {
                attempt_id: execution.attempt_id,
                lease_token: execution.lease_token,
                request_sha256: hex::encode(Sha256::digest(body)),
                workspace_id,
            },
            reservation_id: None,
            container_id: None,
            container_name: None,
        })
        .map_err(RunnerError::admission_unavailable)
    }

    pub(super) fn link_execution(
        &self,
        execution: &Execution,
        reservation: &Reservation,
    ) -> Result<()> {
        let mut record = self
            .attempts(reservation.run_id)?
            .into_iter()
            .find(|record| record.expected.attempt_id == execution.attempt_id)
            .context("missing execution journal")?;
        if record.expected.lease_token != execution.lease_token || record.reservation_id.is_some() {
            bail!("execution binding changed");
        }
        record.reservation_id = Some(reservation.id);
        record.container_name = Some(reservation.container_name.clone());
        self.save_attempt(&record)
    }

    pub(super) fn acknowledge_execution(&self, reservation: &Reservation) -> Result<()> {
        for mut record in self.attempts(reservation.run_id)? {
            if record.reservation_id == Some(reservation.id) {
                record.container_id = reservation.container_id.clone();
                self.save_attempt(&record)?;
                self.check_execution_open(record.run_id, Some(record.expected.lease_token))?;
            }
        }
        Ok(())
    }

    /// Bind an exec to an inspected immutable ID, never a reusable Docker name.
    pub(crate) async fn desktop_execution(
        &self,
        run_id: Uuid,
        name: &str,
        execution: Option<&Execution>,
    ) -> Result<String, RunnerError> {
        let _lock = self
            .lock()
            .await
            .map_err(RunnerError::admission_unavailable)?;
        self.check_execution_open(run_id, execution.map(|value| value.lease_token))
            .map_err(RunnerError::admission_unavailable)?;
        let inventory = self
            .inventory()
            .await
            .map_err(RunnerError::admission_unavailable)?;
        let container = inventory
            .get(name)
            .ok_or_else(|| RunnerError::resource_conflict("desktop container is absent"))?;
        if container.run_id != run_id {
            return Err(RunnerError::resource_conflict(
                "desktop run identity changed",
            ));
        }
        let reservation_id = container.reservation_id.ok_or_else(|| {
            RunnerError::resource_conflict("desktop admission identity is missing")
        })?;
        if let Some(execution) = execution {
            let records = self
                .attempts(run_id)
                .map_err(RunnerError::admission_unavailable)?;
            if records.iter().any(|record| {
                record.container_id.as_deref() == Some(container.id.as_str())
                    && record.expected.lease_token != execution.lease_token
            }) {
                return Err(RunnerError::resource_conflict("desktop belongs to another execution lease; stop and replace it before resuming"));
            }
            let mut record = records
                .into_iter()
                .find(|record| record.expected.attempt_id == execution.attempt_id)
                .ok_or_else(|| RunnerError::resource_conflict("execution journal is absent"))?;
            record.reservation_id = Some(reservation_id);
            record.container_id = Some(container.id.clone());
            record.container_name = Some(name.to_owned());
            self.save_attempt(&record)
                .map_err(RunnerError::admission_unavailable)?;
        }
        Ok(container.id.clone())
    }

    fn fence_stop(&self, request: &StopRequest) -> Result<Vec<Attempt>> {
        if request.version != 1 || request.attempts.len() > 100_000 {
            bail!("invalid stop request");
        }
        let mut seen = HashSet::new();
        for expected in &request.attempts {
            if !seen.insert(expected.attempt_id)
                || expected.request_sha256.len() != 64
                || !expected
                    .request_sha256
                    .chars()
                    .all(|value| value.is_ascii_hexdigit())
                || request
                    .lease_token
                    .is_some_and(|token| token != expected.lease_token)
            {
                bail!("invalid stop attempt binding");
            }
        }
        let path = self.fence_path(request.run_id, request.lease_token);
        if !path.exists() {
            let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
            file.write_all(&serde_json::to_vec(request)?)?;
            file.sync_all()?;
            sync_directory(&self.directory)?;
        }
        let mut records = self.attempts(request.run_id)?;
        for expected in &request.attempts {
            match records
                .iter()
                .find(|record| record.expected.attempt_id == expected.attempt_id)
            {
                Some(record) if record.expected != *expected => {
                    bail!("stop attempt identity changed")
                }
                Some(_) => {}
                None => {
                    // Fence precedes this tombstone: a delayed POST cannot create.
                    let record = Attempt {
                        run_id: request.run_id,
                        expected: expected.clone(),
                        reservation_id: None,
                        container_id: None,
                        container_name: None,
                    };
                    let path = self
                        .execution_directory()?
                        .join(format!("{}.json", expected.attempt_id));
                    if path.exists() {
                        bail!("stop attempt belongs to another run");
                    }
                    self.save_attempt(&record)?;
                    records.push(record);
                }
            }
        }
        Ok(records
            .into_iter()
            .filter(|record| {
                request
                    .lease_token
                    .is_none_or(|token| token == record.expected.lease_token)
            })
            .collect())
    }

    pub(crate) async fn stop_execution(
        &self,
        request: &StopRequest,
    ) -> Result<serde_json::Value, RunnerError> {
        self.stop_execution_with(
            request,
            || self.inventory(),
            |id| async move { crate::docker(&["rm", "--force", &id]).await.map(|_| ()) },
            |id| async move { self.inspect_exact(&id).await },
        )
        .await
    }

    async fn stop_execution_with<I, IFut, R, RFut, E, EFut>(
        &self,
        request: &StopRequest,
        mut inventory_read: I,
        mut remove: R,
        mut exact_read: E,
    ) -> Result<serde_json::Value, RunnerError>
    where
        I: FnMut() -> IFut,
        IFut: std::future::Future<Output = Result<HashMap<String, Container>>>,
        R: FnMut(String) -> RFut,
        RFut: std::future::Future<Output = Result<()>>,
        E: FnMut(String) -> EFut,
        EFut: std::future::Future<Output = Result<Option<Container>>>,
    {
        let _lock = self
            .lock()
            .await
            .map_err(RunnerError::admission_unavailable)?;
        let mut records = self
            .fence_stop(request)
            .map_err(RunnerError::admission_unavailable)?;
        let inventory = inventory_read()
            .await
            .map_err(RunnerError::admission_unavailable)?;
        let reservations = self.load().map_err(RunnerError::admission_unavailable)?;
        let bound: HashSet<Uuid> = records
            .iter()
            .filter_map(|record| record.reservation_id)
            .collect();
        let selected: Vec<_> = reservations
            .iter()
            .filter(|reservation| {
                reservation.run_id == request.run_id
                    && (request.lease_token.is_none() || bound.contains(&reservation.id))
            })
            .collect();
        // A create may still arrive after this inventory. Never treat absence as
        // proof for a reservation whose immutable ID was not acknowledged.
        for reservation in &selected {
            if reservation.container_id.is_none() {
                return Err(RunnerError::resource_conflict(
                    "sandbox launch remains unproven; stop and capacity remain held",
                ));
            }
            if inventory
                .get(&reservation.container_name)
                .is_some_and(|container| Some(&container.id) != reservation.container_id.as_ref())
            {
                return Err(RunnerError::resource_conflict(
                    "sandbox reservation identity changed",
                ));
            }
        }
        let mut identities = HashMap::new();
        for reservation in &selected {
            for record in &mut records {
                if record.reservation_id == Some(reservation.id) && record.container_id.is_none() {
                    // Recovery for a crash between acknowledged reservation-ID
                    // persistence and its attempt-journal update. Persist this
                    // before removal so a lost stop reply remains retryable.
                    record.container_id = reservation.container_id.clone();
                    record.container_name = Some(reservation.container_name.clone());
                    self.save_attempt(record)
                        .map_err(RunnerError::admission_unavailable)?;
                }
            }
            identities.insert(
                reservation.container_id.clone().expect("checked ID"),
                (reservation.container_name.clone(), reservation.id),
            );
        }
        for record in &records {
            if let (Some(id), Some(name), Some(reservation)) = (
                &record.container_id,
                &record.container_name,
                record.reservation_id,
            ) {
                identities.insert(id.clone(), (name.clone(), reservation));
            } else if record.reservation_id.is_some()
                && !selected
                    .iter()
                    .any(|reservation| Some(reservation.id) == record.reservation_id)
            {
                return Err(RunnerError::resource_conflict(
                    "attempt has an unresolved launch identity",
                ));
            }
        }
        // Run-wide cancellation also covers recovered legacy desktops/helpers.
        if request.lease_token.is_none() {
            for container in inventory
                .values()
                .filter(|container| container.run_id == request.run_id)
            {
                let reservation = container.reservation_id.ok_or_else(|| {
                    RunnerError::resource_conflict("sandbox admission identity missing")
                })?;
                identities.insert(container.id.clone(), (container.name.clone(), reservation));
            }
        }
        let mut stopped = Vec::new();
        for (id, (name, reservation_id)) in &identities {
            if let Some(container) = exact_read(id.clone())
                .await
                .map_err(RunnerError::admission_unavailable)?
            {
                if container.id != *id
                    || container.run_id != request.run_id
                    || container.reservation_id != Some(*reservation_id)
                {
                    return Err(RunnerError::resource_conflict(
                        "sandbox stop identity changed",
                    ));
                }
                // rm of an exact ID prevents a late `start` of this same ID.
                let _ = remove(id.clone()).await;
            }
            stopped.push(serde_json::json!({"container_id": id, "container_name": name, "reservation_id": reservation_id}));
        }
        for id in identities.keys() {
            if exact_read(id.clone())
                .await
                .map_err(RunnerError::admission_unavailable)?
                .is_some()
            {
                return Err(RunnerError::resource_conflict(
                    "exact physical sandbox stop is unconfirmed",
                ));
            }
        }
        let after = inventory_read()
            .await
            .map_err(RunnerError::admission_unavailable)?;
        if after.values().any(|container| {
            identities.contains_key(&container.id)
                || (request.lease_token.is_none() && container.run_id == request.run_id)
        }) {
            return Err(RunnerError::resource_conflict(
                "physical sandbox stop is unconfirmed",
            ));
        }
        // Only this stop's exact, independently absent IDs may release charges.
        // An unrelated renamed sandbox must retain its reservation.
        for reservation in selected {
            self.remove(reservation.id)
                .map_err(RunnerError::admission_unavailable)?;
        }
        stopped.sort_by_key(|entry| {
            entry["container_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        });
        Ok(
            serde_json::json!({"version":1,"run_id":request.run_id,"lease_token":request.lease_token,"cancel_lease":request.cancel_lease,"attempts":request.attempts,"containers":stopped,"execution_stopped":true}),
        )
    }
}
