//! Operator-only object backend. Credentials never enter public contracts/errors.
use cowork_domain::{Error, Result, hash};
use cowork_storage::{TEAM_BLOB_MAX_BYTES, TeamBlobVersion};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use zeroize::Zeroizing;

#[path = "s3_project_versions.rs"]
mod project_versions;

#[cfg(windows)]
use crate::private_secret_file as held_private;

struct Remote {
    endpoint: reqwest::Url,
    bucket: String,
    region: String,
    access_key: Zeroizing<String>,
    secret_key: Zeroizing<String>,
    client: reqwest::Client,
}
struct Managed {
    root: PathBuf,
    _held: File,
}
enum Backend {
    Remote(Remote),
    Managed(Managed),
}
pub struct S3Store {
    backend: Backend,
    id: String,
}
fn denied() -> Error {
    Error::Validation("Object storage configuration or private namespace is invalid".into())
}
fn unavailable() -> Error {
    Error::Provider("object_storage_unavailable".into())
}
fn env(name: &str) -> Result<Option<String>> {
    std::env::var(name).map(Some).or_else(|e| match e {
        std::env::VarError::NotPresent => Ok(None),
        _ => Err(denied()),
    })
}
fn nonempty_env(name: &str) -> Result<Option<String>> {
    Ok(env(name)?.filter(|value| !value.is_empty()))
}
fn credential(name: &str) -> Result<Option<String>> {
    let direct = Zeroizing::new(env(name)?);
    let path = env(&format!("{name}_FILE"))?;
    match (direct.as_ref(), path) {
        (Some(_), Some(_)) => Err(denied()),
        (Some(value), None) => Ok(Some(value.clone())),
        (None, None) => Ok(None),
        (None, Some(path)) => {
            if path.is_empty() {
                return Err(denied());
            }
            // The opened handle is retained through the bounded read. Existing
            // permissions are validated; they are never repaired automatically.
            let file = open_private(Path::new(&path)).map_err(|_| denied())?;
            if file.metadata().map_err(|_| denied())?.len() > 256 {
                return Err(denied());
            }
            let mut value = Zeroizing::new(String::new());
            file.take(257)
                .read_to_string(&mut value)
                .map_err(|_| denied())?;
            if value.len() > 256 || value.trim().is_empty() {
                return Err(denied());
            }
            Ok(Some(value.trim().to_owned()))
        }
    }
}

impl S3Store {
    /// Call once before API/worker readiness, with a private directory below the
    /// daemon's held, validated data directory. No hot reload or browser config.
    pub fn from_environment(managed_root: PathBuf) -> Result<Arc<Self>> {
        let endpoint = nonempty_env("COWORK_S3_ENDPOINT")?;
        let allow_http = match env("COWORK_S3_ALLOW_INSECURE_HTTP")?.as_deref() {
            None | Some("0") => false,
            Some("1") => true,
            _ => return Err(denied()),
        };
        let values = Zeroizing::new(vec![
            nonempty_env("COWORK_S3_BUCKET")?,
            nonempty_env("COWORK_S3_REGION")?,
            credential("COWORK_S3_ACCESS_KEY_ID")?,
            credential("COWORK_S3_SECRET_ACCESS_KEY")?,
        ]);
        if let Some(endpoint) = endpoint {
            if values.iter().any(Option::is_none) {
                return Err(denied());
            }
            let parsed = reqwest::Url::parse(&endpoint).map_err(|_| denied())?;
            let local = matches!(
                parsed.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
            );
            if endpoint.len() > 2048
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || parsed.path() != "/"
                || parsed.host_str().is_none()
                || (parsed.scheme() != "https"
                    && !(parsed.scheme() == "http" && (local || allow_http)))
            {
                return Err(denied());
            }
            let bucket = values[0].as_ref().ok_or_else(denied)?.clone();
            let region = values[1].as_ref().ok_or_else(denied)?.clone();
            if !(3..=63).contains(&bucket.len())
                || !bucket.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.')
                })
                || bucket.starts_with(['-', '.'])
                || bucket.ends_with(['-', '.'])
                || bucket.contains("..")
                || region.is_empty()
                || region.len() > 80
                || !region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err(denied());
            }
            let access = values[2].as_ref().ok_or_else(denied)?;
            let secret = values[3].as_ref().ok_or_else(denied)?;
            if access.is_empty()
                || access.len() > 200
                || !access
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                || secret.len() < 8
                || secret.len() > 256
                || !secret.is_ascii()
                || secret.chars().any(char::is_control)
            {
                return Err(denied());
            }
            let backend_id =
                hash(format!("s3/v1:{}:{}:{region}", parsed.as_str(), bucket).as_bytes());
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(90))
                .build()
                .map_err(|_| denied())?;
            Ok(Arc::new(Self {
                id: format!("s3:{backend_id}"),
                backend: Backend::Remote(Remote {
                    endpoint: parsed,
                    bucket,
                    region,
                    access_key: Zeroizing::new(access.clone()),
                    secret_key: Zeroizing::new(secret.clone()),
                    client,
                }),
            }))
        } else {
            // A region default may be present in the operator's deployment
            // environment while the managed backend is selected.
            if [0, 2, 3].into_iter().any(|index| values[index].is_some()) {
                return Err(denied());
            }
            create_managed_directory(&managed_root)?;
            let root = managed_root.canonicalize().map_err(|_| denied())?;
            let held = open_directory(&root)?;
            let id = format!("managed:{}", hash(root.to_string_lossy().as_bytes()));
            Ok(Arc::new(Self {
                id,
                backend: Backend::Managed(Managed { root, _held: held }),
            }))
        }
    }
    pub fn backend_id(&self) -> &str {
        &self.id
    }
    fn validate_version(&self, v: &TeamBlobVersion) -> Result<()> {
        if v.backend_id != self.id
            || v.size > TEAM_BLOB_MAX_BYTES
            || v.sha256.len() != 64
            || !v.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || v.object_key.len() > 400
            || !v
                .object_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-'))
            || !v.object_key.starts_with("cowork/v1/")
            || v.object_key.contains("//")
        {
            return Err(denied());
        }
        Ok(())
    }
    pub async fn put(&self, v: &TeamBlobVersion, bytes: &[u8]) -> Result<()> {
        self.validate_version(v)?;
        if bytes.len() as u64 != v.size || hash(bytes) != v.sha256 {
            return Err(denied());
        }
        match &self.backend {
            Backend::Remote(remote) => {
                let response = remote.request("PUT", v, bytes.to_vec()).await?;
                if !response.status().is_success() {
                    return Err(unavailable());
                }
                Ok(())
            }
            Backend::Managed(managed) => {
                let mut file = create_private(&managed.root.join(hash(v.object_key.as_bytes())))?;
                file.write_all(bytes).map_err(|_| unavailable())?;
                file.sync_all().map_err(|_| unavailable())?;
                #[cfg(unix)]
                managed._held.sync_all().map_err(|_| unavailable())?;
                Ok(())
            }
        }
    }
    pub async fn get(&self, v: &TeamBlobVersion) -> Result<Vec<u8>> {
        self.validate_version(v)?;
        let bytes = match &self.backend {
            Backend::Remote(remote) => {
                let mut response = remote.request("GET", v, Vec::new()).await?;
                if !response.status().is_success()
                    || response
                        .content_length()
                        .is_some_and(|s| s > TEAM_BLOB_MAX_BYTES)
                {
                    return Err(unavailable());
                }
                let mut data = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
                    if data.len().saturating_add(chunk.len()) as u64 > TEAM_BLOB_MAX_BYTES {
                        return Err(unavailable());
                    }
                    data.extend_from_slice(&chunk);
                }
                data
            }
            Backend::Managed(managed) => {
                let file = open_private(&managed.root.join(hash(v.object_key.as_bytes())))?;
                if file.metadata().map_err(|_| unavailable())?.len() > TEAM_BLOB_MAX_BYTES {
                    return Err(unavailable());
                }
                let mut data = Vec::new();
                file.take(TEAM_BLOB_MAX_BYTES + 1)
                    .read_to_end(&mut data)
                    .map_err(|_| unavailable())?;
                data
            }
        };
        if bytes.len() as u64 != v.size || hash(&bytes) != v.sha256 {
            return Err(Error::Conflict);
        }
        Ok(bytes)
    }
    pub async fn delete(&self, v: &TeamBlobVersion) -> Result<()> {
        self.validate_version(v)?;
        match &self.backend {
            Backend::Remote(remote) => {
                let response = remote.request("DELETE", v, Vec::new()).await?;
                if !response.status().is_success()
                    && response.status() != reqwest::StatusCode::NOT_FOUND
                {
                    return Err(unavailable());
                }
                Ok(())
            }
            Backend::Managed(managed) => {
                let path = managed.root.join(hash(v.object_key.as_bytes()));
                match std::fs::symlink_metadata(&path) {
                    Ok(m) => {
                        ordinary(&m, false)?;
                        std::fs::remove_file(path).map_err(|_| unavailable())?;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(unavailable()),
                }
                #[cfg(unix)]
                managed._held.sync_all().map_err(|_| unavailable())?;
                Ok(())
            }
        }
    }
    pub async fn exists(&self, v: &TeamBlobVersion) -> Result<bool> {
        self.validate_version(v)?;
        match &self.backend {
            Backend::Remote(remote) => {
                let response = remote.request("HEAD", v, Vec::new()).await?;
                if response.status() == reqwest::StatusCode::NOT_FOUND {
                    return Ok(false);
                }
                if !response.status().is_success() {
                    return Err(unavailable());
                }
                Ok(true)
            }
            Backend::Managed(managed) => {
                match std::fs::symlink_metadata(managed.root.join(hash(v.object_key.as_bytes()))) {
                    Ok(m) => {
                        ordinary(&m, false)?;
                        Ok(true)
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                    Err(_) => Err(unavailable()),
                }
            }
        }
    }
}
fn hmac(key: &[u8], value: &[u8]) -> Result<Vec<u8>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| denied())?;
    mac.update(value);
    Ok(mac.finalize().into_bytes().to_vec())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
impl Remote {
    async fn request(
        &self,
        method: &str,
        v: &TeamBlobVersion,
        body: Vec<u8>,
    ) -> Result<reqwest::Response> {
        let mut url = self.endpoint.clone();
        url.set_path(&format!("/{}/{}", self.bucket, v.object_key));
        let host = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().ok_or_else(denied)?),
            None => url.host_str().ok_or_else(denied)?.into(),
        };
        let utc = chrono::Utc::now();
        let timestamp = utc.format("%Y%m%dT%H%M%SZ").to_string();
        let date = utc.format("%Y%m%d").to_string();
        let body_hash = hash(&body);
        let mut headers =
            format!("host:{host}\nx-amz-content-sha256:{body_hash}\nx-amz-date:{timestamp}\n");
        let signed = if method == "PUT" {
            headers.push_str(&format!("x-amz-meta-sha256:{}\n", v.sha256));
            "host;x-amz-content-sha256;x-amz-date;x-amz-meta-sha256"
        } else {
            "host;x-amz-content-sha256;x-amz-date"
        };
        let canonical = format!(
            "{method}\n{}\n\n{headers}\n{signed}\n{body_hash}",
            url.path()
        );
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
            hash(canonical.as_bytes())
        );
        let prefix = Zeroizing::new(format!("AWS4{}", self.secret_key.as_str()));
        let date_key = Zeroizing::new(hmac(prefix.as_bytes(), date.as_bytes())?);
        let region_key = Zeroizing::new(hmac(&date_key, self.region.as_bytes())?);
        let service_key = Zeroizing::new(hmac(&region_key, b"s3")?);
        let signing_key = Zeroizing::new(hmac(&service_key, b"aws4_request")?);
        let signature = hex(&hmac(&signing_key, to_sign.as_bytes())?);
        let authorization = Zeroizing::new(format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}",
            self.access_key.as_str()
        ));
        let mut request = self
            .client
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| denied())?,
                url,
            )
            .header("host", host)
            .header("x-amz-content-sha256", body_hash)
            .header("x-amz-date", timestamp)
            .header("authorization", authorization.as_str())
            .body(body);
        if method == "PUT" {
            request = request
                .header("x-amz-meta-sha256", &v.sha256)
                .header("content-type", "application/octet-stream");
        }
        request.send().await.map_err(|_| unavailable())
    }
}
fn ordinary(metadata: &std::fs::Metadata, directory: bool) -> Result<()> {
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(denied());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(denied());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || (!directory && metadata.nlink() != 1)
        {
            return Err(denied());
        }
    }
    Ok(())
}
fn create_managed_directory(path: &Path) -> Result<()> {
    if std::fs::symlink_metadata(path).is_err() {
        #[cfg(windows)]
        held_private::create_private_directory(path).map_err(|_| denied())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .map_err(|_| denied())?;
        }
    }
    ordinary(
        &std::fs::symlink_metadata(path).map_err(|_| denied())?,
        true,
    )
}
fn open_directory(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x02000000 | 0x00200000).share_mode(3);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
    }
    let file = options.open(path).map_err(|_| denied())?;
    ordinary(&file.metadata().map_err(|_| denied())?, true)?;
    #[cfg(windows)]
    held_private::validate_directory(&file).map_err(|_| denied())?;
    Ok(file)
}
fn create_private(path: &Path) -> Result<File> {
    #[cfg(windows)]
    {
        let file = held_private::create_private_file(path).map_err(|_| unavailable())?;
        held_private::validate(&file).map_err(|_| denied())?;
        Ok(file)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| unavailable())
    }
}
fn open_private(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000).share_mode(1);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|_| unavailable())?;
    ordinary(&file.metadata().map_err(|_| denied())?, false)?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
            || info.nNumberOfLinks != 1
        {
            return Err(denied());
        }
        held_private::validate(&file).map_err(|_| denied())?;
    }
    Ok(file)
}
