use super::*;

/// Explicit per-extraction limit. The archive format still supports 200,000
/// entries; actual admission also needs enough OS descriptors. No implicit
/// RLIMIT, systemd, WSL or global configuration changes occur here.
pub struct TargetDescriptorPolicy {
    max_objects: usize,
}
impl TargetDescriptorPolicy {
    pub fn new(max_objects: usize) -> Result<Self> {
        if max_objects == 0 || max_objects > MAX_ENTRIES {
            return Err(Error::Archive("target_descriptor_policy"));
        }
        Ok(Self { max_objects })
    }
}

pub(super) enum PinnedTarget {
    Directory(Dir),
    File(File),
    Symlink(File),
}
impl PinnedTarget {
    pub(super) fn identity(&self) -> Result<native::Identity> {
        match self {
            Self::Directory(dir) => native::directory_identity(dir),
            Self::File(file) => native::file_identity(file),
            Self::Symlink(link) => native::link_identity(link),
        }
    }
    pub(super) fn directory(&self) -> Result<&Dir> {
        match self {
            Self::Directory(dir) => Ok(dir),
            _ => Err(Error::Staging),
        }
    }
    pub(super) fn file(&self) -> Result<&File> {
        match self {
            Self::File(file) => Ok(file),
            _ => Err(Error::Staging),
        }
    }
    pub(super) fn file_mut(&mut self) -> Result<&mut File> {
        match self {
            Self::File(file) => Ok(file),
            _ => Err(Error::Staging),
        }
    }
    pub(super) fn link(&self) -> Result<&File> {
        match self {
            Self::Symlink(link) => Ok(link),
            _ => Err(Error::Staging),
        }
    }
}

pub(in crate::staging::archive) struct DescriptorReservation {
    slots: Vec<Dir>,
}
impl DescriptorReservation {
    pub(super) fn reserve(
        root: &Dir,
        objects: usize,
        depth: usize,
        policy: TargetDescriptorPolicy,
    ) -> Result<Self> {
        Self::reserve_with_extra(root, objects, depth, 0, policy)
    }
    pub(in crate::staging::archive) fn reserve_with_extra(
        root: &Dir,
        objects: usize,
        depth: usize,
        extra: usize,
        policy: TargetDescriptorPolicy,
    ) -> Result<Self> {
        if objects > policy.max_objects {
            return Err(Error::Archive("target_descriptor_budget"));
        }
        // Locked cap-primitives 4.0.3 ReadDirInner holds both its OwnedFd and
        // Dir::read_from's duplicate. Recursive verification retains every
        // active iterator. Include the target root, then fixed helper space.
        let headroom = depth
            .checked_add(1)
            .and_then(|levels| levels.checked_mul(2))
            .and_then(|iterators| iterators.checked_add(32))
            .and_then(|helpers| helpers.checked_add(extra))
            .ok_or(Error::Archive("target_descriptor_budget"))?;
        let mut slots = Vec::with_capacity(objects);
        let mut temporary = Vec::with_capacity(headroom);
        // Actual close-on-exec descriptor reservations, not a stale /proc count
        // or an assertion that a numeric soft limit implies available slots.
        for _ in 0..objects {
            slots.push(reserve_one(root)?);
        }
        for _ in 0..headroom {
            temporary.push(reserve_one(root)?);
        }
        // Keep member slots until each original inode is opened. Release the
        // separately verified temporary headroom before operations need it.
        drop(temporary);
        Ok(Self { slots })
    }
    pub(super) fn release_one(&mut self) -> Result<()> {
        drop(
            self.slots
                .pop()
                .ok_or(Error::Archive("target_descriptor_budget"))?,
        );
        Ok(())
    }
    pub(in crate::staging::archive) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}
fn reserve_one(root: &Dir) -> Result<Dir> {
    root.try_clone().map_err(|error| {
        if matches!(error.raw_os_error(), Some(libc::EMFILE | libc::ENFILE)) {
            Error::Archive("target_descriptor_budget")
        } else {
            error.into()
        }
    })
}
