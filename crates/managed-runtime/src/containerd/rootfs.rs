//! Sealed package root filesystem; no host tool/PATH fallback. Every installed
//! byte/link is covered by an independently pinned manifest, not by discovery.
use super::gvisor::{Stamp, require_directory};
use super::*;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RootfsEntry {
    Directory,
    File {
        bytes: u64,
        sha256: String,
        executable: bool,
    },
    Symlink {
        target: String,
    },
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootfsManifest {
    pub format_version: u32,
    pub entries: BTreeMap<String, RootfsEntry>,
}
pub struct SealedRootfs {
    directory: Dir,
    path: PathBuf,
    root: Stamp,
    manifest_sha256: String,
    manifest: RootfsManifest,
    stamps: BTreeMap<String, Stamp>,
}
impl SealedRootfs {
    pub fn open(
        path: &Path,
        exact_manifest: &[u8],
        expected_manifest_sha256: &str,
    ) -> Result<Self> {
        if unsafe { libc::geteuid() } != 0
            || !path.is_absolute()
            || exact_manifest.len() > 16 * 1024 * 1024
            || !crate::valid_hash(expected_manifest_sha256)
            || format!("{:x}", Sha256::digest(exact_manifest)) != expected_manifest_sha256
        {
            return Err(Error::Rootfs);
        }
        super::gvisor::require_owned_ancestors(path)?;
        for parent in path.ancestors() {
            if std::fs::symlink_metadata(parent)?.file_type().is_symlink() {
                return Err(Error::Rootfs);
            }
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        require_directory(&file.metadata()?)?;
        let root = Stamp::of(&file.metadata()?);
        let directory = Dir::from_std_file(file);
        let manifest: RootfsManifest =
            serde_json::from_slice(exact_manifest).map_err(|_| Error::Rootfs)?;
        if manifest.format_version != 1
            || manifest.entries.is_empty()
            || manifest.entries.len() > 200000
            || manifest.entries.keys().any(|p| !valid_path(p))
        {
            return Err(Error::Rootfs);
        }
        if inventory(&directory)? != manifest.entries.keys().cloned().collect::<Vec<_>>() {
            return Err(Error::Rootfs);
        }
        let mut total = 0u64;
        let mut stamps = BTreeMap::new();
        for (name, entry) in &manifest.entries {
            let native = std::fs::symlink_metadata(path.join(name))?;
            match entry {
                RootfsEntry::Directory => require_directory(&native)?,
                RootfsEntry::Symlink { target } => {
                    if !native.file_type().is_symlink()
                        || native.uid() != 0
                        || target.is_empty()
                        || target.len() > 1024
                        || target.contains(['\0', '\\'])
                        || std::fs::read_link(path.join(name))?.to_str() != Some(target.as_str())
                    {
                        return Err(Error::Rootfs);
                    }
                    resolve_target(name, target)?;
                }
                RootfsEntry::File {
                    bytes,
                    sha256,
                    executable,
                } => {
                    total = total.checked_add(*bytes).ok_or(Error::Rootfs)?;
                    if *bytes > 12 * 1024 * 1024 * 1024
                        || total > 12 * 1024 * 1024 * 1024
                        || !crate::valid_hash(sha256)
                        || !native.is_file()
                        || native.uid() != 0
                        || native.nlink() != 1
                        || native.len() != *bytes
                        || native.mode() & 0o7777 != if *executable { 0o555 } else { 0o444 }
                    {
                        return Err(Error::Rootfs);
                    }
                    let mut options = cap_std::fs::OpenOptions::new();
                    options.read(true).follow(FollowSymlinks::No);
                    let mut file = directory.open_with(name, &options)?.into_std();
                    let initial = Stamp::of(&native);
                    if Stamp::of(&file.metadata()?) != initial {
                        return Err(Error::Rootfs);
                    }
                    let mut hash = Sha256::new();
                    let count = io::copy(&mut Read::by_ref(&mut file).take(bytes + 1), &mut hash)?;
                    if count != *bytes
                        || format!("{:x}", hash.finalize()) != *sha256
                        || Stamp::of(&file.metadata()?) != initial
                    {
                        return Err(Error::Rootfs);
                    }
                }
            }
            stamps.insert(name.clone(), Stamp::of(&native));
        }
        let rootfs = Self {
            directory,
            path: path.into(),
            root,
            manifest_sha256: expected_manifest_sha256.into(),
            manifest,
            stamps,
        };
        rootfs.verify()?;
        Ok(rootfs)
    }
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub fn verify(&self) -> Result<()> {
        let root = std::fs::symlink_metadata(&self.path)?;
        if Stamp::of(&root) != self.root
            || inventory(&self.directory)? != self.stamps.keys().cloned().collect::<Vec<_>>()
        {
            return Err(Error::Rootfs);
        }
        for (name, stamp) in &self.stamps {
            if Stamp::of(&std::fs::symlink_metadata(self.path.join(name))?) != *stamp {
                return Err(Error::Rootfs);
            }
        }
        Ok(())
    }
    /// Resolve package-relative links using guest-root semantics. No host path
    /// lookup is used to discover executable authority or follow absolute links.
    pub fn executable(&self, relative: &str) -> Result<String> {
        if !valid_path(relative) {
            return Err(Error::Rootfs);
        }
        let mut path = relative.to_owned();
        for _ in 0..40 {
            let mut prefix = String::new();
            let components: Vec<String> = path.split('/').map(str::to_owned).collect();
            let mut redirected = false;
            for (i, part) in components.iter().enumerate() {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(part);
                match self.manifest.entries.get(&prefix).ok_or(Error::Rootfs)? {
                    RootfsEntry::Symlink { target } => {
                        path = resolve_target(&prefix, target)?;
                        if i + 1 < components.len() {
                            path.push('/');
                            path.push_str(&components[i + 1..].join("/"));
                        }
                        redirected = true;
                        break;
                    }
                    RootfsEntry::Directory if i + 1 < components.len() => {}
                    RootfsEntry::File {
                        executable: true, ..
                    } if i + 1 == components.len() => return Ok(format!("/{relative}")),
                    _ => return Err(Error::Rootfs),
                }
            }
            if !redirected {
                return Err(Error::Rootfs);
            }
        }
        Err(Error::Rootfs)
    }
}
fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.starts_with('/')
        && !path.contains(['\0', '\\'])
        && path
            .split('/')
            .all(|p| !p.is_empty() && !matches!(p, "." | ".."))
}
fn resolve_target(link: &str, target: &str) -> Result<String> {
    let mut parts = if target.starts_with('/') {
        Vec::new()
    } else {
        link.split('/')
            .take(link.split('/').count() - 1)
            .collect::<Vec<_>>()
    };
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or(Error::Rootfs)?;
            }
            _ => parts.push(part),
        }
    }
    let path = parts.join("/");
    if !valid_path(&path) {
        return Err(Error::Rootfs);
    }
    Ok(path)
}
fn inventory(dir: &Dir) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let mut pending = vec![String::new()];
    while let Some(prefix) = pending.pop() {
        let mut directory = dir.try_clone()?;
        if !prefix.is_empty() {
            for part in prefix.split('/') {
                directory = directory.open_dir_nofollow(part)?;
            }
        }
        for entry in directory.entries()? {
            let entry = entry?;
            let leaf = entry.file_name().to_str().ok_or(Error::Rootfs)?.to_owned();
            let name = if prefix.is_empty() {
                leaf.clone()
            } else {
                format!("{prefix}/{leaf}")
            };
            if !valid_path(&name) {
                return Err(Error::Rootfs);
            }
            names.push(name.clone());
            if names.len() > 200000 {
                return Err(Error::Rootfs);
            }
            if entry.file_type()?.is_dir() {
                pending.push(name);
            } else if !entry.file_type()?.is_file() && !entry.file_type()?.is_symlink() {
                return Err(Error::Rootfs);
            }
        }
    }
    names.sort();
    Ok(names)
}
