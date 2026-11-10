//! OS-enforced confinement for one reviewed headless program (currently Tectonic).
//! Unlike `supervise`, the child cannot open host files outside the listed trees, except
//! what the platform leaves readable to every sandboxed process (Windows: ALL APPLICATION
//! PACKAGES; see docs/WINDOWS-ISOLATION.md). It has no network access and cannot start
//! processes that outlive it. Setup failure is an error; there is no unconfined fallback.
use super::*;
use std::path::Path;

/// Filesystem view and limits of one confined run.
#[derive(Clone, Debug)]
pub struct Confinement {
    /// Private host directory for run journals (Windows crash recovery). Never granted.
    pub state: PathBuf,
    /// Public, read-only trees shared by all runs, e.g. the program directory and caches.
    /// Windows keeps a read/execute grant for ALL APPLICATION PACKAGES on them, so they must
    /// be owned by this application and hold public data only.
    pub shared_read: Vec<PathBuf>,
    /// This run's read-only trees.
    pub read: Vec<PathBuf>,
    /// This run's application-staged writable trees. On Windows their children
    /// inherit host-owner access before launch; the package grant is per run.
    pub write: Vec<PathBuf>,
    /// Committed memory (Windows Job) or address space (Linux RLIMIT_AS).
    pub memory_bytes: usize,
    /// Windows Job process limit; Linux denies any new process outright.
    pub processes: u32,
    /// Windows Job CPU hard cap in millicores.
    pub cpu_millis: u32,
}

impl Confinement {
    fn validate(&self, launch: &TrustedLaunch) -> Result<()> {
        let canonical = |path: &PathBuf| -> Result<PathBuf> {
            if !path.is_absolute() {
                return Err(Error::Invalid("confined paths must be absolute"));
            }
            Ok(std::fs::canonicalize(path)?)
        };
        let shared = self
            .shared_read
            .iter()
            .map(canonical)
            .collect::<Result<Vec<_>>>()?;
        let read = self
            .read
            .iter()
            .map(canonical)
            .collect::<Result<Vec<_>>>()?;
        let write = self
            .write
            .iter()
            .map(canonical)
            .collect::<Result<Vec<_>>>()?;
        let state = canonical(&self.state)?;
        if write.is_empty() || write.iter().any(|p| !p.is_dir()) || !state.is_dir() {
            return Err(Error::Invalid(
                "confined write trees and state must be directories",
            ));
        }
        #[cfg(windows)]
        if shared.iter().chain(&read).any(|p| !p.is_dir()) {
            return Err(Error::Invalid("Windows grants apply to directories only"));
        }
        let beneath = |path: &Path, roots: &[&Vec<PathBuf>]| {
            roots
                .iter()
                .any(|set| set.iter().any(|root| path.starts_with(root)))
        };
        let granted = [&shared, &read, &write];
        if beneath(&state, &granted)
            || granted
                .iter()
                .flat_map(|set| set.iter())
                .any(|root| root.starts_with(&state))
        {
            return Err(Error::Invalid(
                "confinement state must stay outside granted trees",
            ));
        }
        if !beneath(&canonical(&launch.program)?, &[&shared, &read]) {
            return Err(Error::Invalid(
                "confined program must be inside a read tree",
            ));
        }
        if !beneath(&canonical(&launch.directory)?, &granted) {
            return Err(Error::Invalid("confined working directory must be granted"));
        }
        Ok(())
    }
}

/// Whether this OS build offers the confinement at all. A real launch still verifies it and
/// fails closed.
pub fn confinement_supported() -> bool {
    #[cfg(windows)]
    return true;
    #[cfg(target_os = "linux")]
    return linux::sandbox_supported();
    #[allow(unreachable_code)]
    false
}

/// Runs `launch` once inside the confinement. The caller must not set SystemRoot or
/// LOCALAPPDATA on Windows; the capsule supplies them.
#[cfg(any(windows, target_os = "linux"))]
pub fn supervise_confined(
    launch: &TrustedLaunch,
    confinement: &Confinement,
    cancel: &AtomicBool,
) -> Result<Outcome> {
    validate(launch)?;
    confinement.validate(launch)?;
    #[cfg(windows)]
    return windows_isolation::confined::supervise(launch, confinement, cancel);
    #[cfg(target_os = "linux")]
    {
        let read: Vec<PathBuf> = confinement
            .shared_read
            .iter()
            .chain(&confinement.read)
            .cloned()
            .collect();
        let sandbox = linux::Sandbox::prepare(
            &launch.program,
            &read,
            &confinement.write,
            confinement.memory_bytes,
        )?;
        supervise_with(launch, cancel, || Child::spawn_confined(launch, &sandbox))
    }
}
