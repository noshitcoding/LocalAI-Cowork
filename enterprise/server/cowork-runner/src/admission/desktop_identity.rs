//! Private physical job identities. Names and endpoint URLs grant no authority.
use super::*;
use cowork_contracts::SandboxDesktopSessionSpec;
use sha2::{Digest, Sha256};

pub(crate) const CREATE_DOMAIN: &[u8] = b"open-cowork/runner-desktop-create/v1\0";
pub(crate) const PROBE_DOMAIN: &[u8] = b"open-cowork/runner-desktop-probe/v1\0";
pub(crate) const STOP_DOMAIN: &[u8] = b"open-cowork/runner-desktop-stop/v1\0";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct JobBinding {
    pub version: u8,
    pub runner_id: Uuid,
    pub engine_id: String,
    pub scope_prefix: String,
    pub reservation_id: Uuid,
    pub attempt_id: Uuid,
    pub container_id: String,
    pub container_name: String,
    pub created_at: String,
    pub started_at: String,
    pub run_id: Uuid,
    pub session_id: Uuid,
    pub lease_token: Uuid,
    pub workspace_id: Option<Uuid>,
    pub workspace_volume: String,
}
impl JobBinding {
    fn digest(&self) -> Result<String> {
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(self)?)))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LiveJob {
    pub binding: JobBinding,
    pub running: bool,
    pub paused: bool,
    pub restarting: bool,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProbeRequest {
    pub version: u8,
    pub challenge: Uuid,
    pub binding: JobBinding,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreationQuery {
    pub lease_token: Option<Uuid>,
    pub attempt_id: Option<Uuid>,
}
impl CreationQuery {
    pub(crate) fn private(&self) -> Result<Option<(Uuid, Uuid)>> {
        match (self.lease_token, self.attempt_id) {
            (None, None) => Ok(None),
            (Some(lease), Some(attempt)) if !lease.is_nil() && !attempt.is_nil() => {
                Ok(Some((lease, attempt)))
            }
            _ => bail!("private desktop creation needs exact lease and attempt identities"),
        }
    }
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Identity {
    version: u8,
    runner_id: Uuid,
    engine_id: String,
    scope_prefix: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u8,
    run_id: Uuid,
    session_id: Uuid,
    lease_token: Uuid,
    attempt_id: Uuid,
    workspace_id: Option<Uuid>,
    spec_sha256: String,
    reservation_id: Option<Uuid>,
    container_id: Option<String>,
    binding: Option<JobBinding>,
    stop_requested: bool,
}

fn read_private<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
        bail!("invalid private runner identity record");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 || metadata.nlink() != 1 {
            bail!("runner identity records must be private");
        }
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn save_private<T: Serialize>(directory: &Path, name: &str, value: &T) -> Result<()> {
    let temporary = directory.join(format!("{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    fs::rename(temporary, directory.join(name))?;
    sync_directory(directory)
}
fn valid_engine(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && id.bytes().all(|byte| byte.is_ascii_graphic())
}

impl Admission {
    async fn engine_id(&self) -> Result<String> {
        let output = crate::docker(&["info", "--format", "{{.ID}}"]).await?;
        let id = std::str::from_utf8(&output.stdout)?.trim();
        if !valid_engine(id) {
            bail!("Docker engine identity is unavailable");
        }
        Ok(id.to_owned())
    }
    pub(crate) async fn initialize_desktop_identity(&self) -> Result<()> {
        let _lock = self.lock().await?;
        let engine_id = self.engine_id().await?;
        let path = self.directory.join("runner-identity");
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                self.checked_identity(&engine_id)?;
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::symlink_metadata(self.directory.join("desktop-identities")) {
                    Ok(_) => bail!("private desktop inventory exists without its runner identity; operator review required"),
                    Err(error) if error.kind()==std::io::ErrorKind::NotFound => {},
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        }
        save_private(
            &self.directory,
            "runner-identity",
            &Identity {
                version: 1,
                runner_id: Uuid::new_v4(),
                engine_id,
                scope_prefix: self.prefix.clone(),
            },
        )
    }
    fn checked_identity(&self, engine: &str) -> Result<Identity> {
        let identity: Identity = read_private(&self.directory.join("runner-identity"))?;
        if identity.version != 1
            || identity.runner_id.is_nil()
            || identity.engine_id != engine
            || identity.scope_prefix != self.prefix
        {
            bail!("runner physical engine or journal identity changed");
        }
        Ok(identity)
    }
    fn desktop_directory(&self, create: bool) -> Result<PathBuf> {
        let path = self.directory.join("desktop-identities");
        if create && !path.exists() {
            #[cfg(unix)]
            let mut builder = fs::DirBuilder::new();
            #[cfg(not(unix))]
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&path)?;
            sync_directory(&self.directory)?;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            bail!("invalid desktop identity directory");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.mode() & 0o077 != 0 {
                bail!("desktop identity directory must be private");
            }
        }
        Ok(path)
    }
    fn intent(&self, session: Uuid) -> Result<Intent> {
        let intent: Intent = read_private(
            &self
                .desktop_directory(false)?
                .join(format!("{session}.json")),
        )?;
        if intent.version != 1 || intent.session_id != session {
            bail!("desktop journal identity changed");
        }
        Ok(intent)
    }
    fn save_intent(&self, intent: &Intent) -> Result<()> {
        save_private(
            &self.desktop_directory(true)?,
            &format!("{}.json", intent.session_id),
            intent,
        )
    }

    pub(crate) async fn prepare_desktop_identity(
        &self,
        spec: &SandboxDesktopSessionSpec,
        lease: Uuid,
        attempt: Uuid,
    ) -> Result<()> {
        self.prepare_desktop_identity_with(spec, lease, attempt, self.engine_id())
            .await
    }
    async fn prepare_desktop_identity_with(
        &self,
        spec: &SandboxDesktopSessionSpec,
        lease: Uuid,
        attempt: Uuid,
        engine: impl std::future::Future<Output = Result<String>>,
    ) -> Result<()> {
        if [spec.run_id, spec.session_id, lease, attempt]
            .iter()
            .any(Uuid::is_nil)
            || spec.workspace_id.is_some_and(|id| id.is_nil())
        {
            bail!("invalid private desktop identity");
        }
        let _lock = self.lock().await?;
        self.check_execution_open(spec.run_id, Some(lease))?;
        self.checked_identity(&engine.await?)?;
        let directory = self.desktop_directory(true)?;
        let marker = format!("attempt-{attempt}");
        if directory.join(format!("{}.json", spec.session_id)).exists()
            || directory.join(&marker).exists()
        {
            bail!("desktop creation attempt was already received; no create replay permitted");
        }
        save_private(&directory, &marker, &spec.session_id)?;
        self.save_intent(&Intent {
            version: 1,
            run_id: spec.run_id,
            session_id: spec.session_id,
            lease_token: lease,
            attempt_id: attempt,
            workspace_id: spec.workspace_id,
            spec_sha256: hex::encode(Sha256::digest(serde_json::to_vec(spec)?)),
            reservation_id: None,
            container_id: None,
            binding: None,
            stop_requested: false,
        })
    }
    pub(crate) async fn bind_desktop_container(
        &self,
        session: Uuid,
        permit: &Permit,
        id: &str,
    ) -> Result<()> {
        let _lock = self.lock().await?;
        let mut intent = self.intent(session)?;
        if intent.stop_requested
            || intent.run_id != permit.reservation.run_id
            || intent.container_id.is_some()
        {
            bail!("desktop creation identity changed");
        }
        intent.container_id = Some(id.to_owned());
        intent.reservation_id = Some(permit.reservation.id);
        self.save_intent(&intent)?;
        self.check_execution_open(intent.run_id, Some(intent.lease_token))
    }

    async fn inspect_value(&self, id: &str) -> Result<serde_json::Value> {
        if id.len() != 64
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            bail!("invalid exact desktop ID");
        }
        let output = crate::docker(&["inspect", id]).await?;
        if output.stdout.len() > 1024 * 1024 {
            bail!("desktop inspection exceeds bounds");
        }
        let mut values: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)?;
        if values.len() != 1 {
            bail!("desktop inspection is ambiguous");
        }
        Ok(values.remove(0))
    }
    fn physical_job(
        &self,
        intent: &Intent,
        identity: &Identity,
        value: &serde_json::Value,
    ) -> Result<LiveJob> {
        let text = |pointer: &str| -> Result<&str> {
            value
                .pointer(pointer)
                .and_then(serde_json::Value::as_str)
                .context("desktop physical identity field missing")
        };
        let label = |name: &str| -> Result<&str> {
            value
                .pointer("/Config/Labels")
                .and_then(|labels| labels.get(name))
                .and_then(serde_json::Value::as_str)
                .context("desktop physical identity label missing")
        };
        let container = parse_container(value)?;
        let run: Uuid = label("dev.opencowork.run_id")?.parse()?;
        let session: Uuid = label("dev.opencowork.desktop_session")?.parse()?;
        let lease: Uuid = label("dev.opencowork.desktop_lease")?.parse()?;
        let attempt: Uuid = label("dev.opencowork.desktop_attempt")?.parse()?;
        let workspace = match label("dev.opencowork.workspace_id")? {
            "none" => None,
            value => Some(value.parse::<Uuid>()?),
        };
        let mounts = value["Mounts"]
            .as_array()
            .context("desktop workspace mount missing")?;
        let workspaces: Vec<_> = mounts
            .iter()
            .filter(|mount| mount["Destination"] == "/workspace")
            .collect();
        if workspaces.len() != 1 || workspaces[0]["Type"] != "volume" || workspaces[0]["RW"] != true
        {
            bail!("desktop workspace mount identity changed");
        }
        let volume = workspaces[0]["Name"]
            .as_str()
            .context("desktop workspace volume missing")?;
        if run != intent.run_id
            || session != intent.session_id
            || lease != intent.lease_token
            || attempt != intent.attempt_id
            || workspace != intent.workspace_id
            || Some(&container.id) != intent.container_id.as_ref()
            || container.reservation_id != intent.reservation_id
            || container.name != format!("{}desktop-{}", self.prefix, session.simple())
            || volume != crate::workspace_volume(run, workspace)
            || label("dev.opencowork.runner_managed")? != "true"
        {
            bail!("desktop physical identity differs from its frozen intent");
        }
        let reservation = self
            .load()?
            .into_iter()
            .find(|reservation| Some(reservation.id) == intent.reservation_id)
            .context("desktop admission reservation missing")?;
        if reservation.container_id.as_ref() != Some(&container.id)
            || reservation.run_id != run
            || reservation.resources != container.resources
        {
            bail!("desktop admission identity changed");
        }
        let created = text("/Created")?;
        let started = text("/State/StartedAt")?;
        if [created, started]
            .iter()
            .any(|time| time.is_empty() || time.len() > 64 || !time.contains('T'))
        {
            bail!("invalid desktop lifecycle timestamps");
        }
        let state = |name: &str| -> Result<bool> {
            value["State"][name]
                .as_bool()
                .context("desktop live state missing")
        };
        Ok(LiveJob {
            binding: JobBinding {
                version: 1,
                runner_id: identity.runner_id,
                engine_id: identity.engine_id.clone(),
                scope_prefix: self.prefix.clone(),
                reservation_id: reservation.id,
                attempt_id: attempt,
                container_id: container.id,
                container_name: container.name,
                created_at: created.to_owned(),
                started_at: started.to_owned(),
                run_id: run,
                session_id: session,
                lease_token: lease,
                workspace_id: workspace,
                workspace_volume: volume.to_owned(),
            },
            running: state("Running")?,
            paused: state("Paused")?,
            restarting: state("Restarting")?,
        })
    }
    fn require_live(job: &LiveJob) -> Result<()> {
        if !job.running || job.paused || job.restarting {
            bail!("desktop job is not continuously running");
        }
        Ok(())
    }
    pub(crate) async fn desktop_creation_receipt(
        &self,
        spec: &SandboxDesktopSessionSpec,
        lease: Uuid,
        attempt: Uuid,
    ) -> Result<LiveJob> {
        self.desktop_creation_receipt_with(
            spec,
            lease,
            attempt,
            || self.engine_id(),
            |id| async move { self.inspect_value(&id).await },
        )
        .await
    }
    async fn desktop_creation_receipt_with<E, EFut, I, IFut>(
        &self,
        spec: &SandboxDesktopSessionSpec,
        lease: Uuid,
        attempt: Uuid,
        mut engine: E,
        mut inspect: I,
    ) -> Result<LiveJob>
    where
        E: FnMut() -> EFut,
        EFut: std::future::Future<Output = Result<String>>,
        I: FnMut(String) -> IFut,
        IFut: std::future::Future<Output = Result<serde_json::Value>>,
    {
        let _lock = self.lock().await?;
        let mut intent = self.intent(spec.session_id)?;
        if intent.stop_requested
            || intent.lease_token != lease
            || intent.attempt_id != attempt
            || intent.spec_sha256 != hex::encode(Sha256::digest(serde_json::to_vec(spec)?))
        {
            bail!("desktop receipt intent changed");
        }
        self.check_execution_open(intent.run_id, Some(lease))?;
        let identity = self.checked_identity(&engine().await?)?;
        let value = inspect(
            intent
                .container_id
                .clone()
                .context("desktop create is unconfirmed")?,
        )
        .await?;
        let job = self.physical_job(&intent, &identity, &value)?;
        Self::require_live(&job)?;
        if intent
            .binding
            .as_ref()
            .is_some_and(|binding| binding != &job.binding)
        {
            bail!("desktop creation binding changed");
        }
        if self.checked_identity(&engine().await?)? != identity {
            bail!("runner physical identity changed during desktop receipt");
        }
        intent.binding = Some(job.binding.clone());
        self.save_intent(&intent)?;
        Ok(job)
    }
    async fn checked_desktop(&self, session: Uuid, digest: &str) -> Result<LiveJob> {
        self.checked_desktop_with(
            session,
            digest,
            || self.engine_id(),
            |id| async move { self.inspect_value(&id).await },
        )
        .await
    }
    async fn checked_desktop_with<E, EFut, I, IFut>(
        &self,
        session: Uuid,
        digest: &str,
        mut engine: E,
        mut inspect: I,
    ) -> Result<LiveJob>
    where
        E: FnMut() -> EFut,
        EFut: std::future::Future<Output = Result<String>>,
        I: FnMut(String) -> IFut,
        IFut: std::future::Future<Output = Result<serde_json::Value>>,
    {
        let intent = self.intent(session)?;
        if intent.stop_requested {
            bail!("desktop stop is pending");
        }
        let binding = intent
            .binding
            .as_ref()
            .context("desktop creation receipt was never confirmed")?;
        if binding.digest()? != digest {
            bail!("desktop expected physical binding differs");
        }
        self.check_execution_open(intent.run_id, Some(intent.lease_token))?;
        let identity = self.checked_identity(&engine().await?)?;
        let value = inspect(binding.container_id.clone()).await?;
        let job = self.physical_job(&intent, &identity, &value)?;
        Self::require_live(&job)?;
        if job.binding != *binding {
            bail!("desktop physical job was replaced or restarted");
        }
        if self.checked_identity(&engine().await?)? != identity {
            bail!("runner physical identity changed during desktop inspection");
        }
        Ok(job)
    }
    pub(crate) async fn probe_desktop(&self, request: &ProbeRequest) -> Result<serde_json::Value> {
        self.probe_desktop_with(
            request,
            || self.engine_id(),
            |id| async move { self.inspect_value(&id).await },
        )
        .await
    }
    async fn probe_desktop_with<E, EFut, I, IFut>(
        &self,
        request: &ProbeRequest,
        engine: E,
        inspect: I,
    ) -> Result<serde_json::Value>
    where
        E: FnMut() -> EFut,
        EFut: std::future::Future<Output = Result<String>>,
        I: FnMut(String) -> IFut,
        IFut: std::future::Future<Output = Result<serde_json::Value>>,
    {
        if request.version != 1 || request.challenge.is_nil() {
            bail!("invalid private desktop probe");
        }
        let _lock = self.lock().await?;
        let job = self
            .checked_desktop_with(
                request.binding.session_id,
                &request.binding.digest()?,
                engine,
                inspect,
            )
            .await?;
        if job.binding != request.binding {
            bail!("desktop probe identity changed");
        }
        Ok(serde_json::json!({"version":1,"challenge":request.challenge,"job":job}))
    }
    pub(crate) async fn desktop_stream_id(&self, session: Uuid, digest: &str) -> Result<String> {
        let _lock = self.lock().await?;
        Ok(self
            .checked_desktop(session, digest)
            .await?
            .binding
            .container_id)
    }
    pub(crate) async fn stop_bound_desktop(
        &self,
        session: Uuid,
        digest: &str,
    ) -> Result<serde_json::Value> {
        self.stop_bound_desktop_with(
            session,
            digest,
            || self.engine_id(),
            |id| async move {
                if self.inspect_exact(&id).await?.is_none() {
                    return Ok(None);
                }
                self.inspect_value(&id).await.map(Some)
            },
            |id| async move { crate::docker(&["rm", "--force", &id]).await.map(|_| ()) },
        )
        .await
    }
    async fn stop_bound_desktop_with<E, EFut, I, IFut, R, RFut>(
        &self,
        session: Uuid,
        digest: &str,
        mut engine: E,
        mut inspect: I,
        mut remove: R,
    ) -> Result<serde_json::Value>
    where
        E: FnMut() -> EFut,
        EFut: std::future::Future<Output = Result<String>>,
        I: FnMut(String) -> IFut,
        IFut: std::future::Future<Output = Result<Option<serde_json::Value>>>,
        R: FnMut(String) -> RFut,
        RFut: std::future::Future<Output = Result<()>>,
    {
        let _lock = self.lock().await?;
        let mut intent = self.intent(session)?;
        let binding = intent
            .binding
            .clone()
            .context("desktop physical creation is unconfirmed")?;
        if binding.digest()? != digest {
            bail!("desktop stop binding differs");
        }
        let identity = self.checked_identity(&engine().await?)?;
        if binding.runner_id != identity.runner_id
            || binding.engine_id != identity.engine_id
            || binding.scope_prefix != identity.scope_prefix
        {
            bail!("desktop stop belongs to a different persistent runner identity");
        }
        let previously_requested = intent.stop_requested;
        // Commit uncertainty before remote stop; later probes cannot adopt it.
        intent.stop_requested = true;
        self.save_intent(&intent)?;
        if let Some(value) = inspect(binding.container_id.clone()).await? {
            if previously_requested {
                bail!("previous desktop stop is unresolved; no physical stop replay permitted");
            }
            let job = self.physical_job(&intent, &identity, &value)?;
            if job.binding != binding {
                bail!("desktop stop targets a changed physical job");
            }
            if self.checked_identity(&engine().await?)? != identity {
                bail!("runner physical identity changed before desktop stop");
            }
            let _ = remove(binding.container_id.clone()).await;
        }
        if inspect(binding.container_id.clone()).await?.is_some() {
            bail!("exact desktop stop remains unconfirmed");
        }
        if self.checked_identity(&engine().await?)? != identity {
            bail!("runner physical identity changed during desktop stop");
        }
        if let Some(reservation) = self
            .load()?
            .into_iter()
            .find(|reservation| reservation.id == binding.reservation_id)
        {
            if reservation.container_id.as_deref() != Some(binding.container_id.as_str())
                || reservation.run_id != binding.run_id
            {
                bail!("desktop reservation changed during stop");
            }
            self.remove(reservation.id)?;
        }
        Ok(serde_json::json!({"version":1,"binding":binding,"execution_stopped":true}))
    }
}

pub(crate) fn proof_response(
    domain: &[u8],
    key: &[u8],
    fields: [&str; 4],
    request: &[u8],
    response: &[u8],
    binding: Option<&[u8]>,
) -> Result<String> {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<Sha256>::new_from_slice(key)?;
    mac.update(domain);
    let request_hash = hex::encode(Sha256::digest(request));
    let response_hash = hex::encode(Sha256::digest(response));
    for field in fields
        .into_iter()
        .chain([request_hash.as_str(), "200", response_hash.as_str()])
    {
        mac.update(&(field.len() as u64).to_be_bytes());
        mac.update(field.as_bytes());
    }
    if let Some(binding) = binding {
        let hash = hex::encode(Sha256::digest(binding));
        mac.update(&(hash.len() as u64).to_be_bytes());
        mac.update(hash.as_bytes());
    }
    Ok(format!("v1:{}", hex::encode(mac.finalize().into_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    const ENGINE: &str = "owned-test-engine";
    struct Fixture {
        gate: Admission,
        spec: SandboxDesktopSessionSpec,
        lease: Uuid,
        attempt: Uuid,
        value: serde_json::Value,
    }
    impl Fixture {
        fn new() -> Self {
            let prefix = crate::scoped_resource("", "")
                .trim_end_matches('-')
                .to_owned()
                + "-";
            let gate = Admission::new(
                std::env::temp_dir().join(format!("cowork-desktop-proof-test-{}", Uuid::new_v4())),
                prefix.clone(),
                Limits {
                    cpu_nanos: 4_000_000_000,
                    memory_bytes: 8 * 1024 * 1024 * 1024,
                    concurrent_runs: 2,
                    containers: 4,
                },
            )
            .unwrap();
            let spec = SandboxDesktopSessionSpec {
                schema_version: cowork_contracts::SCHEMA_VERSION,
                run_id: Uuid::from_u128(5),
                session_id: Uuid::from_u128(4),
                workspace_id: Some(Uuid::from_u128(7)),
                dimensions: cowork_contracts::DesktopDimensions {
                    width: 1280,
                    height: 720,
                    scale_factor: 1.0,
                },
                network: cowork_contracts::SandboxNetwork::FilteredEgress,
                limits: cowork_contracts::SandboxLimits::default(),
            };
            let lease = Uuid::from_u128(6);
            let attempt = Uuid::from_u128(3);
            let reservation = Uuid::from_u128(2);
            save_private(
                &gate.directory,
                "runner-identity",
                &Identity {
                    version: 1,
                    runner_id: Uuid::from_u128(1),
                    engine_id: ENGINE.into(),
                    scope_prefix: prefix.clone(),
                },
            )
            .unwrap();
            let value = serde_json::json!({"Id":"a".repeat(64),"Name":format!("/{prefix}desktop-{}",spec.session_id.simple()),"Created":"2026-10-09T00:00:00Z",
                "State":{"StartedAt":"2026-10-09T00:00:01Z","Running":true,"Paused":false,"Restarting":false},
                "HostConfig":{"NanoCpus":spec.limits.cpu_nanos,"Memory":spec.limits.memory_bytes},
                "Config":{"Labels":{"dev.opencowork.run_id":spec.run_id.to_string(),"dev.opencowork.desktop_session":spec.session_id.to_string(),"dev.opencowork.desktop_lease":lease.to_string(),"dev.opencowork.desktop_attempt":attempt.to_string(),"dev.opencowork.workspace_id":spec.workspace_id.unwrap().to_string(),"dev.opencowork.admission":reservation.to_string(),"dev.opencowork.runner_managed":"true"}},
                "Mounts":[{"Destination":"/workspace","Name":crate::workspace_volume(spec.run_id,spec.workspace_id),"Type":"volume","RW":true}]});
            Self {
                gate,
                spec,
                lease,
                attempt,
                value,
            }
        }
        async fn prepare(&self) {
            self.gate
                .begin_execution(
                    self.spec.run_id,
                    self.spec.workspace_id,
                    &Execution {
                        version: 1,
                        lease_token: self.lease,
                        attempt_id: self.attempt,
                    },
                    &serde_json::to_vec(&self.spec).unwrap(),
                )
                .await
                .unwrap();
            self.gate
                .prepare_desktop_identity_with(&self.spec, self.lease, self.attempt, async {
                    Ok(ENGINE.into())
                })
                .await
                .unwrap();
        }
        async fn bind(&self) {
            let reservation = Reservation {
                id: Uuid::from_u128(2),
                run_id: self.spec.run_id,
                container_name: self.value["Name"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches('/')
                    .into(),
                container_id: Some("a".repeat(64)),
                resources: Resources {
                    cpu_nanos: self.spec.limits.cpu_nanos,
                    memory_bytes: self.spec.limits.memory_bytes,
                },
            };
            self.gate.save(&reservation).unwrap();
            self.gate
                .bind_desktop_container(
                    self.spec.session_id,
                    &Permit { reservation },
                    &"a".repeat(64),
                )
                .await
                .unwrap();
        }
        async fn receipt(&self) -> LiveJob {
            self.gate
                .desktop_creation_receipt_with(
                    &self.spec,
                    self.lease,
                    self.attempt,
                    || async { Ok(ENGINE.into()) },
                    |id| {
                        assert_eq!(id, "a".repeat(64));
                        let value = self.value.clone();
                        async move { Ok(value) }
                    },
                )
                .await
                .unwrap()
        }
        async fn confirmed(&self) -> LiveJob {
            self.prepare().await;
            self.bind().await;
            self.receipt().await
        }
        async fn checked(
            &self,
            digest: &str,
            value: serde_json::Value,
            engine: &str,
        ) -> Result<LiveJob> {
            let _lock = self.gate.lock().await?;
            self.gate
                .checked_desktop_with(
                    self.spec.session_id,
                    digest,
                    || async { Ok(engine.into()) },
                    |id| {
                        assert_eq!(id, "a".repeat(64));
                        let value = value.clone();
                        async move { Ok(value) }
                    },
                )
                .await
        }
        fn reopen(&self) -> Admission {
            Admission::new(
                self.gate.directory.clone(),
                self.gate.prefix.clone(),
                self.gate.limits.clone(),
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.gate.directory);
        }
    }

    async fn fake_stop(
        gate: &Admission,
        session: Uuid,
        digest: &str,
        live: Arc<Mutex<Option<serde_json::Value>>>,
        removed: Arc<Mutex<Vec<String>>>,
        confirm: bool,
    ) -> Result<serde_json::Value> {
        let inspected = live.clone();
        gate.stop_bound_desktop_with(
            session,
            digest,
            || async { Ok(ENGINE.into()) },
            move |id| {
                let inspected = inspected.clone();
                async move {
                    assert_eq!(id, "a".repeat(64));
                    let value = inspected.lock().unwrap().clone();
                    Ok(value)
                }
            },
            move |id| {
                let live = live.clone();
                let removed = removed.clone();
                async move {
                    removed.lock().unwrap().push(id);
                    if confirm {
                        *live.lock().unwrap() = None;
                    }
                    Ok(())
                }
            },
        )
        .await
    }

    #[tokio::test]
    async fn initial_intent_blocks_replay_and_unconfirmed_create_cannot_mint_receipt() {
        let fixture = Fixture::new();
        fixture.prepare().await;
        let mut other = fixture.spec.clone();
        other.session_id = Uuid::new_v4();
        assert!(fixture
            .reopen()
            .prepare_desktop_identity_with(&other, fixture.lease, fixture.attempt, async {
                Ok(ENGINE.into())
            })
            .await
            .is_err());
        assert!(fixture
            .gate
            .desktop_creation_receipt_with(
                &fixture.spec,
                fixture.lease,
                fixture.attempt,
                || async { Ok(ENGINE.into()) },
                |_| async { panic!("unknown Create must not inspect a name") }
            )
            .await
            .is_err());
        fixture.bind().await;
        let receipt = fixture.receipt().await;
        assert_eq!(receipt.binding.attempt_id, fixture.attempt);
        assert_eq!(
            fixture
                .reopen()
                .intent(fixture.spec.session_id)
                .unwrap()
                .binding
                .unwrap(),
            receipt.binding
        );
        assert!(fixture
            .reopen()
            .checked_identity("different-engine")
            .is_err());
    }

    #[tokio::test]
    async fn physical_identity_and_live_state_are_required_by_probe_and_rfb_core() {
        let fixture = Fixture::new();
        let binding = fixture.confirmed().await.binding;
        let digest = binding.digest().unwrap();
        assert!(fixture
            .checked(&digest, fixture.value.clone(), ENGINE)
            .await
            .is_ok());
        assert!(fixture
            .checked(&digest, fixture.value.clone(), "other-engine")
            .await
            .is_err());
        for (pointer, replacement) in [
            ("/Id", serde_json::json!("b".repeat(64))),
            ("/Name", serde_json::json!("/renamed-job")),
            (
                "/Config/Labels/dev.opencowork.run_id",
                serde_json::json!(Uuid::new_v4().to_string()),
            ),
            (
                "/Config/Labels/dev.opencowork.desktop_lease",
                serde_json::json!(Uuid::new_v4().to_string()),
            ),
            (
                "/Config/Labels/dev.opencowork.desktop_attempt",
                serde_json::json!(Uuid::new_v4().to_string()),
            ),
            (
                "/Config/Labels/dev.opencowork.admission",
                serde_json::json!(Uuid::new_v4().to_string()),
            ),
            (
                "/Config/Labels/dev.opencowork.workspace_id",
                serde_json::json!(Uuid::new_v4().to_string()),
            ),
            (
                "/State/StartedAt",
                serde_json::json!("2026-10-09T00:00:02Z"),
            ),
            ("/State/Running", serde_json::json!(false)),
            ("/State/Paused", serde_json::json!(true)),
            ("/State/Restarting", serde_json::json!(true)),
            ("/HostConfig/NanoCpus", serde_json::json!(1)),
            ("/Mounts/0/Name", serde_json::json!("replacement-workspace")),
        ] {
            let mut value = fixture.value.clone();
            *value.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                fixture.checked(&digest, value, ENGINE).await.is_err(),
                "accepted mutated {pointer}"
            );
        }
        assert!(fixture
            .checked(&"0".repeat(64), fixture.value.clone(), ENGINE)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn fresh_probe_echoes_the_challenge_and_exact_persisted_binding() {
        let fixture = Fixture::new();
        let binding = fixture.confirmed().await.binding;
        let challenge = Uuid::new_v4();
        let request = ProbeRequest {
            version: 1,
            challenge,
            binding: binding.clone(),
        };
        let reply = fixture
            .gate
            .probe_desktop_with(
                &request,
                || async { Ok(ENGINE.into()) },
                |id| {
                    assert_eq!(id, binding.container_id);
                    let value = fixture.value.clone();
                    async move { Ok(value) }
                },
            )
            .await
            .unwrap();
        assert_eq!(reply["challenge"], challenge.to_string());
        assert_eq!(
            reply["job"]["binding"],
            serde_json::to_value(&binding).unwrap()
        );
        assert_eq!(reply["job"]["running"], true);
        assert_eq!(
            fixture
                .gate
                .intent(fixture.spec.session_id)
                .unwrap()
                .binding,
            Some(binding)
        );
    }

    #[tokio::test]
    async fn invalid_or_unknown_probe_has_no_journal_write_or_create_side_effect() {
        let fixture = Fixture::new();
        let directory = fixture.gate.directory.join("desktop-identities");
        assert!(!directory.exists());
        assert!(fixture.gate.intent(fixture.spec.session_id).is_err());
        assert!(!directory.exists());
        let binding = JobBinding {
            version: 1,
            runner_id: Uuid::from_u128(1),
            engine_id: ENGINE.into(),
            scope_prefix: fixture.gate.prefix.clone(),
            reservation_id: Uuid::from_u128(2),
            attempt_id: fixture.attempt,
            container_id: "a".repeat(64),
            container_name: format!(
                "{}desktop-{}",
                fixture.gate.prefix,
                fixture.spec.session_id.simple()
            ),
            created_at: "2026-10-09T00:00:00Z".into(),
            started_at: "2026-10-09T00:00:01Z".into(),
            run_id: fixture.spec.run_id,
            session_id: fixture.spec.session_id,
            lease_token: fixture.lease,
            workspace_id: fixture.spec.workspace_id,
            workspace_volume: crate::workspace_volume(
                fixture.spec.run_id,
                fixture.spec.workspace_id,
            ),
        };
        assert!(fixture
            .gate
            .probe_desktop(&ProbeRequest {
                version: 1,
                challenge: Uuid::nil(),
                binding: binding.clone()
            })
            .await
            .is_err());
        assert!(fixture
            .gate
            .probe_desktop(&ProbeRequest {
                version: 1,
                challenge: Uuid::new_v4(),
                binding
            })
            .await
            .is_err());
        assert!(!directory.exists());
        assert!(fixture.gate.load().unwrap().is_empty());
    }

    #[tokio::test]
    async fn confirmed_exact_stop_retains_identity_and_repeated_stop_is_readonly() {
        let fixture = Fixture::new();
        let binding = fixture.confirmed().await.binding;
        let digest = binding.digest().unwrap();
        let live = Arc::new(Mutex::new(Some(fixture.value.clone())));
        let removed = Arc::new(Mutex::new(Vec::new()));
        let first = fake_stop(
            &fixture.gate,
            fixture.spec.session_id,
            &digest,
            live.clone(),
            removed.clone(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(first["execution_stopped"], true);
        assert!(fixture.gate.load().unwrap().is_empty());
        let reopened = fixture.reopen();
        let again = fake_stop(
            &reopened,
            fixture.spec.session_id,
            &digest,
            live,
            removed.clone(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(first, again);
        assert_eq!(*removed.lock().unwrap(), vec!["a".repeat(64)]);
        assert_eq!(
            reopened.intent(fixture.spec.session_id).unwrap().binding,
            Some(binding)
        );
        assert!(fixture
            .checked(&digest, fixture.value.clone(), ENGINE)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn unconfirmed_stop_and_restarted_runner_never_repeat_physical_stop_or_release_charge() {
        let fixture = Fixture::new();
        let digest = fixture.confirmed().await.binding.digest().unwrap();
        let live = Arc::new(Mutex::new(Some(fixture.value.clone())));
        let removed = Arc::new(Mutex::new(Vec::new()));
        assert!(fake_stop(
            &fixture.gate,
            fixture.spec.session_id,
            &digest,
            live.clone(),
            removed.clone(),
            false
        )
        .await
        .is_err());
        assert!(fake_stop(
            &fixture.reopen(),
            fixture.spec.session_id,
            &digest,
            live.clone(),
            removed.clone(),
            true
        )
        .await
        .is_err());
        assert_eq!(removed.lock().unwrap().len(), 1);
        assert_eq!(fixture.gate.load().unwrap().len(), 1);
        *live.lock().unwrap() = None;
        assert!(fake_stop(
            &fixture.reopen(),
            fixture.spec.session_id,
            &digest,
            live,
            removed.clone(),
            true
        )
        .await
        .is_ok());
        assert_eq!(removed.lock().unwrap().len(), 1);
        assert!(fixture.gate.load().unwrap().is_empty());
    }

    #[tokio::test]
    async fn absence_cannot_mint_a_stop_proof_after_journal_identity_replacement() {
        let fixture = Fixture::new();
        let digest = fixture.confirmed().await.binding.digest().unwrap();
        save_private(
            &fixture.gate.directory,
            "runner-identity",
            &Identity {
                version: 1,
                runner_id: Uuid::new_v4(),
                engine_id: ENGINE.into(),
                scope_prefix: fixture.gate.prefix.clone(),
            },
        )
        .unwrap();
        let removed = Arc::new(Mutex::new(Vec::new()));
        assert!(fake_stop(
            &fixture.gate,
            fixture.spec.session_id,
            &digest,
            Arc::new(Mutex::new(None)),
            removed.clone(),
            true
        )
        .await
        .is_err());
        assert!(removed.lock().unwrap().is_empty());
        assert_eq!(fixture.gate.load().unwrap().len(), 1);
        assert!(
            !fixture
                .gate
                .intent(fixture.spec.session_id)
                .unwrap()
                .stop_requested
        );
    }

    #[tokio::test]
    async fn engine_switch_during_receipt_probe_or_absence_check_cannot_mint_authority() {
        let fixture = Fixture::new();
        fixture.prepare().await;
        fixture.bind().await;
        let mut calls = 0;
        assert!(fixture
            .gate
            .desktop_creation_receipt_with(
                &fixture.spec,
                fixture.lease,
                fixture.attempt,
                || {
                    calls += 1;
                    let engine = if calls == 1 { ENGINE } else { "other-engine" };
                    async move { Ok(engine.into()) }
                },
                |_| {
                    let value = fixture.value.clone();
                    async move { Ok(value) }
                }
            )
            .await
            .is_err());
        assert!(fixture
            .gate
            .intent(fixture.spec.session_id)
            .unwrap()
            .binding
            .is_none());
        let binding = fixture.receipt().await.binding;
        let digest = binding.digest().unwrap();
        let request = ProbeRequest {
            version: 1,
            challenge: Uuid::new_v4(),
            binding,
        };
        calls = 0;
        assert!(fixture
            .gate
            .probe_desktop_with(
                &request,
                || {
                    calls += 1;
                    let engine = if calls == 1 { ENGINE } else { "other-engine" };
                    async move { Ok(engine.into()) }
                },
                |_| {
                    let value = fixture.value.clone();
                    async move { Ok(value) }
                }
            )
            .await
            .is_err());
        calls = 0;
        assert!(fixture
            .gate
            .stop_bound_desktop_with(
                fixture.spec.session_id,
                &digest,
                || {
                    calls += 1;
                    let engine = if calls == 1 { ENGINE } else { "other-engine" };
                    async move { Ok(engine.into()) }
                },
                |_| async { Ok(None) },
                |_| async { panic!("absence check must not stop a container") }
            )
            .await
            .is_err());
        assert_eq!(fixture.gate.load().unwrap().len(), 1);
        assert!(
            fixture
                .gate
                .intent(fixture.spec.session_id)
                .unwrap()
                .stop_requested
        );
    }

    #[tokio::test]
    async fn canceled_stop_future_durably_blocks_adoption_and_unsafe_physical_retry() {
        let fixture = Fixture::new();
        let digest = fixture.confirmed().await.binding.digest().unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        let mut pending = Box::pin(fixture.gate.stop_bound_desktop_with(
            fixture.spec.session_id,
            &digest,
            || async { Ok(ENGINE.into()) },
            |_| {
                let value = fixture.value.clone();
                async move { Ok(Some(value)) }
            },
            move |_| {
                let signal = signal.clone();
                async move {
                    signal.notify_one();
                    std::future::pending::<Result<()>>().await
                }
            },
        ));
        tokio::select! {result=&mut pending=>panic!("unexpected completion {result:?}"),_=entered.notified()=>{}}
        drop(pending);
        assert!(
            fixture
                .gate
                .intent(fixture.spec.session_id)
                .unwrap()
                .stop_requested
        );
        let removed = Arc::new(Mutex::new(Vec::new()));
        assert!(fake_stop(
            &fixture.reopen(),
            fixture.spec.session_id,
            &digest,
            Arc::new(Mutex::new(Some(fixture.value.clone()))),
            removed.clone(),
            true
        )
        .await
        .is_err());
        assert!(removed.lock().unwrap().is_empty());
        assert_eq!(fixture.gate.load().unwrap().len(), 1);
    }

    #[test]
    fn response_proof_domains_and_each_request_field_are_independent() {
        let fields = [
            "POST",
            "/v1/desktop-sessions?lease_token=exact&attempt_id=exact",
            "1700000000",
            "00000000-0000-4000-8000-000000000001",
        ];
        let signed = proof_response(
            CREATE_DOMAIN,
            &[b'k'; 32],
            fields,
            b"request",
            b"response",
            Some(b"binding"),
        )
        .unwrap();
        for index in 0..fields.len() {
            let mut changed = fields;
            changed[index] = "changed";
            assert_ne!(
                signed,
                proof_response(
                    CREATE_DOMAIN,
                    &[b'k'; 32],
                    changed,
                    b"request",
                    b"response",
                    Some(b"binding")
                )
                .unwrap()
            );
        }
        for domain in [
            PROBE_DOMAIN,
            STOP_DOMAIN,
            b"open-cowork/runner-stop/v1\0".as_slice(),
            b"open-cowork/runner-no-start/v1\0".as_slice(),
        ] {
            assert_ne!(
                signed,
                proof_response(
                    domain,
                    &[b'k'; 32],
                    fields,
                    b"request",
                    b"response",
                    Some(b"binding")
                )
                .unwrap()
            );
        }
        for (request, response, binding) in [
            (
                b"changed".as_slice(),
                b"response".as_slice(),
                Some(b"binding".as_slice()),
            ),
            (
                b"request".as_slice(),
                b"changed".as_slice(),
                Some(b"binding".as_slice()),
            ),
            (
                b"request".as_slice(),
                b"response".as_slice(),
                Some(b"changed".as_slice()),
            ),
            (b"request".as_slice(), b"response".as_slice(), None),
        ] {
            assert_ne!(
                signed,
                proof_response(
                    CREATE_DOMAIN,
                    &[b'k'; 32],
                    fields,
                    request,
                    response,
                    binding
                )
                .unwrap()
            );
        }
    }
}
