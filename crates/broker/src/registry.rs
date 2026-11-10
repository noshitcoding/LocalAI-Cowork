use super::*;

const REGISTRY: &str = "folder-registry.json";
const MAX_REGISTRY_BYTES: u64 = 1_048_576;

/// Capture the native picker's selected identity before asking for human confirmation.
pub fn inspect_native_folder(path: &Path) -> Result<(PathBuf, String)> {
    if !path.is_absolute() {
        return Err(Error::Forbidden);
    }
    let canonical = std::fs::canonicalize(path).map_err(storage_error)?;
    let display = canonical.to_str().ok_or(Error::Forbidden)?;
    if display
        .chars()
        .any(|c| c.is_control() || matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}'))
    {
        return Err(Error::Forbidden);
    }
    let dir = Dir::open_ambient_dir(&canonical, ambient_authority()).map_err(storage_error)?;
    let metadata = dir.dir_metadata().map_err(storage_error)?;
    Ok((
        canonical,
        hash(format!("{}-{}", metadata.dev(), metadata.ino()).as_bytes()),
    ))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    key: String,
    request_hash: String,
    config: FolderConfig,
    canonical: PathBuf,
    identity: String,
    grant: FolderGrant,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    format: u32,
    revision: u64,
    registrations: Vec<Registration>,
    revocations: Vec<FolderGrant>,
}

fn grant(config: &FolderConfig, canonical: &Path, identity: &str) -> FolderGrant {
    FolderGrant {
        order_id: None,
        resource_id: hash(identity.as_bytes()),
        id: config.id.clone(),
        project_id: config.project_id.clone(),
        name: config.name.clone(),
        writable: config.writable,
        revision: canonical_hash(
            &json!({"config":config,"canonical":canonical,"identity":identity}),
        ),
    }
}
fn normalized(path: &Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    } else {
        path.to_owned()
    }
}
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

impl Registry {
    fn revoked(&self, id: &str) -> bool {
        self.revocations.iter().any(|g| g.id == id)
    }
    fn validate(&self) -> Result<()> {
        if self.format != 1
            || self.registrations.len() > 1024
            || self.revocations.len() > 2048
            || self.revision != (self.registrations.len() + self.revocations.len()) as u64
        {
            return Err(Error::Validation("Invalid native folder registry".into()));
        }
        let mut ids = std::collections::HashSet::new();
        let mut requests = std::collections::HashSet::new();
        for record in &self.registrations {
            valid_text(&record.key, 128)?;
            valid_text(&record.config.project_id, 128)?;
            valid_text(&record.config.name, 200)?;
            let identity: Vec<_> = record.identity.split('-').collect();
            if !record.config.id.starts_with("native:")
                || !record.config.path.is_absolute()
                || !record.canonical.is_absolute()
                || identity.len() != 2
                || identity.iter().any(|v| v.parse::<u64>().is_err())
                || record.grant != grant(&record.config, &record.canonical, &record.identity)
                || !ids.insert(&record.config.id)
                || !requests.insert((&record.config.project_id, &record.key))
            {
                return Err(Error::Forbidden);
            }
        }
        let mut revoked = std::collections::HashSet::new();
        if self.revocations.iter().any(|g| {
            g.order_id.is_some() || g.id.starts_with("workspace:") || !revoked.insert(&g.id)
        }) {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}

impl FileBroker {
    pub(super) fn validate_registry_config(&self) -> Result<()> {
        let registry = self.read_registry()?;
        if registry
            .registrations
            .iter()
            .filter(|r| !registry.revoked(&r.grant.id))
            .any(|r| {
                self.configured_paths.iter().any(|(id, p)| {
                    !registry.revoked(id) && overlaps(&normalized(p), &normalized(&r.canonical))
                }) || self
                    .protected_paths
                    .iter()
                    .any(|p| overlaps(p, &normalized(&r.canonical)))
            })
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
    fn read_registry(&self) -> Result<Registry> {
        let registry = match read_regular_limit(&self.registry_state, REGISTRY, MAX_REGISTRY_BYTES)?
        {
            Some(bytes) => serde_json::from_slice::<Registry>(&bytes)
                .map_err(|_| Error::Validation("Native folder registry is unreadable".into()))?,
            None => Registry {
                format: 1,
                revision: 0,
                registrations: vec![],
                revocations: vec![],
            },
        };
        registry.validate()?;
        // This policy applies to ordinary brokers as well as long-path document users.
        // Recheck fresh native names before exposing even a cached configured grant.
        let roots = self.roots.read().map_err(|_| Error::Forbidden)?;
        for (id, path) in &self.configured_paths {
            if !registry.revoked(id) {
                let root = roots.get(id).ok_or(Error::Forbidden)?;
                self.require_not_document_workspace(path, Some(&root.dir))?;
            }
        }
        for record in &registry.registrations {
            if !registry.revoked(&record.grant.id) {
                self.require_not_document_workspace(&record.config.path, None)?;
                self.require_not_document_workspace(&record.canonical, None)?;
            }
        }
        Ok(registry)
    }

    /// Caller owns the registry writer lock. Publication is atomic; no in-memory cache is authoritative.
    fn write_registry(&self, registry: &Registry) -> Result<()> {
        registry.validate()?;
        let bytes = serde_json::to_vec(registry).map_err(storage_error)?;
        if bytes.len() as u64 > MAX_REGISTRY_BYTES {
            return Err(Error::Validation("Native folder registry is full".into()));
        }
        let stage = format!(".folder-registry-{}", uuid::Uuid::new_v4());
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = self
            .registry_state
            .open_with(&stage, &options)
            .map_err(storage_error)?;
        if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = self.registry_state.remove_file(&stage);
            return Err(storage_error(error));
        }
        drop(file);
        self.registry_state
            .rename(&stage, &self.registry_state, REGISTRY)
            .map_err(|_| Error::UncertainEffect)?;
        #[cfg(unix)]
        sync_directory(&self.registry_state).map_err(|_| Error::UncertainEffect)?;
        if read_regular_limit(&self.registry_state, REGISTRY, MAX_REGISTRY_BYTES)
            .map_err(|_| Error::UncertainEffect)?
            .as_deref()
            != Some(bytes.as_slice())
        {
            return Err(Error::UncertainEffect);
        }
        Ok(())
    }

    fn open_registration(&self, record: &Registration) -> Result<Arc<Root>> {
        self.require_not_document_workspace(&record.config.path, None)?;
        let canonical = std::fs::canonicalize(&record.canonical).map_err(storage_error)?;
        if normalized(&canonical) != normalized(&record.canonical)
            || self
                .protected_paths
                .iter()
                .any(|p| overlaps(p, &normalized(&canonical)))
        {
            return Err(Error::Forbidden);
        }
        let dir = Dir::open_ambient_dir(&canonical, ambient_authority()).map_err(storage_error)?;
        self.require_not_document_workspace(&canonical, Some(&dir))?;
        let metadata = dir.dir_metadata().map_err(storage_error)?;
        if format!("{}-{}", metadata.dev(), metadata.ino()) != record.identity {
            return Err(Error::Forbidden);
        }
        Ok(Arc::new(Root {
            namespace: None,
            grant: record.grant.clone(),
            dir: Arc::new(dir),
            lock_name: record.identity.clone(),
        }))
    }

    pub(super) fn lookup_root(&self, id: &str, reconciliation: bool) -> Result<Arc<Root>> {
        let registry = self.read_registry()?;
        if !reconciliation && registry.revoked(id) {
            return Err(Error::Forbidden);
        }
        if let Some(record) = registry.registrations.iter().find(|r| r.grant.id == id) {
            return self.open_registration(record);
        }
        let root = self
            .roots
            .read()
            .map_err(|_| Error::Forbidden)?
            .get(id)
            .cloned()
            .ok_or(Error::Forbidden)?;
        // Revocation permits observation of an original effect, never bypass of
        // the native private namespace policy on a cached configured capability.
        if let Some(path) = self.configured_paths.get(id) {
            self.require_not_document_workspace(path, Some(&root.dir))?;
        }
        Ok(root)
    }

    pub(super) fn current_grants(&self, project: &str) -> Result<Vec<FolderGrant>> {
        let registry = self.read_registry()?;
        let roots = self.roots.read().map_err(|_| Error::Forbidden)?;
        let mut grants: Vec<_> = roots
            .values()
            .filter(|r| {
                r.grant.project_id == project
                    && r.grant.order_id.is_none()
                    && !registry.revoked(&r.grant.id)
            })
            .map(|r| r.grant.clone())
            .chain(
                registry
                    .registrations
                    .iter()
                    .filter(|r| r.grant.project_id == project && !registry.revoked(&r.grant.id))
                    .map(|r| r.grant.clone()),
            )
            .collect();
        grants.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        Ok(grants)
    }

    pub(super) fn require_active(&self, grant: &FolderGrant) -> Result<()> {
        if self.lookup_root(&grant.id, false)?.grant != *grant {
            return Err(Error::Forbidden);
        }
        Ok(())
    }

    /// A human can reconcile an already journaled effect after revocation. This grants no new IO action.
    pub(super) fn reconciliation_root(&self, run: &RunRecord, id: &str) -> Result<Arc<Root>> {
        let root = self.spec_root(&run.spec, id, true)?;
        if root.grant.project_id != run.spec.project_id
            || root
                .grant
                .order_id
                .as_ref()
                .is_some_and(|o| o != &run.spec.order_id)
            || !root.grant.writable
            || run.spec.access_mode == AccessMode::ReadOnly
            || !run.spec.folders.contains(&root.grant)
        {
            return Err(Error::Forbidden);
        }
        Ok(root)
    }

    pub fn registered_folders(&self, project: &str) -> Result<Vec<RegisteredFolder>> {
        let registry = self.read_registry()?;
        let roots = self.roots.read().map_err(|_| Error::Forbidden)?;
        let mut result: Vec<_> = self
            .configured_paths
            .iter()
            .filter_map(|(id, path)| {
                roots
                    .get(id)
                    .filter(|r| r.grant.project_id == project)
                    .map(|r| RegisteredFolder {
                        grant: r.grant.clone(),
                        canonical_path: path.clone(),
                        revoked: registry.revoked(id),
                    })
            })
            .collect();
        result.extend(
            registry
                .registrations
                .iter()
                .filter(|r| r.grant.project_id == project)
                .map(|r| RegisteredFolder {
                    grant: r.grant.clone(),
                    canonical_path: r.canonical.clone(),
                    revoked: registry.revoked(&r.grant.id),
                }),
        );
        result.sort_by(|a, b| {
            a.grant
                .name
                .cmp(&b.grant.name)
                .then(a.grant.id.cmp(&b.grant.id))
        });
        Ok(result)
    }

    pub fn registration_for_request(
        &self,
        project: &str,
        key: &str,
    ) -> Result<Option<RegisteredFolder>> {
        let registry = self.read_registry()?;
        Ok(registry
            .registrations
            .iter()
            .find(|r| r.config.project_id == project && r.key == key)
            .map(|r| RegisteredFolder {
                grant: r.grant.clone(),
                canonical_path: r.canonical.clone(),
                revoked: registry.revoked(&r.grant.id),
            }))
    }

    pub fn register_native_folder(
        &self,
        input: &NativeFolderSelection,
    ) -> Result<RegisteredFolder> {
        valid_text(&input.idempotency_key, 128)?;
        valid_text(&input.project_id, 128)?;
        valid_text(&input.name, 200)?;
        validate_hash(&input.expected_resource_id)?;
        if !input.path.is_absolute() {
            return Err(Error::Forbidden);
        }
        let _registry_lock = self.lock_named("folder-registry", true)?;
        let mut registry = self.read_registry()?;
        let request_hash = canonical_hash(input);
        if let Some(record) = registry
            .registrations
            .iter()
            .find(|r| r.config.project_id == input.project_id && r.key == input.idempotency_key)
        {
            if record.request_hash != request_hash {
                return Err(Error::Conflict);
            }
            return Ok(RegisteredFolder {
                grant: record.grant.clone(),
                canonical_path: record.canonical.clone(),
                revoked: registry.revoked(&record.grant.id),
            });
        }
        self.require_not_document_workspace(&input.path, None)?;
        let (canonical, _) = inspect_native_folder(&input.path)?;
        self.require_not_document_workspace(&canonical, None)?;
        let comparison = normalized(&canonical);
        if self
            .protected_paths
            .iter()
            .any(|p| overlaps(p, &comparison))
            || self
                .configured_paths
                .iter()
                .any(|(id, p)| !registry.revoked(id) && overlaps(&normalized(p), &comparison))
            || registry.registrations.iter().any(|r| {
                !registry.revoked(&r.grant.id) && overlaps(&normalized(&r.canonical), &comparison)
            })
        {
            return Err(Error::Forbidden);
        }
        let dir = Dir::open_ambient_dir(&canonical, ambient_authority()).map_err(storage_error)?;
        self.require_not_document_workspace(&canonical, Some(&dir))?;
        let metadata = dir.dir_metadata().map_err(storage_error)?;
        let identity = format!("{}-{}", metadata.dev(), metadata.ino());
        if hash(identity.as_bytes()) != input.expected_resource_id {
            return Err(Error::FileConflict);
        }
        let roots = self.roots.read().map_err(|_| Error::Forbidden)?;
        if roots
            .values()
            .any(|r| !registry.revoked(&r.grant.id) && r.lock_name == identity)
            || registry
                .registrations
                .iter()
                .any(|r| !registry.revoked(&r.grant.id) && r.identity == identity)
        {
            return Err(Error::Forbidden);
        }
        drop(roots);
        let config = FolderConfig {
            id: format!("native:{}", uuid::Uuid::new_v4()),
            project_id: input.project_id.clone(),
            name: input.name.clone(),
            path: input.path.clone(),
            writable: input.writable,
        };
        let grant = grant(&config, &canonical, &identity);
        let result = RegisteredFolder {
            grant: grant.clone(),
            canonical_path: canonical.clone(),
            revoked: false,
        };
        registry.registrations.push(Registration {
            key: input.idempotency_key.clone(),
            request_hash,
            config,
            canonical,
            identity,
            grant,
        });
        registry.revision += 1;
        self.write_registry(&registry)?;
        Ok(result)
    }

    pub fn revoke_native_folder(
        &self,
        project: &str,
        id: &str,
        expected_revision: &str,
    ) -> Result<FolderGrant> {
        let _registry_lock = self.lock_named("folder-registry", true)?;
        let mut registry = self.read_registry()?;
        if let Some(previous) = registry.revocations.iter().find(|g| g.id == id) {
            if previous.project_id != project || previous.revision != expected_revision {
                return Err(Error::Conflict);
            }
            return Ok(previous.clone());
        }
        let (grant, identity) =
            if let Some(record) = registry.registrations.iter().find(|r| r.grant.id == id) {
                (record.grant.clone(), record.identity.clone())
            } else {
                let roots = self.roots.read().map_err(|_| Error::Forbidden)?;
                let root = roots.get(id).ok_or(Error::NotFound)?;
                (root.grant.clone(), root.lock_name.clone())
            };
        if grant.project_id != project || grant.order_id.is_some() {
            return Err(Error::Forbidden);
        }
        if grant.revision != expected_revision {
            return Err(Error::Conflict);
        }
        // A confirmed revocation cannot overtake native IO whose receipt is still being committed.
        let _resource_lock = self.lock_named(&identity, true)?;
        registry.revocations.push(grant.clone());
        registry.revision += 1;
        self.write_registry(&registry)?;
        Ok(grant)
    }
}
