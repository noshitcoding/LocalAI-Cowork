//! Windows isolation primitives, not a qualified production runner or a public tool.
//! Each capsule has a fresh LPAC identity, disabled Win32k calls and a bounded Job Object.
//! The additional private desktop is not by itself an interactive-desktop access boundary.
//! Only capsule-owned directories receive ACLs; host/project folders are never modified.
//! Disk quotas, bundle qualification, durable run/runner fencing and output promotion
//! remain required before this adapter can satisfy an EnvironmentQualification.
//! The separately reviewed plain-AppContainer launch for Tectonic is `Capsule::run_granted`
//! (via `crate::confined`); see docs/WINDOWS-ISOLATION.md.
pub(crate) mod confined;
mod files;
mod identity;
mod orphan;
pub use orphan::{
    NativeObject, OrphanInspection, OrphanRetirement, OrphanSnapshot, ProfileEntry, retired_marker,
};

use super::*;
pub(crate) use identity::Identity;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path},
};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Job-wide committed memory, including all child processes; not working-set size.
    pub memory_bytes: usize,
    pub processes: u32,
    /// Upper CPU allocation in millicores, converted to a Windows hard-cap rate.
    pub cpu_millis: u32,
}
impl Limits {
    pub(crate) fn validate(&self) -> Result<()> {
        if !(32 * 1024 * 1024..=64usize * 1024 * 1024 * 1024).contains(&self.memory_bytes)
            || !(1..=64).contains(&self.processes)
            || !(100..=64_000).contains(&self.cpu_millis)
        {
            return Err(Error::Invalid("Windows process limits are out of range"));
        }
        Ok(())
    }
}

/// Restrictions beyond the package identity, private desktop and Job Object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Profile {
    /// LPAC (no ALL APPLICATION PACKAGES access) and Win32k calls disabled.
    Headless,
    /// Plain AppContainer with Win32k allowed, for the reviewed Tectonic launch only. It can
    /// read whatever ALL APPLICATION PACKAGES can read.
    AppContainer,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    profile: String,
}

pub struct Capsule {
    root: PathBuf,
    identity: Option<Identity>,
    _lock: File,
    limits: Limits,
    programs: BTreeMap<String, (String, u64)>,
    consumed: bool,
}
impl Capsule {
    /// `parent` is a private runner-controlled state directory, never a renderer path.
    /// Capsules are fresh. Recovery never reconstructs a launch-capable object.
    pub fn create(parent: &Path, limits: Limits) -> Result<Self> {
        limits.validate()?;
        let parent = fs::canonicalize(parent)?;
        if !parent.is_dir() {
            return Err(Error::Invalid("capsule parent is not a directory"));
        }
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let root = parent.join(format!("capsule-{suffix}"));
        files::create_control_directory(&root)?;
        let lock = lock(&root)?;
        let profile = format!("LocalAI.Cowork.V2.{suffix}");
        let journal = Journal {
            version: 1,
            profile: profile.clone(),
        };
        let mut marker = files::create_control_file(&root.join("identity.json"))?;
        marker.write_all(&serde_json::to_vec(&journal).map_err(io::Error::other)?)?;
        marker.sync_all()?;
        // The journal precedes the OS mutation. Existing profiles are never adopted.
        let identity = Identity::create(&profile)?;
        // No recovery deletion without this durable ownership receipt. A crash between
        // OS creation and this receipt remains unresolved, never guessed or adopted.
        write_marker(&root, "owned", profile.as_bytes())?;
        fs::create_dir(root.join("bundle"))?;
        fs::create_dir(root.join("work"))?;
        Ok(Self {
            root,
            identity: Some(identity),
            _lock: lock,
            limits,
            programs: BTreeMap::new(),
            consumed: false,
        })
    }
    pub fn directory(&self) -> &Path {
        &self.root
    }
    pub fn workspace(&self) -> PathBuf {
        self.root.join("work")
    }
    pub fn profile_name(&self) -> &str {
        self.identity.as_ref().expect("live capsule").name()
    }
    pub fn sid(&self) -> Result<String> {
        self.identity.as_ref().expect("live capsule").sid_string()
    }

    /// Stage approved executable bytes before admission. There is no host-path grant.
    pub fn stage_program(&mut self, name: &str, bytes: &[u8], expected_sha256: &str) -> Result<()> {
        if self.consumed
            || !valid_name(name)
            || bytes.len() > 128 * 1024 * 1024
            || !name.to_ascii_lowercase().ends_with(".exe")
            || digest(bytes) != expected_sha256
        {
            return Err(Error::Invalid("invalid or changed staged program"));
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.root.join("bundle").join(name))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        self.programs
            .insert(name.into(), (expected_sha256.into(), bytes.len() as u64));
        Ok(())
    }
    pub fn stage_input(&mut self, relative: &str, bytes: &[u8]) -> Result<()> {
        if self.consumed || bytes.len() > 64 * 1024 * 1024 {
            return Err(Error::Invalid("capsule staging is closed or too large"));
        }
        let relative = relative_path(relative)?;
        let path = self.workspace().join(relative);
        fs::create_dir_all(
            path.parent()
                .ok_or(Error::Invalid("input parent missing"))?,
        )?;
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
    /// Preserve empty input/runtime directories inside this fresh private capsule.
    pub fn stage_directory(&mut self, relative: &str, bundle: bool) -> Result<()> {
        if self.consumed {
            return Err(Error::Invalid("capsule staging is closed"));
        }
        let relative = relative_path(relative)?;
        let root = if bundle {
            self.root.join("bundle")
        } else {
            self.workspace()
        };
        fs::create_dir_all(root.join(relative))?;
        Ok(())
    }
    /// Approved runtime/DLL bytes receive only the capsule bundle's read/execute
    /// ACL. This does not grant any host path or add another launchable program.
    pub fn stage_bundle_file(
        &mut self,
        relative: &str,
        bytes: &[u8],
        expected_sha256: &str,
    ) -> Result<()> {
        if self.consumed || bytes.len() > 128 * 1024 * 1024 || digest(bytes) != expected_sha256 {
            return Err(Error::Invalid("invalid or changed bundle file"));
        }
        let relative = relative_path(relative)?;
        let path = self.root.join("bundle").join(relative);
        fs::create_dir_all(
            path.parent()
                .ok_or(Error::Invalid("bundle parent missing"))?,
        )?;
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
    /// One attempt per capsule. Errors, cancellation and nonzero exits never rearm it.
    pub fn run(
        &mut self,
        program: &str,
        arguments: Vec<String>,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<Outcome> {
        if self.consumed {
            return Err(Error::Invalid("capsule was already consumed"));
        }
        let expected = self
            .programs
            .get(program)
            .ok_or(Error::Invalid("unregistered capsule program"))?;
        let executable = self.root.join("bundle").join(program);
        // Hold a no-follow, read-share-only handle through the entire launch/run. The
        // loader reads the hashed image; replacement/write races cannot swap it out.
        use std::os::windows::fs::OpenOptionsExt;
        let program_guard = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .custom_flags(0x00200000)
            .open(&executable)?;
        ordinary_file(&program_guard, expected.1)?;
        if program_guard.metadata()?.len() != expected.1 {
            return Err(Error::Invalid("staged executable size changed"));
        }
        let mut bytes = Vec::new();
        (&program_guard)
            .take(expected.1 + 1)
            .read_to_end(&mut bytes)?;
        if digest(&bytes) != expected.0 {
            return Err(Error::Invalid("staged executable changed"));
        }
        drop(bytes);
        let identity = self
            .identity
            .as_ref()
            .ok_or(Error::Invalid("capsule identity is closed"))?;
        let sid = identity.sid_string()?;
        // The freshly created AppContainer profile must not provide an unbounded
        // writable directory outside the runner's quota volume. It belongs to this
        // journaled identity; no existing user/application folder is changed.
        files::owner_only(Path::new(&identity.profile_folder()?))?;
        files::grant_capsule(&self.root, &sid)?;
        let launch = TrustedLaunch {
            program: executable,
            arguments,
            directory: self.workspace(),
            // No credentials, PATH, user profile or inherited parent environment.
            environment: BTreeMap::from([
                ("SystemRoot".into(), files::system_root()?),
                (
                    "LOCALAPPDATA".into(),
                    self.workspace().to_string_lossy().into_owned(),
                ),
                (
                    "TEMP".into(),
                    self.workspace().to_string_lossy().into_owned(),
                ),
                (
                    "TMP".into(),
                    self.workspace().to_string_lossy().into_owned(),
                ),
            ]),
            timeout,
            capture_bytes_per_stream: 65_536,
        };
        self.consumed = true;
        crate::supervise_with(&launch, cancel, || {
            crate::windows::Child::spawn_isolated(&launch, identity, self.limits, Profile::Headless)
        })
    }
    /// One plain-AppContainer launch (`Profile::AppContainer`: zero capabilities, no LPAC,
    /// Win32k allowed) of a host program the caller made readable to ALL APPLICATION
    /// PACKAGES. This capsule's package SID gets read/execute on `read` and modify on `write`
    /// for this launch only; the bundle/work areas are unused. Grants are revoked afterwards
    /// (read trees with propagation, write trees on their root only) and die with the
    /// profile on `close`. Errors, cancellation and nonzero exits never rearm it.
    pub(crate) fn run_granted(
        &mut self,
        launch: &TrustedLaunch,
        read: &[PathBuf],
        write: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<Outcome> {
        if self.consumed {
            return Err(Error::Invalid("capsule was already consumed"));
        }
        if launch
            .environment
            .keys()
            .any(|k| k.eq_ignore_ascii_case("SystemRoot") || k.eq_ignore_ascii_case("LOCALAPPDATA"))
        {
            return Err(Error::Invalid(
                "SystemRoot and LOCALAPPDATA are set by the capsule",
            ));
        }
        let identity = self
            .identity
            .as_ref()
            .ok_or(Error::Invalid("capsule identity is closed"))?;
        let sid = identity.sid_string()?;
        // Unlike `run`, the package keeps its own fresh storage: Tectonic resolves its known
        // folders there and fails without access. The profile and folder die on `close`.
        let folder = PathBuf::from(identity.profile_folder()?);
        // Shell expands USERPROFILE and adds Packages/<name>/AC for LocalAppData.
        // Both known folders must exist before the child first resolves them.
        // All storage stays inside this fresh identity and is removed on close.
        let profile = folder.join("Profile");
        let local = profile.join("AppData").join("Local");
        let roaming = profile.join("AppData").join("Roaming");
        fs::create_dir_all(local.join("Packages").join(identity.name()).join("AC"))?;
        fs::create_dir_all(&roaming)?;
        let mut launch = launch.clone();
        launch.environment.retain(|key, _| {
            !key.eq_ignore_ascii_case("USERPROFILE") && !key.eq_ignore_ascii_case("APPDATA")
        });
        launch
            .environment
            .insert("USERPROFILE".into(), profile.display().to_string());
        launch
            .environment
            .insert("APPDATA".into(), roaming.display().to_string());
        launch
            .environment
            .insert("SystemRoot".into(), files::system_root()?);
        launch
            .environment
            .insert("LOCALAPPDATA".into(), local.display().to_string());
        self.consumed = true;
        let mut granted: Vec<(&PathBuf, bool)> = Vec::new();
        let mut outcome = (|| {
            // A private stage can have a non-inheritable host DACL. Without an
            // explicit host ACE on its children, files created by the lowbox can
            // become unreadable to the host after the package identity is retired.
            // Set this on caller-staged trees before any untrusted writer runs;
            // never repair permissions by walking the returned output afterwards.
            let owner = files::owner_sid()?;
            for path in write {
                files::grant(path, &owner, files::MODIFY)?;
            }
            for (paths, mask, writable) in [
                (read, files::READ_EXECUTE, false),
                (write, files::MODIFY, true),
            ] {
                for path in paths {
                    granted.push((path, writable));
                    files::grant(path, &sid, mask)?;
                }
            }
            crate::supervise_with(&launch, cancel, || {
                crate::windows::Child::spawn_isolated(
                    &launch,
                    identity,
                    self.limits,
                    Profile::AppContainer,
                )
            })
        })();
        // supervise_with returns only after the job's processes are gone (or with an error
        // after its Child closed the kill-on-close job), so the trees are static now.
        for (path, writable) in granted.into_iter().rev() {
            let revoked = if writable {
                files::revoke_root(path, &sid)
            } else {
                files::revoke_tree(path, &sid)
            };
            if let Err(error) = revoked {
                outcome = outcome.and(Err(error));
            }
        }
        outcome
    }
    /// Explicit cleanup reports failure; retained workspace bytes are not deleted.
    pub fn close(mut self) -> Result<PathBuf> {
        self.cleanup()?;
        Ok(self.root.clone())
    }
    fn cleanup(&mut self) -> Result<()> {
        if let Some(identity) = self.identity.as_mut() {
            identity.close()?;
        }
        self.identity = None;
        mark_closed(&self.root)
    }
}
impl Drop for Capsule {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Recover only this explicitly selected, journaled capsule. Never restart its process.
/// The file lock and named Job prevent cleanup of a still-running owner/child tree.
pub fn recover(root: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(root)?;
    use std::os::windows::fs::MetadataExt;
    if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
        return Err(Error::Invalid("capsule root must be an ordinary directory"));
    }
    let root = fs::canonicalize(root)?;
    let _lock = lock(&root)?;
    let bytes = read_marker(&root, "identity.json", 1024)?;
    let journal: Journal = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let suffix = journal
        .profile
        .strip_prefix("LocalAI.Cowork.V2.")
        .ok_or(Error::Invalid("foreign capsule profile"))?;
    if journal.version != 1
        || suffix.len() != 32
        || !suffix
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        || root.file_name().and_then(|n| n.to_str()) != Some(format!("capsule-{suffix}").as_str())
    {
        return Err(Error::Invalid(
            "capsule identity does not match its journal",
        ));
    }
    match read_marker(&root, "owned", 128) {
        Ok(bytes) if bytes == journal.profile.as_bytes() => {}
        Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
            return orphan::recover_confirmed(&root, &journal.profile);
        }
        _ => return Err(Error::Invalid("capsule profile ownership is unconfirmed")),
    }
    match read_marker(&root, "closed", 128) {
        Ok(bytes) if bytes == CLOSED => return Ok(()),
        Ok(_) => return Err(Error::Invalid("invalid capsule cleanup marker")),
        Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    identity::recover_profile(&journal.profile)?;
    mark_closed(&root)
}
fn lock(root: &Path) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    let path = root.join("session.lock");
    let file = match files::create_control_file(&path) {
        Ok(file) => file,
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(0x00200000)
            .open(path)?,
        Err(error) => return Err(error),
    };
    ordinary_file(&file, 0)?;
    file.try_lock()
        .map_err(|e| io::Error::other(format!("capsule lock unavailable: {e}")))?;
    Ok(file)
}
fn mark_closed(root: &Path) -> Result<()> {
    match write_marker(root, "closed", CLOSED) {
        Ok(()) => {}
        Err(Error::Io(e)) if e.kind() == io::ErrorKind::AlreadyExists => {
            if read_marker(root, "closed", 128)? != CLOSED {
                return Err(Error::Invalid("invalid capsule cleanup marker"));
            }
        }
        Err(e) => return Err(e),
    }
    Ok(())
}
const CLOSED: &[u8] = b"profile and job closed; process never replayed\n";
fn ordinary_file(file: &File, max_bytes: u64) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 || metadata.len() > max_bytes
    {
        return Err(Error::Invalid(
            "capsule control file is invalid or too large",
        ));
    }
    Ok(())
}
fn read_marker(root: &Path, name: &str, bound: u64) -> Result<Vec<u8>> {
    use std::os::windows::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(0x00200000)
        .open(root.join(name))?;
    ordinary_file(&file, bound)?;
    let mut bytes = Vec::new();
    file.take(bound + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > bound {
        return Err(Error::Invalid("capsule control file grew beyond its bound"));
    }
    Ok(bytes)
}
fn write_marker(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = files::create_control_file(&root.join(name))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        && name != "."
        && name != ".."
        && !name.ends_with('.')
        && ![
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ]
        .contains(
            &name
                .split('.')
                .next()
                .unwrap_or("")
                .to_ascii_uppercase()
                .as_str(),
        )
}
fn relative_path(input: &str) -> Result<&Path> {
    let path = Path::new(input);
    if input.is_empty()
        || input.len() > 200
        || input.contains(['\\', ':', '\0'])
        || input.chars().any(char::is_control)
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || input.split('/').any(|c| !valid_name(c))
    {
        return Err(Error::Invalid("invalid capsule-relative input path"));
    }
    Ok(path)
}
