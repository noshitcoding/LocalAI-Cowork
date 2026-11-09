#[cfg(unix)]
use anyhow::Context;
use anyhow::{Result, ensure};
use cowork_runner_protocol::{Binding, SigningKey};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::fs::File;
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub format: u32,
    pub listen: SocketAddr,
    pub binding: Binding,
    pub data_directory: PathBuf,
    pub workspace_state_directory: PathBuf,
    pub token_file: PathBuf,
    pub runner_key_file: PathBuf,
    pub tls_certificate_file: PathBuf,
    pub tls_key_file: PathBuf,
    pub probe_executable: PathBuf,
    pub docker_executable: Option<PathBuf>,
    #[serde(default)]
    pub local_container_profile: bool,
    #[serde(default)]
    pub local_engine_id: Option<String>,
    #[serde(default)]
    pub agent_sandbox: Option<cowork_agent_sandbox::Configuration>,
    #[serde(default)]
    pub windows_bundles: BTreeMap<String, PathBuf>,
    #[serde(default = "retention_default")]
    pub retention_seconds: u64,
    #[serde(default = "attempt_limit_default")]
    pub max_retained_attempts: usize,
}
fn retention_default() -> u64 {
    7 * 24 * 60 * 60
}
fn attempt_limit_default() -> usize {
    256
}
/// Flush directory metadata before a completion record. Windows requires an
/// administrative volume handle; failure refuses a durable-success claim.
pub fn sync_directory(path: &Path) -> Result<()> {
    ordinary(path, true)?;
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(windows)]
    {
        use std::os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        };
        use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut mount = vec![0u16; 32768];
        let mut volume = vec![0u16; 32768];
        ensure!(
            unsafe { GetVolumePathNameW(wide.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) }
                != 0,
            "Workspace volume path unavailable"
        );
        ensure!(
            unsafe {
                GetVolumeNameForVolumeMountPointW(
                    mount.as_ptr(),
                    volume.as_mut_ptr(),
                    volume.len() as u32,
                )
            } != 0,
            "Workspace volume identity unavailable"
        );
        let end = volume
            .iter()
            .position(|v| *v == 0)
            .ok_or_else(|| anyhow::anyhow!("Invalid volume name"))?;
        ensure!(
            end > 0 && volume[end - 1] == b'\\' as u16,
            "Invalid volume name"
        );
        volume[end - 1] = 0;
        let handle = unsafe {
            CreateFileW(
                volume.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        ensure!(
            handle != INVALID_HANDLE_VALUE,
            "Durable volume flush requires operator volume access: {}",
            std::io::Error::last_os_error()
        );
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        ensure!(
            unsafe { FlushFileBuffers(handle.as_raw_handle()) } != 0,
            "Volume durability remains unconfirmed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}
pub fn ordinary(path: &Path, directory: bool) -> Result<()> {
    ensure!(path.is_absolute(), "Absolute operator path required");
    for parent in path.ancestors() {
        let m = std::fs::symlink_metadata(parent)?;
        ensure!(!m.file_type().is_symlink(), "Link in operator path");
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                m.file_attributes() & 0x400 == 0,
                "Reparse point in operator path"
            );
        }
    }
    let m = std::fs::symlink_metadata(path)?;
    ensure!(
        if directory { m.is_dir() } else { m.is_file() },
        "Wrong operator path type"
    );
    Ok(())
}
pub fn private_read(path: &Path, max: u64) -> Result<Vec<u8>> {
    ordinary(path, false)?;
    let mut o = OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(1).custom_flags(0x00200000);
    }
    let mut f = o.open(path)?;
    let m = f.metadata()?;
    ensure!(m.len() > 0 && m.len() <= max, "Private record is oversized");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0 && m.nlink() == 1,
            "Private authority is not owner-only"
        );
    }
    #[cfg(windows)]
    crate::private_secret_file::validate(&f)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut f).take(max + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 == m.len(), "Private record changed");
    Ok(bytes)
}
pub fn private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        if !path.exists() {
            std::fs::DirBuilder::new().mode(0o700).create(path)?;
        }
        ordinary(path, true)?;
        let m = std::fs::metadata(path)?;
        ensure!(
            m.permissions().mode() & 0o077 == 0 && m.uid() == unsafe { libc::geteuid() },
            "Data directory must be private and operator-owned"
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        match std::fs::symlink_metadata(path) {
            Ok(_) => {
                ordinary(path, true)?;
                let directory = OpenOptions::new()
                    .access_mode(0x0002_0081)
                    .share_mode(1 | 2)
                    .custom_flags(0x0200_0000 | 0x0020_0000)
                    .open(path)?;
                let metadata = directory.metadata()?;
                ensure!(
                    metadata.is_dir() && metadata.file_attributes() & 0x400 == 0,
                    "Original private data directory is a reparse object or changed type"
                );
                crate::private_secret_file::validate_directory(&directory)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                crate::private_secret_file::create_private_directory(path)?
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    let mut f = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?
    };
    #[cfg(windows)]
    let mut f = crate::private_secret_file::create_private_file(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}
pub fn durable_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    write_new(&temporary, &serde_json::to_vec(value)?)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
        }
        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        ensure!(
            unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) } != 0,
            "Atomic durable replacement failed: {}",
            std::io::Error::last_os_error()
        );
    }
    #[cfg(unix)]
    std::fs::rename(&temporary, path)?;
    #[cfg(unix)]
    File::open(path.parent().context("Missing journal parent")?)?.sync_all()?;
    Ok(())
}
pub fn load(path: &Path) -> Result<(Configuration, SigningKey, String)> {
    let c: Configuration = serde_json::from_slice(&private_read(path, 65536)?)?;
    ensure!(c.format == 1, "Unknown configuration format");
    if let Some(agent) = &c.agent_sandbox {
        agent.validate()?;
        ensure!(
            agent.worker_label_value == cowork_domain::canonical_hash(&c.binding)[..32],
            "Kubernetes worker binding label changed"
        );
    }
    ensure!(
        (3600..=90 * 24 * 60 * 60).contains(&c.retention_seconds)
            && (8..=10000).contains(&c.max_retained_attempts),
        "Invalid retention bounds"
    );
    ordinary(&c.workspace_state_directory, true)?;
    ordinary(&c.probe_executable, false)?;
    private_directory(&c.data_directory)?;
    let key = SigningKey::from_pkcs8(&private_read(&c.runner_key_file, 16384)?)?;
    ensure!(
        key.public_key() == c.binding.runner_public_key,
        "Runner signing identity changed"
    );
    let token = String::from_utf8(private_read(&c.token_file, 16384)?)?
        .trim()
        .to_owned();
    ensure!(
        token.len() >= 48 && !token.chars().any(char::is_whitespace),
        "Invalid transport authority"
    );
    Ok((c, key, token))
}
