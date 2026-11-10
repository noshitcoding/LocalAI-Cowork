//! Snapshot and promotion for the existing FileBroker workspace. The same OS lock
//! is held through staging, process termination and promotion; no host folder grant.
use anyhow::{Context, Result, ensure};
use cap_fs_ext::MetadataExt;
use cap_std::{ambient_authority, fs::Dir};
use cowork_domain::canonical_hash;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Read,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub sha256: String,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub directory: bool,
}
pub type Manifest = BTreeMap<String, Entry>;
const MAX_FILES: usize = 10000;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_TOTAL: u64 = 512 * 1024 * 1024;
pub fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn relative(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && path.len() <= 1024 && !path.contains(['\\', ':', '\0']),
        "Invalid workspace path"
    );
    ensure!(
        Path::new(path)
            .components()
            .all(|p| matches!(p, Component::Normal(_))),
        "Workspace traversal denied"
    );
    Ok(())
}
fn ordinary_file(path: &Path, mounted: bool) -> Result<()> {
    if !mounted {
        super::config::ordinary(path, false)?;
    }
    let m = std::fs::symlink_metadata(path)?;
    #[cfg(unix)]
    {
        ensure!(
            std::os::unix::fs::MetadataExt::nlink(&m) == 1,
            "Linked workspace file denied"
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            m.file_attributes() & 0x400 == 0,
            "Workspace reparse point denied"
        );
    }
    ensure!(m.len() <= MAX_FILE, "Workspace file limit exceeded");
    Ok(())
}
fn open_read(path: &Path) -> Result<File> {
    open_read_inner(path, false)
}
fn open_read_inner(path: &Path, mounted: bool) -> Result<File> {
    ordinary_file(path, mounted)?;
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
    let file = o.open(path)?;
    ensure!(
        cap_std::fs::File::from_std(file.try_clone()?)
            .metadata()?
            .nlink()
            == 1,
        "Linked workspace file denied"
    );
    Ok(file)
}
pub fn files(root: &Path) -> Result<Manifest> {
    super::config::ordinary(root, true)?;
    scan(root, false)
}
// Only the Windows adapter calls this after its exact owned Job is quiescent and
// Capsule::close has restored private ACLs. The intentional VHD mount ancestor
// is allowed; every capsule root/descendant still rejects reparse points.
#[cfg(windows)]
pub fn files_capsule(root: &Path) -> Result<Manifest> {
    let m = root.symlink_metadata()?;
    use std::os::windows::fs::MetadataExt;
    ensure!(
        m.is_dir() && m.file_attributes() & 0x400 == 0,
        "Capsule output root changed"
    );
    scan(root, true)
}
fn scan(root: &Path, mounted: bool) -> Result<Manifest> {
    let mut result = Manifest::new();
    let mut total = 0u64;
    let mut entries = 0usize;
    let mut pending = vec![String::new()];
    while let Some(parent) = pending.pop() {
        for entry in std::fs::read_dir(root.join(&parent))? {
            let entry = entry?;
            entries += 1;
            ensure!(entries <= MAX_FILES, "Workspace path limit exceeded");
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Non-UTF8 workspace path"))?;
            let relative = if parent.is_empty() {
                name
            } else {
                format!("{parent}/{name}")
            };
            self::relative(&relative)?;
            let m = entry.path().symlink_metadata()?;
            ensure!(!m.file_type().is_symlink(), "Workspace symlink denied");
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    m.file_attributes() & 0x400 == 0,
                    "Workspace reparse point denied"
                );
            }
            if m.is_dir() {
                ensure!(
                    pending.len() + result.len() < MAX_FILES,
                    "Workspace path limit exceeded"
                );
                result.insert(
                    relative.clone(),
                    Entry {
                        sha256: digest(b""),
                        bytes: 0,
                        directory: true,
                    },
                );
                pending.push(relative);
                continue;
            }
            ensure!(
                m.is_file() && result.len() < MAX_FILES,
                "Unsupported/oversized workspace entry"
            );
            let mut f = open_read_inner(&entry.path(), mounted)?;
            let mut bytes = Vec::new();
            Read::by_ref(&mut f)
                .take(MAX_FILE + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 == m.len() && bytes.len() as u64 <= MAX_FILE,
                "Workspace changed during snapshot"
            );
            total = total
                .checked_add(bytes.len() as u64)
                .context("Workspace size overflow")?;
            ensure!(total <= MAX_TOTAL, "Workspace size limit exceeded");
            result.insert(
                relative,
                Entry {
                    sha256: digest(&bytes),
                    bytes: bytes.len() as u64,
                    directory: false,
                },
            );
        }
    }
    Ok(result)
}
pub fn copy(root: &Path, target: &Path, manifest: &Manifest) -> Result<()> {
    copy_inner(root, target, manifest, false)
}
#[cfg(windows)]
pub fn copy_capsule(root: &Path, target: &Path, manifest: &Manifest) -> Result<()> {
    ensure!(
        files_capsule(root)? == *manifest,
        "Capsule output changed before export"
    );
    copy_inner(root, target, manifest, true)
}
fn copy_inner(root: &Path, target: &Path, manifest: &Manifest, mounted: bool) -> Result<()> {
    super::config::private_directory(target)?;
    for (name, expected) in manifest {
        relative(name)?;
        if expected.directory {
            let dest = target.join(name);
            if !dest.exists() {
                super::config::private_directory(&dest)?;
            }
            continue;
        }
        let mut f = open_read_inner(&root.join(name), mounted)?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut f)
            .take(MAX_FILE + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == expected.bytes && digest(&bytes) == expected.sha256,
            "Input changed after snapshot"
        );
        let dest = target.join(name);
        let mut parents = Vec::new();
        let mut p = dest.parent().context("Missing file parent")?;
        while p != target {
            parents.push(p.to_path_buf());
            p = p.parent().context("Invalid input parent")?;
        }
        for parent in parents.into_iter().rev() {
            if !parent.exists() {
                super::config::private_directory(&parent)?;
            }
        }
        super::config::write_new(&dest, &bytes)?;
    }
    for name in manifest.keys().rev().chain(std::iter::once(&String::new())) {
        let path = target.join(name);
        if path.is_dir() {
            super::config::sync_directory(&path)?;
        }
    }
    Ok(())
}
pub struct Workspace {
    pub root: PathBuf,
    pub before: Manifest,
    _directory: Dir,
    _lock: File,
}
impl Workspace {
    pub fn acquire(state: &Path, resource: &str) -> Result<Self> {
        let mut found = None;
        let mut count = 0;
        for entry in std::fs::read_dir(state.join("workspaces"))? {
            count += 1;
            ensure!(count <= 10000, "Workspace registry bound exceeded");
            let entry = entry?;
            super::config::ordinary(&entry.path(), true)?;
            let marker = entry.path().join("workspace.json");
            if !marker.is_file() {
                continue;
            }
            let m = marker.symlink_metadata()?;
            ensure!(
                !m.file_type().is_symlink() && m.len() <= 65536,
                "Invalid workspace marker"
            );
            let value: serde_json::Value = serde_json::from_reader(open_read(&marker)?)?;
            if value
                .pointer("/folder/resource_id")
                .and_then(|v| v.as_str())
                != Some(resource)
            {
                continue;
            }
            ensure!(found.is_none(), "Ambiguous workspace resource");
            let root = entry.path().join("files");
            super::config::ordinary(&root, true)?;
            let directory = Dir::open_ambient_dir(&root, ambient_authority())?;
            let meta = directory.dir_metadata()?;
            let identity = format!("{}-{}", meta.dev(), meta.ino());
            ensure!(
                digest(identity.as_bytes()) == resource,
                "Workspace identity changed"
            );
            let lock_path = state.join("locks").join(identity);
            super::config::ordinary(&state.join("locks"), true)?;
            let mut o = OpenOptions::new();
            o.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.custom_flags(libc::O_NOFOLLOW).mode(0o600);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                o.custom_flags(0x00200000);
            }
            let lock = o.open(lock_path)?;
            ensure!(lock.metadata()?.is_file(), "Invalid resource lock");
            lock.try_lock().context("Workspace is in use")?;
            let before = files(&root)?;
            found = Some(Self {
                root,
                before,
                _directory: directory,
                _lock: lock,
            });
        }
        found.context("Exact broker workspace not found")
    }
    pub fn promote(&self, output: &Path, journal: &Path) -> Result<()> {
        ensure!(
            files(&self.root)? == self.before,
            "Workspace changed outside broker lock"
        );
        let after = files(output)?;
        // The complete manifest is retained before the first host mutation. A lost
        // promotion reply remains uncertain; the receiver never repeats this operation.
        super::config::durable_json(
            journal,
            &serde_json::json!({"before":self.before,"after":after,"output":output}),
        )?;
        // Delete leaves before parents. Type replacements are explicit; directory
        // deletion is non-recursive and cannot traverse an unexpected descendant.
        let mut obsolete: Vec<_> = self
            .before
            .iter()
            .filter(|(name, old)| {
                after
                    .get(*name)
                    .is_none_or(|new| new.directory != old.directory)
            })
            .collect();
        obsolete.sort_by_key(|(name, _)| std::cmp::Reverse(name.matches('/').count()));
        for (name, old) in obsolete {
            if old.directory {
                self._directory.remove_dir(name)?;
            } else {
                self._directory.remove_file(name)?;
            }
        }
        // Every directory, including an empty one, is a first-class snapshot entry.
        for (name, item) in &after {
            if item.directory && !self._directory.exists(name) {
                self._directory.create_dir(name)?;
            }
        }
        for (name, item) in &after {
            if item.directory {
                continue;
            }
            if self.before.get(name) == Some(item) {
                continue;
            }
            let mut f = open_read(&output.join(name))?;
            let mut bytes = Vec::new();
            Read::by_ref(&mut f)
                .take(MAX_FILE + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                digest(&bytes) == item.sha256,
                "Output changed before promotion"
            );
            let dest = self.root.join(name);
            let parent = dest.parent().context("Invalid output parent")?;
            super::config::ordinary(parent, true)?;
            let temp = parent.join(format!(".cowork-{}.tmp", uuid::Uuid::new_v4()));
            super::config::write_new(&temp, &bytes)?;
            // Cap-std performs beneath-root resolution, including replacement.
            let temp_relative = temp.strip_prefix(&self.root)?;
            self._directory
                .rename(temp_relative, &self._directory, name)?;
        }
        ensure!(
            files(&self.root)? == after,
            "Promoted workspace verification failed"
        );
        // Flush renamed/deleted entries and newly created directories before the
        // committed receipt. On Windows this flushes the actual containing volume.
        for (name, item) in after.iter().rev() {
            if item.directory {
                super::config::sync_directory(&self.root.join(name))?;
            }
        }
        super::config::sync_directory(&self.root)?;
        super::config::durable_json(
            &journal.with_extension("committed.json"),
            &serde_json::json!({"after_sha256":canonical_hash(&after)}),
        )?;
        Ok(())
    }
}
