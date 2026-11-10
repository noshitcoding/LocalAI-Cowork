//! Reclamation only from an original completed task's consumed native volumes.
//! Published fences stay permanent. No saved record can reopen an input volume.
use super::*;
use crate::containerd::tasks::CompletedVolumeRetirementSeal;

/// Original process-local namespace ownership survives partial cleanup. None
/// of these handles or native identities is a portable/public wire contract.
pub(crate) struct RetiringPublishedVolume {
    pool: Arc<Inner>,
    namespace: Dir,
    record: Record,
    retired: Option<RetiredPublishedVolume>,
    collection: Option<MetadataCollection>,
    collected: bool,
}
struct MetadataCollection {
    root: Option<Dir>,
    fence: Option<File>,
    journals: [Option<File>; 2],
}

/// Local evidence only, not authority. The service exports a digest of its
/// aggregate proof, never the native identities below.
#[derive(Clone, Serialize)]
pub(crate) struct RetiredPublishedVolume {
    pub(crate) volume_id: String,
    pub(crate) request_sha256: String,
    pub(crate) reserved_bytes: u64,
    pub(crate) reserved_inodes: u32,
    namespace: NativeId,
    image: NativeId,
    base: NativeId,
    unmounted: bool,
    loop_detached: bool,
    image_absent: bool,
}

impl PublishedVolume {
    /// The completed task first closes all independent workspace, temporary,
    /// snapshot and control handles. Consuming this volume closes its remaining
    /// mount/image/loop descriptors before any real unmount is attempted.
    pub(crate) fn into_retirement(
        self,
        seal: &CompletedVolumeRetirementSeal,
    ) -> std::result::Result<RetiringPublishedVolume, Box<(VolumeError, Self)>> {
        let checked = (|| -> Result<RetiringPublishedVolume> {
            let pool = self.inner.pool.clone();
            let _guard = lock(&pool)?;
            self.inner.usage()?;
            let record = read_record(&self.inner.namespace)?;
            require_same(&self.inner.record, &record)?;
            if record.phase != Phase::Published
                || !seal.allows(&record.id, &record.request.request_sha256)
            {
                return Err(VolumeError::Uncertain);
            }
            Ok(RetiringPublishedVolume {
                pool: pool.clone(),
                namespace: self.inner.namespace.try_clone()?,
                record,
                retired: None,
                collection: None,
                collected: false,
            })
        })();
        match checked {
            Ok(retiring) => {
                drop(self);
                Ok(retiring)
            }
            Err(error) => Err(Box::new((error, self))),
        }
    }
}

impl RetiringPublishedVolume {
    /// Each uncertain step keeps its reservation. Repetition uses only this
    /// original capability and native observations, never an execution replay.
    pub(crate) fn retire(&mut self) -> Result<RetiredPublishedVolume> {
        if let Some(original) = &self.retired {
            return Ok(original.clone());
        }
        let pool = self.pool.clone();
        let _guard = lock(&pool)?;
        let mut current = read_record(&self.namespace)?;
        require_same(&self.record, &current)?;
        match current.phase {
            Phase::Published => {
                current.phase = Phase::PublishedRetiring;
                replace_json(&self.namespace, &mut current)?;
            }
            Phase::PublishedRetiring | Phase::PublishedDrained | Phase::PublishedRetired => {}
            _ => return Err(VolumeError::Uncertain),
        }
        self.record = current.clone();
        drain(self.namespace.try_clone()?, current, true)?;
        let current = read_record(&self.namespace)?;
        require_same(&self.record, &current)?;
        if current.phase != Phase::PublishedRetired {
            return Err(VolumeError::Uncertain);
        }
        match self.namespace.symlink_metadata("image.ext4") {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            _ => return Err(VolumeError::Uncertain),
        }
        let base = self.namespace.open_dir_nofollow("root")?;
        if NativeId::directory(&base)? != current.base
            || mount_id(&base)? != mount_id(&self.namespace)?
        {
            return Err(VolumeError::StillAttached);
        }
        self.record = current;
        // drain checked loop detachment while it still held the original image
        // inode open, before unlink. After deletion that inode may be reused by
        // another object; never interpret its later number as our old image.
        let proof = RetiredPublishedVolume {
            volume_id: self.record.id.clone(),
            request_sha256: self.record.request.request_sha256.clone(),
            reserved_bytes: self.record.request.bytes,
            reserved_inodes: self.record.request.inodes,
            namespace: self.record.namespace.clone(),
            image: self.record.image.clone(),
            base: self.record.base.clone(),
            unmounted: true,
            loop_detached: true,
            image_absent: true,
        };
        self.retired = Some(proof.clone());
        Ok(proof)
    }

    /// Call only after the delivery journal has durably stored its final local
    /// evidence. This removes exactly our retired, empty original namespace;
    /// no recursive path deletion or mount adoption is available here.
    pub(crate) fn collect_metadata(&mut self) -> Result<()> {
        if self.collected {
            return Ok(());
        }
        let pool = self.pool.clone();
        let _guard = lock(&pool)?;
        if self.collection.is_none() {
            let current = read_record(&self.namespace)?;
            require_same(&self.record, &current)?;
            let named = pool.volumes.open_dir_nofollow(&current.id)?;
            if self.retired.is_none()
                || current.phase != Phase::PublishedRetired
                || NativeId::directory(&named)? != current.namespace
                || NativeId::directory(&self.namespace)? != current.namespace
            {
                return Err(VolumeError::Identity);
            }
            match self.namespace.symlink_metadata("image.ext4") {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                _ => return Err(VolumeError::Uncertain),
            }
            let root = self.namespace.open_dir_nofollow("root")?;
            if NativeId::directory(&root)? != current.base
                || mount_id(&root)? != mount_id(&self.namespace)?
                || root.entries()?.next().is_some()
            {
                return Err(VolumeError::StillAttached);
            }
            let fence = open_file(&self.namespace, "publish.fence", false)?;
            let journals = [
                open_file(&self.namespace, "volume.a", true)?,
                open_file(&self.namespace, "volume.b", true)?,
            ];
            for (file, expected) in journals.iter().zip(&current.journal) {
                if NativeId::file(file)? != *expected {
                    return Err(VolumeError::Identity);
                }
            }
            // Pin every original object before the first unlink. A partial
            // namespace no longer satisfies read_record; this live state does.
            self.collection = Some(MetadataCollection {
                root: Some(root),
                fence: Some(fence),
                journals: journals.map(Some),
            });
        }
        match pool.volumes.open_dir_nofollow(&self.record.id) {
            Ok(named) if NativeId::directory(&named)? == self.record.namespace => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let state = self.collection.as_ref().ok_or(VolumeError::Uncertain)?;
                if state.root.is_some()
                    || state.fence.is_some()
                    || state.journals.iter().any(Option::is_some)
                {
                    return Err(VolumeError::Uncertain);
                }
                sync(&pool.volumes)?;
                self.collected = true;
                return Ok(());
            }
            _ => return Err(VolumeError::Identity),
        }
        let state = self.collection.as_mut().ok_or(VolumeError::Uncertain)?;
        remove_original_root(&self.namespace, &mut state.root, &self.record.base)?;
        remove_original_file(&self.namespace, "publish.fence", &mut state.fence)?;
        remove_original_file(&self.namespace, "volume.a", &mut state.journals[0])?;
        remove_original_file(&self.namespace, "volume.b", &mut state.journals[1])?;
        if self.namespace.entries()?.next().is_some() {
            return Err(VolumeError::Identity);
        }
        if NativeId::directory(&pool.volumes.open_dir_nofollow(&self.record.id)?)?
            != self.record.namespace
        {
            return Err(VolumeError::Identity);
        }
        pool.volumes.remove_dir(&self.record.id)?;
        sync(&pool.volumes)?;
        self.collected = true;
        Ok(())
    }
}

fn remove_original_root(
    namespace: &Dir,
    original: &mut Option<Dir>,
    expected: &NativeId,
) -> Result<()> {
    match namespace.symlink_metadata("root") {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            original.take();
            sync(namespace)?;
            return Ok(());
        }
        Ok(_) => {}
        Err(error) => return Err(error.into()),
    }
    let pinned = original.as_ref().ok_or(VolumeError::Identity)?;
    let current = namespace.open_dir_nofollow("root")?;
    if NativeId::directory(pinned)? != *expected
        || NativeId::directory(&current)? != *expected
        || mount_id(&current)? != mount_id(namespace)?
        || current.entries()?.next().is_some()
    {
        return Err(VolumeError::Identity);
    }
    namespace.remove_dir("root")?;
    sync(namespace)?;
    original.take();
    Ok(())
}
fn remove_original_file(namespace: &Dir, name: &str, original: &mut Option<File>) -> Result<()> {
    match namespace.symlink_metadata(name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            original.take();
            sync(namespace)?;
            return Ok(());
        }
        Ok(_) => {}
        Err(error) => return Err(error.into()),
    }
    let pinned = original.as_ref().ok_or(VolumeError::Identity)?;
    if NativeId::file(&open_file(namespace, name, true)?)? != NativeId::file(pinned)? {
        return Err(VolumeError::Identity);
    }
    namespace.remove_file(name)?;
    sync(namespace)?;
    original.take();
    Ok(())
}

#[cfg(test)]
#[path = "published_retirement_tests.rs"]
mod tests;
