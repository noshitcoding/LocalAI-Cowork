//! One live Toolbox owner per physical, alias-free configured root.
//! This lease prevents another live instance's startup cleanup. It is not a
//! proof that orphaned child processes have stopped after their owner crashes.
use super::*;

pub(super) fn acquire(root: &Path) -> Result<fs::File> {
    let path = root.join(".toolbox-owner.lock");
    namespace(&path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Allow another contender to open/read, but forbid replacing this held
        // original lock file. The native exclusive lock decides ownership.
        options.share_mode(3).custom_flags(0x0020_0000);
    }
    let file = options.open(&path)?;
    single_file(&file)?;
    file.try_lock().map_err(|error| match error {
        fs::TryLockError::WouldBlock => Error::RootInUse,
        fs::TryLockError::Error(error) => error.into(),
    })?;
    namespace(&path)?;
    Ok(file)
}
