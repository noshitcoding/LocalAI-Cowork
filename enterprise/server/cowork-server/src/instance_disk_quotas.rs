//! Runtime half of the original management placement. A short-lived current
//! manager decision and an actual statvfs readback gate writes; the fully
//! allocated filesystem is the final concurrent/background-write ceiling.
use crate::error::ApiError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{future::Future, path::PathBuf, sync::OnceLock, time::Duration};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Filesystem {
    path: PathBuf,
    filesystem_uuid: Uuid,
    device: u64,
    fsid: u64,
    maximum_bytes: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    version: u8,
    organisation_id: Uuid,
    instance_id: Uuid,
    profile_revision: i64,
    specification_sha256: String,
    filesystem: Filesystem,
}
static CONFIG: OnceLock<Option<Configuration>> = OnceLock::new();
fn unavailable() -> ApiError {
    ApiError::Unavailable(
        "current total physical disk authority is unavailable or exhausted".into(),
    )
}
pub fn initialize() -> anyhow::Result<()> {
    let configured = match std::env::var("COWORK_INSTANCE_DISK_POLICY") {
        Ok(s) => Some(serde_json::from_str::<Configuration>(&s)?),
        Err(std::env::VarError::NotPresent) => None,
        Err(e) => return Err(e.into()),
    };
    if let Some(c) = &configured {
        anyhow::ensure!(
            c.version == 1
                && !c.instance_id.is_nil()
                && !c.organisation_id.is_nil()
                && c.profile_revision > 0
                && c.specification_sha256.len() == 64
                && c.specification_sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                && c.filesystem.maximum_bytes >= 1073741824
                && !c.filesystem.filesystem_uuid.is_nil(),
            "invalid original disk runtime configuration"
        );
        anyhow::ensure!(
            std::env::var("COWORK_INSTANCE_ID")?.parse::<Uuid>()? == c.instance_id
                && std::env::var("COWORK_ORGANISATION_ID")?.parse::<Uuid>()? == c.organisation_id
                && crate::fleet::enabled(),
            "disk runtime belongs to another instance"
        );
        local(c, 0).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    CONFIG
        .set(configured)
        .map_err(|_| anyhow::anyhow!("disk runtime already initialized"))
}
pub fn contract() -> Option<u8> {
    CONFIG.get().and_then(Option::as_ref).map(|_| 1)
}
#[cfg(target_os = "linux")]
fn local(c: &Configuration, bytes: u64) -> Result<(), ApiError> {
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
    let p = &c.filesystem.path;
    if !p.is_absolute()
        || p.ancestors()
            .any(|a| std::fs::symlink_metadata(a).is_ok_and(|m| m.file_type().is_symlink()))
    {
        return Err(unavailable());
    }
    let metadata = std::fs::metadata(p).map_err(|_| unavailable())?;
    if !metadata.is_dir() || metadata.dev() != c.filesystem.device {
        return Err(unavailable());
    }
    let path = std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| unavailable())?;
    let mut fs = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // CString remains alive and statvfs writes the complete platform struct only
    // on success. No borrowed pointer or partially initialized field escapes.
    if unsafe { libc::statvfs(path.as_ptr(), fs.as_mut_ptr()) } != 0 {
        return Err(unavailable());
    }
    let fs = unsafe { fs.assume_init() };
    let capacity = (fs.f_blocks as u64)
        .checked_mul(fs.f_frsize as u64)
        .ok_or_else(unavailable)?;
    let free = (fs.f_bavail as u64)
        .checked_mul(fs.f_frsize as u64)
        .ok_or_else(unavailable)?;
    if fs.f_fsid as u64 != c.filesystem.fsid
        || capacity == 0
        || capacity > c.filesystem.maximum_bytes
        || free < bytes.saturating_add(1048576)
    {
        return Err(unavailable());
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn local(_: &Configuration, _: u64) -> Result<(), ApiError> {
    Err(unavailable())
}

pub async fn write_gate(purpose: &str, bytes: u64) -> Result<(), ApiError> {
    let Some(c) = CONFIG.get().and_then(Option::as_ref) else {
        return Ok(());
    };
    let epoch: Uuid = std::env::var("COWORK_EXECUTION_EPOCH")
        .map_err(|_| unavailable())?
        .parse()
        .map_err(|_| unavailable())?;
    let base = std::env::var("COWORK_FLEET_MANAGEMENT_URL").map_err(|_| unavailable())?;
    let client = crate::management_transport::client(&base, Duration::from_secs(5))
        .map_err(|_| unavailable())?;
    let id = Uuid::new_v4();
    let request = json!({"organisation_id":c.organisation_id,"instance_id":c.instance_id,"execution_epoch":epoch,"profile_revision":c.profile_revision,
  "specification_sha256":c.specification_sha256,"request_id":id,"purpose":purpose,"bytes":bytes});
    let mut response = client
        .post(format!(
            "{}/management/runner/v1/disk-quota/io-authority",
            base.trim_end_matches('/')
        ))
        .header(
            "x-cowork-instance-key",
            crate::fleet::secret("COWORK_FLEET_KEY_FILE")?,
        )
        .json(&request)
        .send()
        .await
        .map_err(|_| unavailable())?;
    if !response.status().is_success() {
        return Err(unavailable());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if body.len().saturating_add(chunk.len()) > 16384 {
            return Err(unavailable());
        }
        body.extend_from_slice(&chunk);
    }
    let r: Value = serde_json::from_slice(&body).map_err(|_| unavailable())?;
    for key in [
        "organisation_id",
        "instance_id",
        "execution_epoch",
        "profile_revision",
        "specification_sha256",
        "request_id",
        "purpose",
        "bytes",
    ] {
        if r[key] != request[key] {
            return Err(unavailable());
        }
    }
    let until: DateTime<Utc> =
        serde_json::from_value(r["expires_at"].clone()).map_err(|_| unavailable())?;
    if r["format"] != "cowork-disk-io-authority-v1" || until <= Utc::now() {
        return Err(unavailable());
    }
    local(c, bytes)?;
    // No long cached permit and no replay: one response gates the next IO only.
    Ok(())
}
pub async fn guard<T>(work: impl Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    if contract().is_none() {
        return work.await;
    }
    tokio::pin!(work);
    let mut timer = tokio::time::interval(Duration::from_secs(2));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {biased;_=timer.tick()=>write_gate("workspace",0).await.map_err(|e|anyhow::anyhow!("{e}"))?,result=&mut work=>return result}
    }
}
pub async fn request(method: &axum::http::Method, path: &str) -> Result<(), ApiError> {
    if matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS")
        || crate::organisation_mfa::recovery_route(method, path)
        || method == axum::http::Method::DELETE
        || path.ends_with("/cancel")
    {
        return Ok(());
    }
    write_gate("database", 0).await
}
