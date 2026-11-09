//! Fixed-space private failure evidence. Never authority, a receipt or a reason
//! to retry an action. No request/credential bytes are retained in this slot.
use anyhow::{Result, ensure};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use std::{
    fs::File,
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, MetadataExt},
    },
    sync::Mutex,
};
const NAME: &str = "managed-failure.bin";
const BYTES: usize = 16 * 1024;
pub(super) struct Diagnostics {
    directory: Dir,
    file: Mutex<File>,
}
impl Diagnostics {
    pub(super) fn open(directory: &Dir) -> Result<Self> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .follow(FollowSymlinks::No)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
        let file = directory.open_with(NAME, &options)?.into_std();
        let m = file.metadata()?;
        ensure!(
            m.is_file()
                && m.uid() == 0
                && m.nlink() == 1
                && m.mode() & 0o777 == 0o600
                && (m.len() == 0 || m.len() == BYTES as u64),
            "Invalid private diagnostic slot"
        );
        let status = unsafe { libc::posix_fallocate(file.as_raw_fd(), 0, BYTES as i64) };
        ensure!(status == 0, "Diagnostic reservation unavailable");
        file.sync_all()?;
        directory.open(".")?.into_std().sync_all()?;
        Ok(Self {
            directory: directory.try_clone()?,
            file: Mutex::new(file),
        })
    }
    pub(super) fn record(
        &self,
        phase: &'static str,
        attempt: &str,
        error: &impl std::fmt::Debug,
        clock: serde_json::Value,
    ) -> Result<()> {
        let file = self
            .file
            .lock()
            .map_err(|_| anyhow::anyhow!("Diagnostic lock unavailable"))?;
        let m = file.metadata()?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let current = self
            .directory
            .open_with(NAME, &options)?
            .into_std()
            .metadata()?;
        ensure!(
            m.is_file()
                && m.uid() == 0
                && m.nlink() == 1
                && m.mode() & 0o777 == 0o600
                && m.len() == BYTES as u64
                && m.blocks() * 512 >= BYTES as u64
                && current.dev() == m.dev()
                && current.ino() == m.ino(),
            "Diagnostic identity changed"
        );
        let value = serde_json::json!({"format_version":1,"diagnostic_only":true,"phase":phase,
            "attempt_sha256":cowork_domain::hash(attempt.as_bytes()),"error":format!("{error:?}").chars().take(1024).collect::<String>(),"clock_rollback":clock});
        let data = serde_json::to_vec(&value)?;
        ensure!(data.len() <= BYTES - 36, "Diagnostic record limit");
        let mut bytes = vec![0; BYTES];
        bytes[..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
        bytes[4..36].copy_from_slice(ring::digest::digest(&ring::digest::SHA256, &data).as_ref());
        bytes[36..36 + data.len()].copy_from_slice(&data);
        file.write_all_at(&bytes, 0)?;
        file.sync_all()?;
        Ok(())
    }
}
