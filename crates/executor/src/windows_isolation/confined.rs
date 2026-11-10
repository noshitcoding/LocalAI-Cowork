//! Windows side of `crate::confined`: one fresh plain-AppContainer capsule per run.
//! Shared read trees (program directory, caches) carry a kept read/execute grant for
//! ALL APPLICATION PACKAGES, like Program Files; per-run trees are granted to the run's
//! own package SID only. Journals live in `Confinement::state`, one capsule each.
use super::*;
use std::{collections::HashSet, sync::Mutex};

/// State directories already scanned for crashed runs by this process. Capsule creation
/// also happens under this lock, so a scan never meets a half-created capsule.
static SCANNED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
/// Shared trees fully (re)granted by this process. Serializes all shared DACL edits.
static SHARED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

pub(crate) fn supervise(
    launch: &TrustedLaunch,
    confinement: &crate::Confinement,
    cancel: &AtomicBool,
) -> Result<Outcome> {
    let limits = Limits {
        memory_bytes: confinement.memory_bytes,
        processes: confinement.processes,
        cpu_millis: confinement.cpu_millis,
    };
    limits.validate()?;
    share(&confinement.shared_read)?;
    let mut capsule = {
        let mut scanned = SCANNED
            .lock()
            .map_err(|_| Error::Invalid("confinement state lock is poisoned"))?;
        let state = fs::canonicalize(&confinement.state)?;
        if scanned
            .get_or_insert_with(HashSet::new)
            .insert(state.clone())
        {
            files::owner_only(&state)?;
            recover_state(&state);
        }
        Capsule::create(&state, limits)?
    };
    let root = capsule.directory().to_path_buf();
    let outcome = capsule.run_granted(launch, &confinement.read, &confinement.write, cancel);
    // Profile, desktop and job are closed even when the run failed; the journal stays when
    // that cleanup is unconfirmed so a later scan can retry it.
    let closed = capsule.close();
    if closed.is_ok() {
        let _ = fs::remove_dir_all(&root);
    }
    let outcome = outcome?;
    closed?;
    Ok(outcome)
}

/// Cleanup-only recovery of capsules left by a crashed owner. A live run holds its capsule
/// lock, and capsules without a confirmed ownership receipt stay untouched (see `recover`).
pub(crate) fn recover_state(state: &Path) {
    let Ok(entries) = fs::read_dir(state) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("capsule-") {
            continue;
        }
        let path = entry.path();
        if recover(&path).is_ok() {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn share(paths: &[PathBuf]) -> Result<()> {
    let mut done = SHARED
        .lock()
        .map_err(|_| Error::Invalid("shared grant lock is poisoned"))?;
    let done = done.get_or_insert_with(HashSet::new);
    for path in paths {
        let key = fs::canonicalize(path)?;
        // One full grant per process repairs a propagation a crash interrupted; later runs
        // only confirm the root ACE, which a recreated tree lacks.
        if !done.contains(&key) || !files::granted(path, files::ALL_PACKAGES, files::READ_EXECUTE)?
        {
            files::grant(path, files::ALL_PACKAGES, files::READ_EXECUTE)?;
            done.insert(key);
        }
    }
    Ok(())
}
