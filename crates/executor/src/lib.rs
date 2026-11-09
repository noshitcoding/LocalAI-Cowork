//! Lifecycle supervision for trusted executor adapters. This is NOT a security sandbox.
//!
//! The application does not expose this module as a tool or HTTP command. A production
//! adapter must establish its filesystem, credential, network and resource boundaries
//! before accepting untrusted programs. In particular Linux process groups are escapable;
//! use a qualified cgroup/container boundary for untrusted execution.
mod confined;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub mod windows_isolation;
#[cfg(any(windows, target_os = "linux"))]
pub use confined::supervise_confined;
pub use confined::{Confinement, confinement_supported};
#[cfg(target_os = "linux")]
pub use linux::CgroupPlacement;
#[cfg(target_os = "linux")]
use linux::Child;
#[cfg(windows)]
use windows::Child;

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid trusted executor configuration: {0}")]
    Invalid(&'static str),
    #[error("Executor supervision failed; do not replay the action automatically")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Private runner configuration, deliberately not part of the renderer/API contracts.
#[derive(Clone, Debug)]
pub struct TrustedLaunch {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub directory: PathBuf,
    /// Exactly these entries are provided. Parent credentials and PATH are not inherited.
    pub environment: BTreeMap<String, String>,
    pub timeout: Duration,
    pub capture_bytes_per_stream: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Termination {
    Exited,
    TimedOut,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Capture {
    /// Original bytes; decoding is a display concern. Never log this by default.
    pub bytes: Vec<u8>,
    pub total_bytes: u64,
    pub truncated: bool,
    pub invalid_utf8: bool,
}
impl Capture {
    fn append(&mut self, bytes: &[u8], bound: usize) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
        let keep = bytes.len().min(bound.saturating_sub(self.bytes.len()));
        self.bytes.extend_from_slice(&bytes[..keep]);
        self.truncated |= keep < bytes.len();
    }
    fn finish(&mut self) {
        // A truncated final code point is also explicitly marked as undecodable capture.
        self.invalid_utf8 = std::str::from_utf8(&self.bytes).is_err();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub termination: Termination,
    pub process_id: Option<u32>,
    pub exit_code: Option<i64>,
    pub signal: Option<i32>,
    pub stdout: Capture,
    pub stderr: Capture,
    pub duration_ms: u64,
    /// False when an inherited pipe did not reach EOF after bounded final draining.
    pub output_complete: bool,
}

#[derive(Clone, Copy)]
struct Exit {
    code: Option<i64>,
    signal: Option<i32>,
}

/// Check the process working-directory path budget without touching the filesystem.
/// Existence, authority and executable identity are still checked by the caller/launch.
/// Windows CreateProcessW needs room for its trailing separator and NUL even when
/// ordinary filesystem APIs accept longer paths. Count UTF-16 units, not UTF-8 bytes.
/// https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setcurrentdirectory
pub fn working_directory_path_fits(directory: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        directory.as_os_str().encode_wide().count() <= 258
    }
    #[cfg(not(windows))]
    {
        let _ = directory;
        true
    }
}

fn validate(launch: &TrustedLaunch) -> Result<()> {
    if !launch.program.is_absolute()
        || !launch.program.is_file()
        || !launch.directory.is_absolute()
        || !launch.directory.is_dir()
    {
        return Err(Error::Invalid(
            "program and working directory must exist and be absolute",
        ));
    }
    if !working_directory_path_fits(&launch.directory) {
        return Err(Error::Invalid(
            "Windows working directory is too long; use a shorter private data directory",
        ));
    }
    if launch
        .program
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("bat") || e.eq_ignore_ascii_case("cmd"))
    {
        return Err(Error::Invalid(
            "batch files require an explicit, separately authorized shell adapter",
        ));
    }
    if launch.timeout.is_zero()
        || launch.timeout > Duration::from_secs(86_400)
        || !(1..=4 * 1024 * 1024).contains(&launch.capture_bytes_per_stream)
    {
        return Err(Error::Invalid(
            "timeout or capture bound is outside the supported range",
        ));
    }
    if launch.arguments.len() > 1024
        || launch.arguments.iter().any(|s| s.contains('\0'))
        || launch.arguments.iter().map(String::len).sum::<usize>() > 32_000
    {
        return Err(Error::Invalid("invalid or oversized argument list"));
    }
    let mut names = std::collections::HashSet::new();
    if launch.environment.len() > 128
        || launch.environment.iter().any(|(k, v)| {
            k.is_empty()
                || k.contains(['=', '\0'])
                || v.contains('\0')
                || !names.insert(k.to_ascii_uppercase())
        })
        || launch
            .environment
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            > 32_000
    {
        return Err(Error::Invalid(
            "invalid, duplicate or oversized environment",
        ));
    }
    Ok(())
}

/// Executes once. The caller must durably journal admission before calling and settle
/// the result afterwards. Cancellation/timeout never means an effect did not happen.
/// An IO error is uncertain and must not be retried as an unobserved new execution.
#[cfg(any(windows, target_os = "linux"))]
pub fn supervise(launch: &TrustedLaunch, cancel: &AtomicBool) -> Result<Outcome> {
    supervise_with(launch, cancel, || Child::spawn(launch))
}

/// Observe only the original newly spawned child while the supervisor still
/// owns its platform handle and has not reaped it. A failing observer closes
/// the owned child through its existing Drop/termination fence. No PID lookup
/// may be used to adopt an already running program.
#[cfg(any(windows, target_os = "linux"))]
pub fn supervise_started(
    launch: &TrustedLaunch,
    cancel: &AtomicBool,
    on_started: impl FnOnce(u32) -> Result<()>,
) -> Result<Outcome> {
    supervise_with_started(launch, cancel, || Child::spawn(launch), on_started)
}
#[cfg(target_os = "linux")]
pub fn supervise_started_in_cgroup(
    launch: &TrustedLaunch,
    cancel: &AtomicBool,
    placement: &CgroupPlacement,
    on_started: impl FnOnce(u32) -> Result<()>,
) -> Result<Outcome> {
    supervise_with_started(
        launch,
        cancel,
        || Child::spawn_placed(launch, placement),
        on_started,
    )
}

#[cfg(any(windows, target_os = "linux"))]
fn supervise_with(
    launch: &TrustedLaunch,
    cancel: &AtomicBool,
    spawn: impl FnOnce() -> Result<Child>,
) -> Result<Outcome> {
    supervise_with_started(launch, cancel, spawn, |_| Ok(()))
}

#[cfg(any(windows, target_os = "linux"))]
fn supervise_with_started(
    launch: &TrustedLaunch,
    cancel: &AtomicBool,
    spawn: impl FnOnce() -> Result<Child>,
    on_started: impl FnOnce(u32) -> Result<()>,
) -> Result<Outcome> {
    validate(launch)?;
    let started = Instant::now();
    let mut result = Outcome {
        termination: Termination::Canceled,
        process_id: None,
        exit_code: None,
        signal: None,
        stdout: Capture::default(),
        stderr: Capture::default(),
        duration_ms: 0,
        output_complete: true,
    };
    if cancel.load(Ordering::Acquire) {
        return Ok(result);
    }
    let mut child = spawn()?;
    result.process_id = Some(child.id());
    on_started(child.id())?;
    let exit = loop {
        child.drain(
            &mut result.stdout,
            &mut result.stderr,
            launch.capture_bytes_per_stream,
        )?;
        if let Some(exit) = child.try_wait()? {
            result.termination = Termination::Exited;
            break exit;
        }
        if cancel.load(Ordering::Acquire) || started.elapsed() >= launch.timeout {
            result.termination = if cancel.load(Ordering::Acquire) {
                Termination::Canceled
            } else {
                Termination::TimedOut
            };
            child.terminate()?;
            break child.wait(Duration::from_secs(3))?;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // A finished root cannot leave a background adapter child owning output handles.
    child.terminate()?;
    #[cfg(windows)]
    child.wait(Duration::from_secs(3))?;
    let drain_deadline = Instant::now() + Duration::from_millis(500);
    while !child.drain(
        &mut result.stdout,
        &mut result.stderr,
        launch.capture_bytes_per_stream,
    )? && Instant::now() < drain_deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    result.output_complete = child.output_complete();
    result.exit_code = exit.code;
    result.signal = exit.signal;
    result.stdout.finish();
    result.stderr.finish();
    result.duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    Ok(result)
}
