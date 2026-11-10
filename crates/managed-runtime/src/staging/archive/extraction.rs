//! Linux-only extraction in the private management filesystem. Every original
//! target inode remains pinned until the returned capability is dropped.
use super::*;
use cap_std::fs::{
    DirBuilder, DirBuilderExt, MetadataExt, OpenOptionsExt, Permissions, PermissionsExt,
};
mod custody;
pub(super) use custody::DescriptorReservation;
use custody::PinnedTarget;
pub use custody::TargetDescriptorPolicy;

pub struct ExtractedArchive {
    source: StagedPackage,
    tree: ReservedTree,
}

pub(super) struct ReservedTree {
    area: StagingArea,
    members: BTreeMap<String, Member>,
    targets: BTreeMap<String, PinnedTarget>,
    expanded_bytes: u64,
}

impl InspectedArchive {
    pub fn extract(self, area: StagingArea) -> Result<ExtractedArchive> {
        self.extract_with_policy(area, TargetDescriptorPolicy::new(MAX_ENTRIES)?)
    }
    pub fn extract_with_policy(
        mut self,
        area: StagingArea,
        policy: TargetDescriptorPolicy,
    ) -> Result<ExtractedArchive> {
        if unsafe { libc::geteuid() } != 0 {
            return Err(Error::Archive("root_manager_required"));
        }
        self.verify()?;
        area.check()?;
        exact_entries(&area.directory, &[])?;
        let depth = self
            .members
            .keys()
            .map(|name| name.split('/').count())
            .max()
            .unwrap_or(0);
        let mut slots =
            DescriptorReservation::reserve(&area.directory, self.members.len(), depth, policy)?;
        let mut tree = ReservedTree::reserve(area, self.members, self.expanded_bytes, &mut slots)?;
        tree.copy(&mut self.staged, self.component)?;
        tree.finish(&mut slots)?;
        if !slots.is_empty() {
            return Err(Error::Staging);
        }
        let mut extracted = ExtractedArchive {
            source: self.staged,
            tree,
        };
        extracted.verify()?;
        Ok(extracted)
    }
}
impl ExtractedArchive {
    pub fn expanded_bytes(&self) -> u64 {
        self.tree.expanded_bytes
    }
    pub fn retained_target_descriptors(&self) -> usize {
        self.tree.targets.len()
    }
    pub fn verify(&mut self) -> Result<()> {
        self.source.verify()?;
        self.tree.verify()
    }
}
impl ReservedTree {
    /// Fresh read-only native acquisition of an existing role against the
    /// independently signed inventory. No stored native identifiers are used,
    /// and no permission, byte, link or directory is created or repaired.
    pub(super) fn open_current(
        area: StagingArea,
        members: BTreeMap<String, Member>,
        expanded_bytes: u64,
        slots: &mut DescriptorReservation,
    ) -> Result<Self> {
        area.check()?;
        let mut targets = BTreeMap::new();
        for (path, member) in &members {
            if matches!(member.entry, ArchiveEntry::Directory) {
                slots.release_one()?;
                let (parent, leaf) = parent(&area, &targets, path)?;
                let directory = parent.open_dir_nofollow(leaf)?;
                if native::leaf_identity(parent, leaf)? != native::directory_identity(&directory)? {
                    return Err(Error::Staging);
                }
                targets.insert(path.clone(), PinnedTarget::Directory(directory));
            }
        }
        for (path, member) in &members {
            if matches!(member.entry, ArchiveEntry::Directory) {
                continue;
            }
            slots.release_one()?;
            let (parent, leaf) = parent(&area, &targets, path)?;
            let value = match &member.entry {
                ArchiveEntry::File { .. } => PinnedTarget::File(native::pin_file(parent, leaf)?),
                ArchiveEntry::Symlink { .. } => {
                    PinnedTarget::Symlink(native::pin_link(parent, leaf)?)
                }
                ArchiveEntry::Directory => return Err(Error::Staging),
            };
            if native::leaf_identity(parent, leaf)? != value.identity()? {
                return Err(Error::Staging);
            }
            targets.insert(path.clone(), value);
        }
        let value = Self {
            area,
            members,
            targets,
            expanded_bytes,
        };
        value.verify()?;
        Ok(value)
    }

    pub(super) fn entries(&self) -> usize {
        self.members.len()
    }
    pub(super) fn expanded_bytes(&self) -> u64 {
        self.expanded_bytes
    }
    pub(super) fn reserve(
        area: StagingArea,
        members: BTreeMap<String, Member>,
        expanded_bytes: u64,
        slots: &mut DescriptorReservation,
    ) -> Result<Self> {
        if expanded_bytes > area.payload_limit {
            return Err(Error::Archive("target_payload_limit"));
        }
        area.check()?;
        exact_entries(&area.directory, &[])?;
        let mut targets = BTreeMap::new();
        for (path, member) in &members {
            if matches!(member.entry, ArchiveEntry::Directory) {
                slots.release_one()?;
                let (parent, leaf) = parent(&area, &targets, path)?;
                let mut builder = DirBuilder::new();
                builder.mode(0o700);
                parent.create_dir_with(leaf, &builder)?;
                let directory = parent.open_dir_nofollow(leaf)?;
                if native::leaf_identity(parent, leaf)? != native::directory_identity(&directory)? {
                    return Err(Error::Staging);
                }
                native::sync_directory(parent)?;
                targets.insert(path.clone(), PinnedTarget::Directory(directory));
            }
        }
        // All file allocations precede the first file payload copy. Each
        // writable original descriptor remains owned through the copy.
        for (path, member) in &members {
            if let ArchiveEntry::File { bytes, .. } = member.entry {
                slots.release_one()?;
                let (parent, leaf) = parent(&area, &targets, path)?;
                let file = native::create_file(parent, leaf)?;
                if bytes != 0 {
                    native::allocate(&file, bytes)?;
                } else {
                    file.sync_all()?;
                }
                if native::leaf_identity(parent, leaf)? != native::file_identity(&file)? {
                    return Err(Error::Staging);
                }
                native::sync_directory(parent)?;
                targets.insert(path.clone(), PinnedTarget::File(file));
            }
        }
        Ok(Self {
            area,
            members,
            expanded_bytes,
            targets,
        })
    }
    pub(super) fn copy(&mut self, source: &mut StagedPackage, component: usize) -> Result<()> {
        let files: Vec<String> = self
            .members
            .iter()
            .filter(|(_, m)| matches!(m.entry, ArchiveEntry::File { .. }))
            .map(|(p, _)| p.clone())
            .collect();
        for path in files {
            {
                let (parent, leaf) = parent(&self.area, &self.targets, &path)?;
                if native::leaf_identity(parent, leaf)? != self.targets[&path].identity()? {
                    return Err(Error::Staging);
                }
            }
            let original = {
                let output = self
                    .targets
                    .get_mut(&path)
                    .ok_or(Error::Staging)?
                    .file_mut()?;
                copy_member(source, component, &self.members[&path], output)?;
                let ArchiveEntry::File { executable, .. } = self.members[&path].entry else {
                    return Err(Error::Staging);
                };
                output.set_permissions(Permissions::from_mode(if executable {
                    0o555
                } else {
                    0o444
                }))?;
                output.sync_all()?;
                native::file_identity(output)?
            };
            // Keep the writer alive while acquiring a readonly handle of the
            // same original. There is never an unpinned inode/reuse window.
            let (parent, leaf) = parent(&self.area, &self.targets, &path)?;
            let readonly = native::pin_file(parent, leaf)?;
            if native::file_identity(&readonly)? != original {
                return Err(Error::Staging);
            }
            self.targets.insert(path, PinnedTarget::File(readonly));
        }
        Ok(())
    }
    pub(super) fn finish(&mut self, slots: &mut DescriptorReservation) -> Result<()> {
        for (path, member) in &self.members {
            if let ArchiveEntry::Symlink { target } = &member.entry {
                slots.release_one()?;
                let (parent, leaf) = parent(&self.area, &self.targets, path)?;
                parent.symlink_contents(target, leaf)?;
                let link = native::pin_link(parent, leaf)?;
                if native::leaf_identity(parent, leaf)? != native::link_identity(&link)?
                    || native::read_pinned_link(&link)? != *target
                {
                    return Err(Error::Staging);
                }
                native::sync_directory(parent)?;
                self.targets
                    .insert(path.clone(), PinnedTarget::Symlink(link));
            }
        }
        for (path, member) in self.members.iter().rev() {
            if matches!(member.entry, ArchiveEntry::Directory) {
                let (parent, leaf) = parent(&self.area, &self.targets, path)?;
                let directory = self.targets[path].directory()?;
                if native::leaf_identity(parent, leaf)? != native::directory_identity(directory)? {
                    return Err(Error::Staging);
                }
                let mut options = OpenOptions::new();
                options
                    .read(true)
                    .follow(FollowSymlinks::No)
                    .custom_flags(libc::O_DIRECTORY);
                let file = directory.open_with(".", &options)?;
                let actual = file.metadata()?;
                let original = directory.dir_metadata()?;
                if !actual.is_dir()
                    || actual.dev() != original.dev()
                    || actual.ino() != original.ino()
                {
                    return Err(Error::Staging);
                }
                file.set_permissions(Permissions::from_mode(0o555))?;
                file.sync_all()?;
                native::sync_directory(parent)?;
            }
        }
        native::sync_directory(&self.area.directory)?;
        Ok(())
    }
    pub(super) fn verify(&self) -> Result<()> {
        self.area.check()?;
        let mut found = 0usize;
        self.verify_directory(&self.area.directory, "", &mut found)?;
        if found != self.members.len() || found != self.targets.len() {
            return Err(Error::Archive("missing_expanded_member"));
        }
        self.area.check()
    }
    fn verify_directory(&self, directory: &Dir, prefix: &str, found: &mut usize) -> Result<()> {
        for entry in directory.entries()? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or(Error::Archive("expanded_path_encoding"))?;
            let path = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            let member = self
                .members
                .get(&path)
                .ok_or(Error::Archive("unexpected_expanded_member"))?;
            let pinned = self.targets.get(&path).ok_or(Error::Staging)?;
            let original_id = pinned.identity()?;
            if native::leaf_identity(directory, name)? != original_id {
                return Err(Error::Staging);
            }
            *found += 1;
            if *found > self.members.len() {
                return Err(Error::Staging);
            }
            match &member.entry {
                ArchiveEntry::Directory => {
                    let child = pinned.directory()?;
                    let meta = child.dir_metadata()?;
                    if meta.uid() != 0 || meta.mode() & 0o7777 != 0o555 {
                        return Err(Error::Staging);
                    }
                    self.verify_directory(child, &path, found)?;
                }
                ArchiveEntry::File {
                    bytes,
                    sha256,
                    executable,
                } => {
                    let mut file = pinned.file()?;
                    let meta = file.metadata()?;
                    if meta.uid() != 0
                        || meta.len() != *bytes
                        || meta.nlink() != 1
                        || meta.mode() & 0o7777 != if *executable { 0o555 } else { 0o444 }
                    {
                        return Err(Error::Staging);
                    }
                    file.seek(SeekFrom::Start(0))?;
                    let mut hash = Sha256::new();
                    let count =
                        std::io::copy(&mut Read::by_ref(&mut file).take(bytes + 1), &mut hash)?;
                    if count != *bytes
                        || hex(&hash.finalize()) != *sha256
                        || native::file_identity(file)? != original_id
                    {
                        return Err(Error::Archive("expanded_bytes"));
                    }
                    file.seek(SeekFrom::Start(0))?;
                }
                ArchiveEntry::Symlink { target } => {
                    if native::read_pinned_link(pinned.link()?)? != *target {
                        return Err(Error::Staging);
                    }
                }
            }
            if native::leaf_identity(directory, name)? != original_id
                || pinned.identity()? != original_id
            {
                return Err(Error::Staging);
            }
        }
        Ok(())
    }
}

fn parent<'a, 'p>(
    area: &'a StagingArea,
    targets: &'a BTreeMap<String, PinnedTarget>,
    path: &'p str,
) -> Result<(&'a Dir, &'p str)> {
    area.check()?;
    let mut directory = &area.directory;
    let mut parts = path.rsplitn(2, '/');
    let leaf = parts.next().ok_or(Error::Staging)?;
    if let Some(parent) = parts.next() {
        let mut prefix = String::new();
        for part in parent.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let child = targets.get(&prefix).ok_or(Error::Staging)?.directory()?;
            if native::leaf_identity(directory, part)? != native::directory_identity(child)? {
                return Err(Error::Staging);
            }
            directory = child;
        }
    }
    Ok((directory, leaf))
}
