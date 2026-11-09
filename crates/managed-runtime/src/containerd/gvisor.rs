//! Complete immutable gVisor toolset. Manifest hash must come from trusted
//! package/qualification state. A self-declared manifest grants no authority.
use super::*;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolFile {
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsetManifest {
    pub format_version: u32,
    pub version: String,
    pub files: BTreeMap<String, ToolFile>,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stamp {
    dev: u64,
    ino: u64,
    bytes: u64,
    mode: u32,
    uid: u32,
    links: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
impl Stamp {
    pub(super) fn of(m: &std::fs::Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            bytes: m.len(),
            mode: m.mode(),
            uid: m.uid(),
            links: m.nlink(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
        }
    }
}
struct PinnedTool {
    file: File,
    stamp: Stamp,
    sha256: String,
}
pub struct GvisorTools {
    directory: Dir,
    path: PathBuf,
    dev: u64,
    ino: u64,
    manifest_sha256: String,
    files: BTreeMap<String, PinnedTool>,
}
impl GvisorTools {
    /// Expected SHA is independently bound to the installed signed package.
    /// Unlike control records, these public binaries must be readable/executable
    /// by the unprivileged Sentry; every directory and file is root-owned 0555.
    pub fn open(
        path: &Path,
        exact_manifest: &[u8],
        expected_manifest_sha256: &str,
    ) -> Result<Self> {
        if unsafe { libc::geteuid() } != 0
            || !path.is_absolute()
            || exact_manifest.len() > 65536
            || !crate::valid_hash(expected_manifest_sha256)
            || format!("{:x}", Sha256::digest(exact_manifest)) != expected_manifest_sha256
        {
            return Err(Error::Tools);
        }
        require_owned_ancestors(path)?;
        for ancestor in path.ancestors() {
            let m = std::fs::symlink_metadata(ancestor)?;
            if !m.is_dir()
                || m.uid() != 0
                || m.mode() & 0o001 == 0
                || (m.mode() & 0o022 != 0 && m.mode() & libc::S_ISVTX == 0)
            {
                return Err(Error::Tools);
            }
        }
        let manifest: ToolsetManifest =
            serde_json::from_slice(exact_manifest).map_err(|_| Error::Tools)?;
        if manifest.format_version != 1
            || manifest.version.is_empty()
            || manifest.version.len() > 128
            || !(3..=32).contains(&manifest.files.len())
            || !manifest.files.contains_key("runsc")
            || !manifest.files.contains_key("containerd-shim-runsc-v1")
            || !manifest.files.keys().any(|p| p.starts_with("gvisor-bin/"))
        {
            return Err(Error::Tools);
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let m = file.metadata()?;
        require_directory(&m)?;
        let dev = m.dev();
        let ino = m.ino();
        let directory = Dir::from_std_file(file);
        let names = inventory(&directory)?;
        if names != manifest.files.keys().cloned().collect::<Vec<_>>() {
            return Err(Error::Tools);
        }
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        for (name, expected) in manifest.files {
            if !valid_name(&name)
                || !(4..=128 * 1024 * 1024).contains(&expected.bytes)
                || !crate::valid_hash(&expected.sha256)
            {
                return Err(Error::Tools);
            }
            total = total.checked_add(expected.bytes).ok_or(Error::Tools)?;
            if total > 512 * 1024 * 1024 {
                return Err(Error::Tools);
            }
            let mut options = cap_std::fs::OpenOptions::new();
            options.read(true);
            use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
            options.follow(FollowSymlinks::No);
            let mut file = directory.open_with(&name, &options)?.into_std();
            let m = file.metadata()?;
            if !m.is_file()
                || m.uid() != 0
                || m.mode() & 0o7777 != 0o555
                || m.nlink() != 1
                || m.len() != expected.bytes
            {
                return Err(Error::Tools);
            }
            let stamp = Stamp::of(&m);
            let mut hash = Sha256::new();
            let mut header = [0u8; 4];
            file.read_exact(&mut header)?;
            if header != *b"\x7fELF" {
                return Err(Error::Tools);
            }
            hash.update(header);
            let count = io::copy(&mut Read::by_ref(&mut file).take(expected.bytes), &mut hash)?;
            if count + 4 != expected.bytes
                || format!("{:x}", hash.finalize()) != expected.sha256
                || Stamp::of(&file.metadata()?) != stamp
            {
                return Err(Error::Tools);
            }
            files.insert(
                name,
                PinnedTool {
                    file,
                    stamp,
                    sha256: expected.sha256,
                },
            );
        }
        let tools = Self {
            directory,
            path: path.into(),
            dev,
            ino,
            manifest_sha256: expected_manifest_sha256.into(),
            files,
        };
        tools.verify()?;
        Ok(tools)
    }
    /// Live native/stat checks supplement the initial full hash. Root-owned
    /// immutable package files cannot be modified by a workload. Replacement,
    /// chmod, extra files or changed contents/timestamps invalidate this binding.
    pub fn verify(&self) -> Result<()> {
        let m = std::fs::symlink_metadata(&self.path)?;
        require_directory(&m)?;
        if m.dev() != self.dev
            || m.ino() != self.ino
            || inventory(&self.directory)? != self.files.keys().cloned().collect::<Vec<_>>()
        {
            return Err(Error::Tools);
        }
        for (name, pinned) in &self.files {
            if Stamp::of(&pinned.file.metadata()?) != pinned.stamp {
                return Err(Error::Tools);
            }
            let path = self.path.join(name);
            let m = std::fs::symlink_metadata(path)?;
            if !m.is_file() || Stamp::of(&m) != pinned.stamp {
                return Err(Error::Tools);
            }
        }
        Ok(())
    }
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
    pub(super) fn shim_path(&self) -> Result<PathBuf> {
        self.verify()?;
        Ok(self.path.join("containerd-shim-runsc-v1"))
    }
    pub(super) fn runsc(&self) -> Result<(PathBuf, String, Stamp)> {
        self.verify()?;
        let pinned = self.files.get("runsc").ok_or(Error::Tools)?;
        Ok((self.path.join("runsc"), pinned.sha256.clone(), pinned.stamp))
    }
    pub(super) fn configuration(
        &self,
        shim_cgroup: &str,
        state_root: &Path,
        driver: &Path,
    ) -> Result<String> {
        self.verify()?;
        Ok(format!(
            "binary_name = {}\nroot = {}\nshim_cgroup = {}\ngrouping = false\nenable_hibernate_server = false\n[runsc_config]\nplatform = \"systrap\"\nnetwork = \"none\"\nhost-uds = \"none\"\nhost-fifo = \"none\"\ndirectfs = \"false\"\ndebug = \"false\"\ndebug-log = {}\n",
            serde_json::to_string(driver).map_err(|_| Error::Tools)?,
            serde_json::to_string(state_root).map_err(|_| Error::Tools)?,
            serde_json::to_string(shim_cgroup).map_err(|_| Error::Tools)?,
            serde_json::to_string(&state_root.with_file_name("runsc-debug.log"))
                .map_err(|_| Error::Tools)?
        ))
    }
}
pub(super) fn require_directory(m: &std::fs::Metadata) -> Result<()> {
    if !m.is_dir() || m.uid() != 0 || m.mode() & 0o7777 != 0o555 {
        return Err(Error::Tools);
    }
    Ok(())
}
/// Paths later reopened by shims/OCI must not be replaceable by an
/// unprivileged owner of an ancestor. Root-owned sticky /tmp is admissible
/// because the protected child itself is root-owned; symlinks are rejected.
pub(super) fn require_owned_ancestors(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.components().any(|c| {
            !matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(Error::Tools);
    }
    for ancestor in path.ancestors() {
        let m = std::fs::symlink_metadata(ancestor)?;
        if !m.is_dir() || m.uid() != 0 || (m.mode() & 0o022 != 0 && m.mode() & libc::S_ISVTX == 0) {
            return Err(Error::Tools);
        }
    }
    Ok(())
}
fn valid_name(name: &str) -> bool {
    matches!(name, "runsc" | "containerd-shim-runsc-v1")
        || name.strip_prefix("gvisor-bin/").is_some_and(|n| {
            !n.is_empty()
                && n.len() <= 128
                && n.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
}
fn inventory(dir: &Dir) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in dir.entries()? {
        let entry = entry?;
        let name = entry.file_name().to_str().ok_or(Error::Tools)?.to_owned();
        if entry.file_type()?.is_file()
            && matches!(name.as_str(), "runsc" | "containerd-shim-runsc-v1")
        {
            names.push(name);
        } else if name == "gvisor-bin" && entry.file_type()?.is_dir() {
            let sub = dir.open_dir_nofollow(&name)?;
            require_directory(&sub.open(".")?.into_std().metadata()?)?;
            for child in sub.entries()? {
                let child = child?;
                let name = format!(
                    "gvisor-bin/{}",
                    child.file_name().to_str().ok_or(Error::Tools)?
                );
                if !child.file_type()?.is_file() || !valid_name(&name) {
                    return Err(Error::Tools);
                }
                names.push(name);
                if names.len() > 32 {
                    return Err(Error::Tools);
                }
            }
        } else {
            return Err(Error::Tools);
        }
    }
    names.sort();
    Ok(names)
}
