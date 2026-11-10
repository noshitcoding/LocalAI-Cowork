//! Database images are private owner data, never a portable credential/key export.
use super::*;
use cowork_storage::library_config::LibraryOwner;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path as FsPath, PathBuf},
    sync::Mutex,
};
#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    request_key: String,
    backup: DatabaseBackup,
}
#[derive(Default)]
struct Observation {
    last_error: Option<String>,
    last_cleanup_at: Option<i64>,
    cleaned_chats: u64,
}
pub struct MaintenanceService {
    store: cowork_storage::Store,
    toolbox: Option<Arc<cowork_toolbox::Toolbox>>,
    root: PathBuf,
    pg_dump: Option<PathBuf>,
    database_url: Option<String>,
    gate: tokio::sync::Mutex<()>,
    observed: Mutex<Observation>,
}
fn bad(_: impl std::fmt::Display) -> Error {
    Error::Validation(
        "Database maintenance could not be completed; inspect configuration and storage capacity"
            .into(),
    )
}
fn safe(path: &FsPath) -> cowork_domain::Result<()> {
    for parent in path.ancestors() {
        let Ok(m) = fs::symlink_metadata(parent) else {
            continue;
        };
        if m.is_symlink() {
            return Err(Error::Forbidden);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                return Err(Error::Forbidden);
            }
        }
    }
    Ok(())
}
fn hash_file(path: &FsPath) -> cowork_domain::Result<(u64, String)> {
    safe(path)?;
    let mut f = fs::File::open(path).map_err(bad)?;
    let mut h = Sha256::new();
    let mut bytes = 0;
    let mut buf = [0u8; 262144];
    loop {
        let n = f.read(&mut buf).map_err(bad)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        bytes += n as u64;
    }
    Ok((bytes, format!("{:x}", h.finalize())))
}
fn dump_connection(raw: &str) -> cowork_domain::Result<(String, Option<String>, Option<String>)> {
    let mut url = reqwest::Url::parse(raw).map_err(bad)?;
    if !["postgres", "postgresql"].contains(&url.scheme()) {
        return Err(Error::Forbidden);
    }
    let mut password = url.password().map(|value| {
        percent_encoding::percent_decode_str(value)
            .decode_utf8_lossy()
            .into_owned()
    });
    url.set_password(None).map_err(|_| Error::Forbidden)?;
    let mut ssl_password = None;
    let mut query = Vec::new();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "password" => password = Some(value.into_owned()),
            "sslpassword" => ssl_password = Some(value.into_owned()),
            _ => query.push((key.into_owned(), value.into_owned())),
        }
    }
    url.set_query(None);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Ok((url.into(), password, ssl_password))
}
impl MaintenanceService {
    pub fn open(
        store: cowork_storage::Store,
        data: &FsPath,
        toolbox: Option<Arc<cowork_toolbox::Toolbox>>,
    ) -> cowork_domain::Result<Arc<Self>> {
        let root = std::path::absolute(data)
            .map_err(bad)?
            .join("database-backups");
        safe(&root)?;
        fs::create_dir_all(&root).map_err(bad)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(bad)?;
        }
        let pg_dump = std::env::var_os("COWORK_PG_DUMP")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_file());
        Ok(Arc::new(Self {
            store,
            toolbox,
            root,
            pg_dump,
            database_url: std::env::var("COWORK_DATABASE_URL").ok(),
            gate: tokio::sync::Mutex::new(()),
            observed: Mutex::new(Observation::default()),
        }))
    }
    fn manifests(&self) -> cowork_domain::Result<Vec<Manifest>> {
        self.manifests_in(false)
    }
    fn manifests_in(&self, include_retired: bool) -> cowork_domain::Result<Vec<Manifest>> {
        safe(&self.root)?;
        let mut result = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(bad)? {
            let entry = entry.map_err(bad)?;
            let path = entry.path();
            if path
                .extension()
                .is_none_or(|e| e != "json" && (!include_retired || e != "receipt"))
            {
                continue;
            }
            safe(&path)?;
            if entry.metadata().map_err(bad)?.len() > 8192 {
                return Err(Error::Forbidden);
            }
            let manifest: Manifest =
                serde_json::from_slice(&fs::read(&path).map_err(bad)?).map_err(bad)?;
            if manifest.format != 1
                || uuid::Uuid::parse_str(&manifest.backup.id).is_err()
                || path.file_stem().and_then(|s| s.to_str()) != Some(&manifest.backup.id)
            {
                return Err(Error::Forbidden);
            }
            result.push(manifest);
        }
        result.sort_by_key(|v| v.backup.created_at);
        Ok(result)
    }
    fn request_receipt(&self, key: &str) -> cowork_domain::Result<Option<DatabaseBackup>> {
        safe(&self.root)?;
        for entry in fs::read_dir(&self.root).map_err(bad)? {
            let entry = entry.map_err(bad)?;
            let path = entry.path();
            if path
                .extension()
                .is_none_or(|v| v != "json" && v != "receipt")
            {
                continue;
            }
            safe(&path)?;
            if entry.metadata().map_err(bad)?.len() > 8192 {
                return Err(Error::Forbidden);
            }
            let manifest: Manifest =
                serde_json::from_slice(&fs::read(&path).map_err(bad)?).map_err(bad)?;
            if manifest.format != 1
                || uuid::Uuid::parse_str(&manifest.backup.id).is_err()
                || path.file_stem().and_then(|s| s.to_str()) != Some(&manifest.backup.id)
            {
                return Err(Error::Forbidden);
            }
            if manifest.request_key == key {
                return Ok(Some(manifest.backup));
            }
        }
        Ok(None)
    }
    fn available(&self) -> bool {
        self.store.database_backend() == "sqlite"
            || self.pg_dump.is_some() && self.database_url.is_some()
    }
    pub async fn status(&self) -> cowork_domain::Result<MaintenanceStatus> {
        let preferences = self.store.library_preferences(LibraryOwner::Local).await?;
        let backups = self.manifests()?.into_iter().map(|v| v.backup).collect();
        let o = self.observed.lock().map_err(|_| Error::Conflict)?;
        Ok(MaintenanceStatus {
            preferences,
            backend: self.store.database_backend().into(),
            backup_available: self.available(),
            backups,
            last_error: o.last_error.clone(),
            last_cleanup_at: o.last_cleanup_at,
            cleaned_chats: o.cleaned_chats,
        })
    }
    pub async fn backup(&self, key: &str) -> cowork_domain::Result<DatabaseBackup> {
        if key.is_empty() || key.len() > 128 {
            return Err(Error::Validation("Invalid backup key".into()));
        }
        let _guard = self.gate.lock().await;
        if let Some(saved) = self.request_receipt(key)? {
            return Ok(saved);
        }
        if !self.available() {
            return Err(Error::Validation(
                "Set COWORK_PG_DUMP to the absolute pg_dump executable path for PostgreSQL backups"
                    .into(),
            ));
        }
        let id = id();
        let partial = self.root.join(format!("{id}.partial"));
        safe(&partial)?;
        if self.store.database_backend() == "sqlite" {
            self.store.database_backup_sqlite(&partial).await?;
        } else {
            let (url, password, ssl_password) =
                dump_connection(self.database_url.as_deref().ok_or(Error::Forbidden)?)?;
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)
                .map_err(bad)?;
            let mut command =
                tokio::process::Command::new(self.pg_dump.as_ref().ok_or(Error::Forbidden)?);
            command
                .args(["--format=custom", "--no-password", "--dbname", url.as_str()])
                .stdin(std::process::Stdio::null())
                .stdout(file)
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            command.env_clear();
            for name in ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"] {
                if let Some(v) = std::env::var_os(name) {
                    command.env(name, v);
                }
            }
            if let Some(password) = password {
                command.env("PGPASSWORD", password);
            }
            if let Some(password) = ssl_password {
                command.env("PGSSLPASSWORD", password);
            }
            #[cfg(windows)]
            command.creation_flags(0x08000000);
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(600), command.status())
                    .await
                    .map_err(bad)?
                    .map_err(bad)?;
            if !result.success() {
                return Err(bad("pg_dump"));
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&partial, fs::Permissions::from_mode(0o600)).map_err(bad)?;
        }
        let (bytes, sha256) = hash_file(&partial)?;
        if bytes == 0 {
            return Err(bad("empty backup"));
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&partial)
            .map_err(bad)?
            .sync_all()
            .map_err(bad)?;
        let backup = DatabaseBackup {
            id: id.clone(),
            created_at: cowork_storage::now(),
            bytes,
            sha256,
            format: if self.store.database_backend() == "sqlite" {
                "sqlite3"
            } else {
                "postgres-custom"
            }
            .into(),
        };
        let final_path = self.root.join(format!("{id}.backup"));
        fs::rename(&partial, &final_path).map_err(bad)?;
        let manifest = Manifest {
            format: 1,
            request_key: key.into(),
            backup: backup.clone(),
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.root.join(format!("{id}.json")))
            .map_err(bad)?;
        file.write_all(&serde_json::to_vec(&manifest).map_err(bad)?)
            .map_err(bad)?;
        file.sync_all().map_err(bad)?;
        // Only immutable files with our manifest, identity and matching digest are pruned.
        let list = self.manifests()?;
        let excess = list.len().saturating_sub(7);
        for old in list.into_iter().take(excess) {
            let path = self.root.join(format!("{}.backup", old.backup.id));
            if hash_file(&path)? != (old.backup.bytes, old.backup.sha256) {
                return Err(Error::Forbidden);
            }
            // Preserve the original ACK even after its image expires. Retrying an
            // old manual request must never generate a new physical backup.
            fs::rename(
                self.root.join(format!("{}.json", old.backup.id)),
                self.root.join(format!("{}.receipt", old.backup.id)),
            )
            .map_err(bad)?;
            fs::remove_file(path).map_err(bad)?;
        }
        Ok(backup)
    }
    pub async fn cleanup(&self) -> cowork_domain::Result<u64> {
        let p = self.store.library_preferences(LibraryOwner::Local).await?;
        let count = if let Some(toolbox) = &self.toolbox {
            self.store
                .archive_retained_chats_guarded(
                    p.value.chat_retention_days,
                    cowork_storage::now(),
                    |id| toolbox.workspace_lease(id).ok(),
                )
                .await?
        } else {
            self.store
                .archive_retained_chats(p.value.chat_retention_days, cowork_storage::now())
                .await?
        };
        let mut o = self.observed.lock().map_err(|_| Error::Conflict)?;
        o.cleaned_chats = count;
        o.last_cleanup_at = Some(cowork_storage::now());
        Ok(count)
    }
    pub async fn tick(&self, startup: bool) -> cowork_domain::Result<()> {
        let p = self.store.library_preferences(LibraryOwner::Local).await?;
        let interval = i64::from(p.value.db_backup_interval_hours.max(1)) * 3_600_000;
        let now = cowork_storage::now();
        if p.value.auto_backup_db
            && self
                .manifests()?
                .last()
                .is_none_or(|m| now.saturating_sub(m.backup.created_at) >= interval)
        {
            self.backup(&format!("automatic:{}", now / interval))
                .await?;
        }
        if p.value.chat_retention_days > 0 && (!startup || p.value.db_cleanup_on_start) {
            self.cleanup().await?;
        }
        Ok(())
    }
    pub async fn run(
        self: Arc<Self>,
        stop: tokio_util::sync::CancellationToken,
        admission: Arc<tokio::sync::RwLock<()>>,
    ) {
        let mut startup = true;
        loop {
            let guard =
                tokio::select! {biased;_=stop.cancelled()=>break,guard=admission.read()=>guard};
            if stop.is_cancelled() {
                break;
            }
            if self.tick(startup).await.is_err() {
                if let Ok(mut o) = self.observed.lock() {
                    o.last_error=Some("Maintenance failed; existing backups and retained histories remain available".into());
                }
            } else if let Ok(mut o) = self.observed.lock() {
                o.last_error = None;
            }
            startup = false;
            drop(guard);
            tokio::select! {_=stop.cancelled()=>break,_=tokio::time::sleep(std::time::Duration::from_secs(60))=>{}}
        }
    }
    fn download(&self, id: &str) -> cowork_domain::Result<(DatabaseBackup, PathBuf)> {
        let backup = self
            .manifests()?
            .into_iter()
            .find(|m| m.backup.id == id)
            .ok_or(Error::NotFound)?
            .backup;
        let path = self.root.join(format!("{id}.backup"));
        if hash_file(&path)? != (backup.bytes, backup.sha256.clone()) {
            return Err(Error::Forbidden);
        }
        Ok((backup, path))
    }
}
pub fn router(service: Arc<MaintenanceService>) -> Router<AppState> {
    Router::new()
        .route("/maintenance", get(status))
        .route("/maintenance/backups", post(backup))
        .route("/maintenance/backups/{id}", get(download))
        .route("/maintenance/cleanup", post(cleanup))
        .layer(axum::Extension(service))
}
async fn status(
    axum::Extension(s): axum::Extension<Arc<MaintenanceService>>,
) -> Answer<MaintenanceStatus> {
    Ok(Json(s.status().await?))
}
async fn backup(
    axum::Extension(s): axum::Extension<Arc<MaintenanceService>>,
    body: Input<OwnerDatabaseBackupRequest>,
) -> Answer<DatabaseBackup> {
    Ok(Json(s.backup(&input(body)?.idempotency_key).await?))
}
async fn cleanup(
    axum::Extension(s): axum::Extension<Arc<MaintenanceService>>,
) -> Answer<serde_json::Value> {
    Ok(Json(
        serde_json::json!({"archived_chats":s.cleanup().await?}),
    ))
}
async fn download(
    axum::Extension(s): axum::Extension<Arc<MaintenanceService>>,
    Path(id): Path<String>,
) -> std::result::Result<Response, ApiFailure> {
    use tokio::io::AsyncReadExt;
    let (m, path) = s.download(&id)?;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| ApiFailure(Error::NotFound))?;
    let stream = futures_util::stream::try_unfold(file, |mut f| async move {
        let mut bytes = vec![0u8; 65536];
        let n = f.read(&mut bytes).await?;
        bytes.truncate(n);
        Ok::<_, std::io::Error>(if n == 0 { None } else { Some((bytes, f)) })
    });
    let mut response = Response::new(axum::body::Body::from_stream(stream));
    for (name, value) in [
        ("content-type", "application/octet-stream".into()),
        (
            "content-disposition",
            format!("attachment; filename=\"cowork-{}.{}\"", m.id, m.format),
        ),
        ("x-cowork-sha256", m.sha256),
        (
            "x-cowork-file-name",
            format!("cowork-{}.{}", m.id, m.format),
        ),
        ("content-length", m.bytes.to_string()),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().map_err(|_| ApiFailure(Error::Forbidden))?,
        );
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn backup_exact_acknowledgements_survive_more_than_two_thousand_retired_images() {
        let temp = tempfile::tempdir().unwrap();
        let store = cowork_storage::Store::sqlite(&temp.path().join("db"))
            .await
            .unwrap();
        let service = MaintenanceService::open(store, temp.path(), None).unwrap();
        let mut last = None;
        for index in 0..2055 {
            let backup = DatabaseBackup {
                id: id(),
                created_at: 1000 + index,
                bytes: 1,
                sha256: "a".repeat(64),
                format: "sqlite3".into(),
            };
            let manifest = Manifest {
                format: 1,
                request_key: format!("retired-{index}"),
                backup: backup.clone(),
            };
            fs::write(
                service.root.join(format!("{}.receipt", backup.id)),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            last = Some(backup);
        }
        assert_eq!(service.manifests_in(true).unwrap().len(), 2055);
        assert!(service.manifests().unwrap().is_empty());
        let acknowledged = service.backup("retired-2054").await.unwrap();
        assert_eq!(acknowledged.id, last.unwrap().id);
        assert!(service.manifests().unwrap().is_empty());
    }
    #[test]
    fn postgres_secrets_never_enter_dump_arguments() {
        let (url,password,ssl)=dump_connection("postgresql://owner:URLSECRET@localhost/db?password=QUERYSECRET&sslpassword=SSLSECRET&sslmode=require").unwrap();
        assert!(!url.contains("SECRET"));
        assert!(url.contains("sslmode=require"));
        assert_eq!(password.as_deref(), Some("QUERYSECRET"));
        assert_eq!(ssl.as_deref(), Some("SSLSECRET"));
    }
    #[tokio::test]
    async fn consistent_backups_replay_verified_download_and_owned_retention() {
        let temp = tempfile::tempdir().unwrap();
        let store = cowork_storage::Store::sqlite(&temp.path().join("live.sqlite3"))
            .await
            .unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Before snapshot".into(),
            })
            .await
            .unwrap();
        let service = MaintenanceService::open(store.clone(), temp.path(), None).unwrap();
        let first = service.backup("manual-original").await.unwrap();
        assert_eq!(
            service.backup("manual-original").await.unwrap().id,
            first.id
        );
        let (_, path) = service.download(&first.id).unwrap();
        let restored = cowork_storage::Store::sqlite(&temp.path().join("restored.sqlite3"))
            .await
            .unwrap();
        restored.close().await;
        fs::copy(path, temp.path().join("restored.sqlite3")).unwrap();
        let restored = cowork_storage::Store::sqlite(&temp.path().join("restored.sqlite3"))
            .await
            .unwrap();
        assert!(
            restored
                .projects()
                .await
                .unwrap()
                .iter()
                .any(|p| p.id == project.id)
        );
        restored.close().await;
        fs::write(service.root.join("unrelated.txt"), "keep").unwrap();
        for i in 0..8 {
            service.backup(&format!("next-{i}")).await.unwrap();
        }
        assert_eq!(
            service.backup("manual-original").await.unwrap().id,
            first.id
        );
        assert_eq!(service.status().await.unwrap().backups.len(), 7);
        assert_eq!(
            fs::read_to_string(service.root.join("unrelated.txt")).unwrap(),
            "keep"
        );
        let latest = service
            .status()
            .await
            .unwrap()
            .backups
            .last()
            .unwrap()
            .clone();
        fs::write(
            service.root.join(format!("{}.backup", latest.id)),
            "tampered",
        )
        .unwrap();
        assert!(matches!(
            service.download(&latest.id),
            Err(Error::Forbidden)
        ));
        store.close().await;
    }
}
