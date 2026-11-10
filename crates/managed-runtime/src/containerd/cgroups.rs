//! Owned cgroup-v2 controls. Only fresh children are created; no host controller
//! settings, foreign processes, discovery adoption or implicit retirement.
mod fence;
use super::*;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
pub use fence::{BoundaryObservation, TaskBoundary};
use std::{io::Write, os::fd::AsRawFd};

#[derive(Clone, Copy)]
struct NativeId {
    dev: u64,
    ino: u64,
}
impl NativeId {
    fn capture(dir: &Dir) -> Result<Self> {
        let file = dir.open(".")?.into_std();
        let m = file.metadata()?;
        let mut fs = std::mem::MaybeUninit::<libc::statfs>::zeroed();
        if unsafe { libc::fstatfs(file.as_raw_fd(), fs.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if unsafe { fs.assume_init() }.f_type != 0x63677270
            || !m.is_dir()
            || m.uid() != 0
            || m.mode() & 0o022 != 0
        {
            return Err(Error::Cgroup);
        }
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
        })
    }
    fn require(self, dir: &Dir) -> Result<()> {
        let now = Self::capture(dir)?;
        if self.dev != now.dev || self.ino != now.ino {
            return Err(Error::Cgroup);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HardLimits {
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub processes: u32,
}
impl HardLimits {
    pub fn validate(self) -> Result<()> {
        if !(10..=64000).contains(&self.cpu_millis)
            || !(64 * 1024 * 1024..=64 * 1024 * 1024 * 1024).contains(&self.memory_bytes)
            || !(16..=1024).contains(&self.processes)
        {
            return Err(Error::Cgroup);
        }
        Ok(())
    }
}

/// Caller retains this owned generation until every exact task is physically
/// reconciled. Drop intentionally does not kill, remove or release anything.
pub struct CgroupParent {
    base: Dir,
    base_id: NativeId,
    directory: Dir,
    id: NativeId,
    name: String,
    oci_prefix: String,
    serial: std::sync::Mutex<()>,
}
pub struct TaskCgroup {
    base: Dir,
    base_id: NativeId,
    parent_name: String,
    parent: Dir,
    parent_id: NativeId,
    directory: Dir,
    id: NativeId,
    name: String,
    oci_path: String,
    limits: HardLimits,
}
impl CgroupParent {
    pub fn create() -> Result<Self> {
        Self::create_in(Path::new("/sys/fs/cgroup"))
    }
    /// Trusted bootstrap may supply its own delegated generation. Every created
    /// task stays under that exact native cgroup; no existing child is adopted.
    pub fn create_in(path: &Path) -> Result<Self> {
        if unsafe { libc::geteuid() } != 0 {
            return Err(Error::Cgroup);
        }
        let relative = path
            .strip_prefix("/sys/fs/cgroup")
            .map_err(|_| Error::Cgroup)?;
        if relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(Error::Cgroup);
        }
        for ancestor in path.ancestors() {
            let metadata = std::fs::symlink_metadata(ancestor)?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || metadata.uid() != 0
                || metadata.mode() & 0o022 != 0
            {
                return Err(Error::Cgroup);
            }
        }
        let oci_prefix = relative
            .to_str()
            .ok_or(Error::Cgroup)?
            .trim_end_matches('/')
            .to_owned();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let base = Dir::from_std_file(file);
        let base_id = NativeId::capture(&base)?;
        // Read-only check: no enabling controllers on somebody else's ancestor.
        let available = read(&base, "cgroup.subtree_control")?;
        if ["cpu", "memory", "pids"]
            .iter()
            .any(|v| !available.split_ascii_whitespace().any(|x| x == *v))
        {
            return Err(Error::Cgroup);
        }
        let name = format!("cowork-{}", uuid::Uuid::new_v4().simple());
        base.create_dir(&name)?;
        let directory = base.open_dir_nofollow(&name)?;
        let id = NativeId::capture(&directory)?;
        let result = Self {
            base,
            base_id,
            directory,
            id,
            name,
            oci_prefix,
            serial: std::sync::Mutex::new(()),
        };
        result.require()?;
        if !empty(&result.directory)? {
            return Err(Error::Cgroup);
        }
        write(
            &result.directory,
            "cgroup.subtree_control",
            "+cpu +memory +pids",
        )?;
        write(&result.directory, "cgroup.max.depth", "4")?;
        write(&result.directory, "cgroup.max.descendants", "1024")?;
        Ok(result)
    }
    pub(super) fn require(&self) -> Result<()> {
        self.base_id.require(&self.base)?;
        self.id.require(&self.directory)?;
        self.id.require(&self.base.open_dir_nofollow(&self.name)?)
    }
    pub fn reserve(&self, limits: HardLimits) -> Result<TaskCgroup> {
        let _guard = self.serial.lock().map_err(|_| Error::Cgroup)?;
        limits.validate()?;
        self.require()?;
        if directories(&self.directory)?.len() >= 128 {
            return Err(Error::Cgroup);
        }
        let name = format!("task-{}", uuid::Uuid::new_v4().simple());
        self.directory.create_dir(&name)?;
        let directory = self.directory.open_dir_nofollow(&name)?;
        let id = NativeId::capture(&directory)?;
        write(
            &directory,
            "cpu.max",
            &format!("{} 100000", u64::from(limits.cpu_millis) * 100),
        )?;
        write(&directory, "memory.max", &limits.memory_bytes.to_string())?;
        write(&directory, "memory.swap.max", "0")?;
        write(&directory, "memory.oom.group", "1")?;
        write(&directory, "pids.max", &limits.processes.to_string())?;
        let group = TaskCgroup {
            base: self.base.try_clone()?,
            base_id: self.base_id,
            parent_name: self.name.clone(),
            parent: self.directory.try_clone()?,
            parent_id: self.id,
            directory,
            id,
            oci_path: if self.oci_prefix.is_empty() {
                format!("/{}/{name}", self.name)
            } else {
                format!("/{}/{}/{name}", self.oci_prefix, self.name)
            },
            name,
            limits,
        };
        group.verify()?;
        Ok(group)
    }
    /// No tasks/children may remain. This never kills or recursively removes.
    pub fn retire_empty(self) -> Result<()> {
        self.require()?;
        if !empty(&self.directory)? || !directories(&self.directory)?.is_empty() {
            return Err(Error::Cgroup);
        }
        self.base.remove_dir(&self.name)?;
        Ok(())
    }
}
impl TaskCgroup {
    pub fn oci_path(&self) -> &str {
        &self.oci_path
    }
    pub fn limits(&self) -> HardLimits {
        self.limits
    }
    pub fn verify(&self) -> Result<()> {
        self.base_id.require(&self.base)?;
        self.parent_id
            .require(&self.base.open_dir_nofollow(&self.parent_name)?)?;
        self.parent_id.require(&self.parent)?;
        self.id.require(&self.directory)?;
        self.id
            .require(&self.parent.open_dir_nofollow(&self.name)?)?;
        if read(&self.directory, "cpu.max")?
            != format!("{} 100000", u64::from(self.limits.cpu_millis) * 100)
            || read(&self.directory, "memory.max")? != self.limits.memory_bytes.to_string()
            || read(&self.directory, "memory.swap.max")? != "0"
            || read(&self.directory, "memory.oom.group")? != "1"
            || read(&self.directory, "pids.max")? != self.limits.processes.to_string()
        {
            return Err(Error::Cgroup);
        }
        Ok(())
    }
    /// Observation only: emptiness alone is not physical completion authority.
    pub fn is_empty(&self) -> Result<bool> {
        self.verify()?;
        empty(&self.directory)
    }
    /// Only after a caller has separately proved all create/start mutations
    /// completed and the exact owned task has been deleted. Kept crate-private.
    pub(super) fn retire_after_task_delete(self) -> Result<()> {
        self.verify()?;
        if !empty(&self.directory)? || !directories(&self.directory)?.is_empty() {
            return Err(Error::Cgroup);
        }
        self.parent.remove_dir(&self.name)?;
        Ok(())
    }
    /// This is safe only before publication to a task adapter. It consumes the
    /// reservation and refuses populated/descendant groups. No Drop fallback.
    pub fn retire_unlaunched(self) -> Result<()> {
        self.retire_after_task_delete()
    }
}
fn read(dir: &Dir, name: &str) -> Result<String> {
    let mut value = String::new();
    Read::by_ref(&mut dir.open(name)?)
        .take(65537)
        .read_to_string(&mut value)?;
    if value.len() > 65536 {
        return Err(Error::Cgroup);
    }
    Ok(value.trim().into())
}
fn write(dir: &Dir, name: &str, value: &str) -> Result<()> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true);
    dir.open_with(name, &options)?.write_all(value.as_bytes())?;
    Ok(())
}
fn directories(dir: &Dir) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in dir.entries()? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            names.push(entry.file_name().to_str().ok_or(Error::Cgroup)?.into());
            if names.len() > 1024 {
                return Err(Error::Cgroup);
            }
        }
    }
    Ok(names)
}
fn empty(dir: &Dir) -> Result<bool> {
    let events = read(dir, "cgroup.events")?;
    if events.lines().any(|line| line == "populated 0") {
        return Ok(true);
    }
    if events.lines().any(|line| line == "populated 1") {
        return Ok(false);
    }
    Err(Error::Cgroup)
}
