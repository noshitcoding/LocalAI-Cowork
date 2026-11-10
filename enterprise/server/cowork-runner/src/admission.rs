//! Durable admission for one Docker execution scope. Reservations precede Docker
//! calls; a lost response must never make an outstanding launch free capacity.
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

use crate::RunnerError;

mod execution;
pub(crate) use execution::{Execution, StopRequest};
mod desktop_identity;
pub(crate) use desktop_identity::{
    proof_response, CreationQuery, ProbeRequest, CREATE_DOMAIN, PROBE_DOMAIN, STOP_DOMAIN,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resources {
    pub cpu_nanos: u64,
    pub memory_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Limits {
    pub cpu_nanos: u64,
    pub memory_bytes: u64,
    pub concurrent_runs: usize,
    pub containers: usize,
}

#[derive(Debug)]
pub struct Admission {
    directory: PathBuf,
    prefix: String,
    limits: Limits,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reservation {
    id: Uuid,
    container_name: String,
    run_id: Uuid,
    resources: Resources,
    /// None is an ambiguous launch, including a crash before contacting Docker.
    /// An error response or elapsed timeout must never release it without an ID.
    container_id: Option<String>,
}

#[derive(Debug)]
struct Container {
    id: String,
    name: String,
    run_id: Uuid,
    reservation_id: Option<Uuid>,
    resources: Resources,
}

#[derive(Debug)]
pub struct Permit {
    reservation: Reservation,
}

impl Limits {
    pub fn from_env() -> Result<Self> {
        fn positive(name: &str, default: u64) -> Result<u64> {
            let value = match std::env::var(name) {
                Ok(value) => value.parse().with_context(|| format!("invalid {name}"))?,
                Err(std::env::VarError::NotPresent) => default,
                Err(error) => return Err(error.into()),
            };
            if value == 0 {
                bail!("{name} must be positive");
            }
            Ok(value)
        }
        Ok(Self {
            cpu_nanos: positive("COWORK_RUNNER_CPU_MILLIS", 4000)?
                .checked_mul(1_000_000)
                .context("CPU limit overflow")?,
            memory_bytes: positive("COWORK_RUNNER_MEMORY_MIB", 8192)?
                .checked_mul(1024 * 1024)
                .context("memory limit overflow")?,
            concurrent_runs: positive("COWORK_RUNNER_CONCURRENT_RUNS", 2)?.try_into()?,
            containers: positive("COWORK_RUNNER_MAX_CONTAINERS", 64)?.try_into()?,
        })
    }
}

impl Admission {
    pub fn new(directory: PathBuf, prefix: String, limits: Limits) -> Result<Self> {
        fs::create_dir_all(&directory)
            .context("runner admission state must be writable and persistent")?;
        // Bind a persistent ledger to its scope. Sharing it between instances is
        // a configuration error, never a reason to disregard its reservations.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("lock"))?;
        FileExt::try_lock_exclusive(&lock).context("runner admission state is in use")?;
        let scope = directory.join("scope");
        if scope.exists() {
            if fs::read_to_string(scope)? != prefix {
                bail!("runner admission scope mismatch");
            }
        } else {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(scope)?;
            file.write_all(prefix.as_bytes())?;
            file.sync_all()?;
            sync_directory(&directory)?;
        }
        Ok(Self {
            directory,
            prefix,
            limits,
        })
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    async fn lock(&self) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.directory.join("lock"))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(file),
                Err(error)
                    if (error.kind() == std::io::ErrorKind::WouldBlock
                        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error())
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(error) => return Err(error).context("runner admission lock unavailable"),
            }
        }
    }

    fn load(&self) -> Result<Vec<Reservation>> {
        let mut reservations = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let reservation: Reservation = serde_json::from_slice(&fs::read(&path)?)
                .context("corrupt runner admission reservation; operator review required")?;
            if path.file_stem().and_then(|value| value.to_str())
                != Some(&reservation.id.to_string())
                || !self.owns(&reservation.container_name)
                || reservation.resources.cpu_nanos == 0
                || reservation.resources.memory_bytes == 0
            {
                bail!("invalid runner admission reservation; operator review required");
            }
            reservations.push(reservation);
        }
        Ok(reservations)
    }

    fn save(&self, reservation: &Reservation) -> Result<()> {
        let temporary = self.directory.join(format!("{}.tmp", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(reservation)?)?;
        file.sync_all()?;
        fs::rename(
            &temporary,
            self.directory.join(format!("{}.json", reservation.id)),
        )?;
        sync_directory(&self.directory)
    }

    fn remove(&self, id: Uuid) -> Result<()> {
        fs::remove_file(self.directory.join(format!("{id}.json")))?;
        sync_directory(&self.directory)
    }

    fn owns(&self, name: &str) -> bool {
        ["job-", "desktop-", "terminal-", "transfer-"]
            .iter()
            .any(|kind| {
                name.strip_prefix(&self.prefix)
                    .is_some_and(|tail| tail.starts_with(kind))
            })
    }

    async fn inventory(&self) -> Result<HashMap<String, Container>> {
        let filter = format!("name=^/{}", self.prefix);
        let listed = crate::docker(&[
            "ps",
            "--all",
            "--no-trunc",
            "--filter",
            &filter,
            "--format",
            "{{.ID}} {{.Names}}",
        ])
        .await?;
        let mut result = HashMap::new();
        for line in std::str::from_utf8(&listed.stdout)?.lines() {
            let (id, name) = line.split_once(' ').context("invalid Docker inventory")?;
            if !self.owns(name) {
                continue;
            }
            let inspected = Command::new("docker")
                .args(["inspect", id])
                .output()
                .await?;
            if !inspected.status.success() {
                // Auto-removed containers may vanish between list and inspect.
                // A failed daemon or permission check must not look like absence.
                let remaining = crate::docker(&[
                    "ps",
                    "--all",
                    "--no-trunc",
                    "--filter",
                    &format!("id={id}"),
                    "--format",
                    "{{.ID}}",
                ])
                .await?;
                if remaining.stdout.is_empty() {
                    continue;
                }
                bail!("cannot inspect an existing sandbox container");
            }
            let entries: Vec<serde_json::Value> = serde_json::from_slice(&inspected.stdout)?;
            let value = entries.first().context("empty Docker inspection")?;
            let container = parse_container(value)?;
            if container.id != id || container.name != name {
                bail!("Docker inventory changed identity");
            }
            result.insert(container.name.clone(), container);
        }
        Ok(result)
    }

    /// Docker names may change; known physical IDs need daemon-wide checks.
    async fn inspect_exact(&self, id: &str) -> Result<Option<Container>> {
        if id.len() != 64 || !id.chars().all(|ch| ch.is_ascii_hexdigit()) {
            bail!("invalid exact Docker identity");
        }
        let inspected = Command::new("docker")
            .args(["inspect", id])
            .output()
            .await?;
        if inspected.status.success() {
            let entries: Vec<serde_json::Value> = serde_json::from_slice(&inspected.stdout)?;
            let container =
                parse_container(entries.first().context("empty exact Docker inspection")?)?;
            if container.id != id {
                bail!("exact Docker inspection identity changed");
            }
            return Ok(Some(container));
        }
        // An unsuccessful daemon/permission query never proves absence.
        let listed = crate::docker(&[
            "ps",
            "--all",
            "--no-trunc",
            "--filter",
            &format!("id={id}"),
            "--format",
            "{{.ID}}",
        ])
        .await?;
        if !std::str::from_utf8(&listed.stdout)?.trim().is_empty() {
            bail!("cannot inspect an existing exact Docker ID");
        }
        Ok(None)
    }

    async fn inventory_for_reconcile(&self) -> Result<HashMap<String, Container>> {
        let inventory = self.inventory().await?;
        self.verify_retained_ids(
            &inventory,
            |id| async move { self.inspect_exact(&id).await },
        )
        .await?;
        Ok(inventory)
    }

    async fn verify_retained_ids<E, EFut>(
        &self,
        inventory: &HashMap<String, Container>,
        mut exact_read: E,
    ) -> Result<()>
    where
        E: FnMut(String) -> EFut,
        EFut: std::future::Future<Output = Result<Option<Container>>>,
    {
        for reservation in self.load()? {
            if let Some(id) = reservation.container_id {
                if inventory
                    .get(&reservation.container_name)
                    .is_some_and(|container| container.id == id)
                {
                    continue;
                }
                if exact_read(id).await?.is_some() {
                    bail!("known physical sandbox changed its resource name; capacity remains reserved");
                }
            }
        }
        Ok(())
    }

    fn reconcile(
        &self,
        reservations: Vec<Reservation>,
        inventory: &HashMap<String, Container>,
    ) -> Result<Vec<Reservation>> {
        let mut retained = Vec::new();
        for mut reservation in reservations {
            match inventory.get(&reservation.container_name) {
                Some(container) => {
                    if container.reservation_id != Some(reservation.id)
                        || container.run_id != reservation.run_id
                        || container.resources != reservation.resources
                        || reservation
                            .container_id
                            .as_ref()
                            .is_some_and(|id| *id != container.id)
                    {
                        bail!("sandbox reservation identity or limits changed; operator review required");
                    }
                    if reservation.container_id.is_none() {
                        reservation.container_id = Some(container.id.clone());
                        self.save(&reservation)?;
                    }
                    retained.push(reservation);
                }
                None if reservation.container_id.is_some() => self.remove(reservation.id)?,
                None => retained.push(reservation), // No timeout-based forgiveness.
            }
        }
        Ok(retained)
    }

    pub async fn reserve(
        &self,
        container_name: String,
        run_id: Uuid,
        resources: Resources,
    ) -> Result<Permit, RunnerError> {
        self.reserve_execution(container_name, run_id, resources, None)
            .await
    }

    pub async fn reserve_execution(
        &self,
        container_name: String,
        run_id: Uuid,
        resources: Resources,
        execution: Option<&Execution>,
    ) -> Result<Permit, RunnerError> {
        if !self.owns(&container_name) {
            return Err(RunnerError::internal("invalid sandbox resource scope"));
        }
        let _lock = self
            .lock()
            .await
            .map_err(RunnerError::admission_unavailable)?;
        self.check_execution_open(run_id, execution.map(|value| value.lease_token))
            .map_err(RunnerError::admission_unavailable)?;
        let inventory = self
            .inventory_for_reconcile()
            .await
            .map_err(RunnerError::admission_unavailable)?;
        let reservations = self
            .reconcile(
                self.load().map_err(RunnerError::admission_unavailable)?,
                &inventory,
            )
            .map_err(RunnerError::admission_unavailable)?;
        let mut usage = Vec::new();
        for reservation in &reservations {
            usage.push((reservation.run_id, reservation.resources));
        }
        for container in inventory.values() {
            if !reservations
                .iter()
                .any(|reservation| reservation.container_name == container.name)
            {
                usage.push((container.run_id, container.resources));
            }
        }
        if inventory.contains_key(&container_name)
            || reservations
                .iter()
                .any(|entry| entry.container_name == container_name)
        {
            return Err(RunnerError::resource_conflict(
                "this sandbox already exists or has an unproven launch",
            ));
        }
        check_capacity(&self.limits, &usage, run_id, resources)?;
        let reservation = Reservation {
            id: Uuid::new_v4(),
            container_name,
            run_id,
            resources,
            container_id: None,
        };
        self.save(&reservation)
            .map_err(RunnerError::admission_unavailable)?;
        if let Some(execution) = execution {
            self.link_execution(execution, &reservation)
                .map_err(RunnerError::admission_unavailable)?;
        }
        Ok(Permit { reservation })
    }

    /// Record the acknowledged immutable Docker ID before issuing `start`.
    pub async fn created(&self, permit: &Permit, container_id: String) -> Result<(), RunnerError> {
        let _lock = self
            .lock()
            .await
            .map_err(RunnerError::admission_unavailable)?;
        let reservations = self.load().map_err(RunnerError::admission_unavailable)?;
        let mut reservation = reservations
            .into_iter()
            .find(|entry| entry.id == permit.reservation.id)
            .ok_or_else(|| {
                RunnerError::resource_conflict("sandbox reservation was already reconciled")
            })?;
        if reservation
            .container_id
            .as_ref()
            .is_some_and(|id| *id != container_id)
        {
            return Err(RunnerError::resource_conflict(
                "sandbox creation returned a different container identity",
            ));
        }
        reservation.container_id = Some(container_id);
        self.save(&reservation)
            .map_err(RunnerError::admission_unavailable)?;
        self.acknowledge_execution(&reservation)
            .map_err(RunnerError::admission_unavailable)?;
        self.check_execution_open(reservation.run_id, None)
            .map_err(RunnerError::admission_unavailable)
    }

    /// Release only reservations tied to acknowledged or observed Docker IDs.
    /// Even a returned CLI error may conceal a create request still in flight.
    pub async fn finished(&self, permit: &Permit) -> Result<(), RunnerError> {
        let _lock = self
            .lock()
            .await
            .map_err(RunnerError::admission_unavailable)?;
        let inventory = self
            .inventory_for_reconcile()
            .await
            .map_err(RunnerError::admission_unavailable)?;
        let reservations = self
            .reconcile(
                self.load().map_err(RunnerError::admission_unavailable)?,
                &inventory,
            )
            .map_err(RunnerError::admission_unavailable)?;
        if reservations
            .iter()
            .any(|entry| entry.id == permit.reservation.id && entry.container_id.is_none())
        {
            return Err(RunnerError::resource_conflict(
                "sandbox launch remains unproven; capacity is still reserved",
            ));
        }
        Ok(())
    }
}

impl Permit {
    pub fn annotate(&self, arguments: &mut Vec<String>) {
        // Docker's memory limit permits extra swap by default. Bound it too.
        arguments.splice(
            1..1,
            [
                "--label".into(),
                format!("dev.opencowork.admission={}", self.reservation.id),
                "--label".into(),
                format!("dev.opencowork.run_id={}", self.reservation.run_id),
                "--memory-swap".into(),
                self.reservation.resources.memory_bytes.to_string(),
            ],
        );
    }
}

/// An unforgeable-in-parent-code marker minted only by the capacity check,
/// before the new reservation is saved or any Docker create is attempted.
#[derive(Debug)]
pub(super) struct NoStart {
    _private: (),
}

impl RunnerError {
    fn capacity_exhausted(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::TOO_MANY_REQUESTS,
            code: "sandbox_capacity_exhausted",
            message: message.into(),
            no_start: Some(NoStart { _private: () }),
            no_start_proof: None,
        }
    }
}

/// This response MAC deliberately uses a different domain from request auth.
/// Each field is length-prefixed to keep the binding unambiguous.
pub(super) fn no_start_proof(
    _marker: &NoStart,
    key: &[u8],
    method: &str,
    path_and_query: &str,
    timestamp: &str,
    nonce: &str,
    body: &[u8],
) -> Result<String, RunnerError> {
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};
    let digest = hex::encode(Sha256::digest(body));
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| RunnerError::internal("invalid runner signing key"))?;
    mac.update(b"open-cowork/runner-no-start/v1\0");
    for field in [
        method,
        path_and_query,
        timestamp,
        nonce,
        &digest,
        "429",
        "sandbox_capacity_exhausted",
    ] {
        mac.update(&(field.len() as u64).to_be_bytes());
        mac.update(field.as_bytes());
    }
    Ok(format!("v1:{}", hex::encode(mac.finalize().into_bytes())))
}

fn check_capacity(
    limits: &Limits,
    usage: &[(Uuid, Resources)],
    run_id: Uuid,
    requested: Resources,
) -> Result<(), RunnerError> {
    if requested.cpu_nanos == 0
        || requested.memory_bytes == 0
        || requested.cpu_nanos > limits.cpu_nanos
        || requested.memory_bytes > limits.memory_bytes
    {
        return Err(RunnerError::bad_request(
            "sandbox request exceeds the instance resource limits",
        ));
    }
    let mut runs = HashSet::from([run_id]);
    let mut cpu = requested.cpu_nanos;
    let mut memory = requested.memory_bytes;
    for (id, resources) in usage {
        runs.insert(*id);
        cpu = cpu.saturating_add(resources.cpu_nanos);
        memory = memory.saturating_add(resources.memory_bytes);
    }
    if cpu > limits.cpu_nanos
        || memory > limits.memory_bytes
        || runs.len() > limits.concurrent_runs
        || usage.len() >= limits.containers
    {
        return Err(RunnerError::capacity_exhausted("instance sandbox capacity is reserved; wait for work to finish or review interrupted launches"));
    }
    Ok(())
}

fn parse_container(value: &serde_json::Value) -> Result<Container> {
    let required = |pointer: &str| -> Result<&str> {
        value
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("missing Docker field {pointer}"))
    };
    let cpu_nanos = value
        .pointer("/HostConfig/NanoCpus")
        .and_then(serde_json::Value::as_u64)
        .context("missing sandbox CPU cap")?;
    let memory_bytes = value
        .pointer("/HostConfig/Memory")
        .and_then(serde_json::Value::as_u64)
        .context("missing sandbox memory cap")?;
    if cpu_nanos == 0 || memory_bytes == 0 {
        bail!("existing sandbox is unbounded; operator review required");
    }
    Ok(Container {
        id: required("/Id")?.into(),
        name: required("/Name")?.trim_start_matches('/').into(),
        run_id: required("/Config/Labels/dev.opencowork.run_id")?.parse()?,
        reservation_id: value
            .pointer("/Config/Labels/dev.opencowork.admission")
            .and_then(serde_json::Value::as_str)
            .map(str::parse)
            .transpose()?,
        resources: Resources {
            cpu_nanos,
            memory_bytes,
        },
    })
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod tests;

#[cfg(test)]
mod no_start_tests {
    use super::*;

    #[test]
    fn only_pre_reservation_capacity_denials_carry_a_no_start_marker() {
        let limits = Limits {
            cpu_nanos: 100,
            memory_bytes: 100,
            concurrent_runs: 1,
            containers: 1,
        };
        let run = Uuid::new_v4();
        let resources = Resources {
            cpu_nanos: 50,
            memory_bytes: 50,
        };
        let error =
            check_capacity(&limits, &[(Uuid::new_v4(), resources)], run, resources).unwrap_err();
        assert!(error.no_start.is_some());
        assert!(
            error.no_start_proof.is_none(),
            "only the authenticated jobs handler signs responses"
        );
        assert!(check_capacity(
            &limits,
            &[],
            run,
            Resources {
                cpu_nanos: 101,
                memory_bytes: 50
            }
        )
        .unwrap_err()
        .no_start
        .is_none());
        assert!(RunnerError::resource_conflict("unproven create")
            .no_start
            .is_none());
        assert!(RunnerError::internal("Docker error").no_start.is_none());
    }

    #[test]
    fn no_start_mac_matches_server_vector_and_is_domain_separated() {
        let marker = NoStart { _private: () };
        let proof = no_start_proof(
            &marker,
            &[b'k'; 32],
            "POST",
            "/v1/jobs",
            "1700000000",
            "00000000-0000-4000-8000-000000000001",
            br#"{"run_id":"fixture"}"#,
        )
        .unwrap();
        assert_eq!(
            proof,
            "v1:a97929d8a4c4b5a99db42ca778a568afcbdca1951c81878dcca50d7324262349"
        );
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut request = Hmac::<Sha256>::new_from_slice(&[b'k'; 32]).unwrap();
        request.update(b"1700000000\n00000000-0000-4000-8000-000000000001\nPOST\n/v1/jobs\n{\"run_id\":\"fixture\"}");
        assert_ne!(
            proof,
            format!("v1:{}", hex::encode(request.finalize().into_bytes()))
        );
    }
}
