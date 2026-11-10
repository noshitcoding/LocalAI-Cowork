//! Portable output bytes from a proven physically stopped task. All traversal
//! stays beneath its retained native workspace. No caller path selects a host
//! file, and neither the manifest nor these reads authorize original-file writes.
use super::{gvisor::Stamp, tasks::ManagedTaskOutcome, *};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::Dir;
use cowork_contracts::{
    ReadWorkspaceTransferChunk, WorkspaceTransferChunk, WorkspaceTransferFile,
    WorkspaceTransferManifest,
};
use cowork_domain::workspace_transfer as transfer;
use std::{
    collections::BTreeMap,
    os::{fd::AsRawFd, unix::fs::FileExt},
};

pub struct ManagedOutputSnapshot {
    physical: Arc<ManagedTaskOutcome>,
    root: Dir,
    manifest: WorkspaceTransferManifest,
    stamps: BTreeMap<String, Stamp>,
}
impl ManagedOutputSnapshot {
    /// Publish the producer's single immutable provenance, never per-reader
    /// rights. The caller signs with the actual retained runner key. Retention
    /// starts at original completion and cannot be renewed by repeated reads.
    pub fn retain_attestation(
        &self,
        launch: &cowork_runner_protocol::Signed<cowork_runner_protocol::LaunchRequest>,
        receipt: &cowork_runner_protocol::Signed<cowork_runner_protocol::Receipt>,
        attestation: &cowork_runner_protocol::Signed<
            cowork_runner_protocol::ManagedOutputAttestation,
        >,
    ) -> Result<()> {
        self.verify()?;
        let grant = self.physical.input_grant();
        launch
            .verify("launch", &grant.runner.control_public_key)
            .map_err(|_| Error::Output)?;
        attestation
            .verify("managed_output", &grant.runner.runner_public_key)
            .map_err(|_| Error::Output)?;
        if attestation.payload.produced_at != receipt.payload.updated_at {
            return Err(Error::Output);
        }
        attestation
            .payload
            .validate(
                grant,
                launch,
                receipt,
                &self.manifest,
                &self.physical.producer_instance_sha256(),
                attestation.payload.produced_at,
            )
            .map_err(|_| Error::Output)?;
        self.physical
            .persist_attested_snapshot(&self.manifest, &self.stamps, receipt, attestation)
    }
    pub(super) fn capture(
        physical: Arc<ManagedTaskOutcome>,
        maximum_entries: usize,
        receipt: &cowork_runner_protocol::Signed<cowork_runner_protocol::Receipt>,
    ) -> Result<Self> {
        // A larger valid filesystem inode ceiling does not make a small
        // portable result invalid. Limit actual scanned entries separately.
        let maximum_entries = maximum_entries.min(transfer::MAX_ENTRIES);
        if maximum_entries == 0 {
            return Err(Error::Output);
        }
        let root = physical.workspace()?;
        let dev = root.open(".")?.into_std().metadata()?.dev();
        let mut builder = Builder {
            maximum_entries,
            dev,
            directories: Vec::new(),
            files: Vec::new(),
            stamps: BTreeMap::new(),
            total_bytes: 0,
            entries: 0,
        };
        builder.directory(&root, "")?;
        builder.directories.sort();
        builder.files.sort_by(|a, b| a.path.cmp(&b.path));
        let manifest =
            transfer::build(builder.directories, builder.files).map_err(|_| Error::Output)?;
        let snapshot = Self {
            physical,
            root,
            manifest,
            stamps: builder.stamps,
        };
        snapshot.verify()?;
        snapshot
            .physical
            .persist_result_snapshot(&snapshot.manifest, &snapshot.stamps, receipt)?;
        Ok(snapshot)
    }
    pub fn manifest(&self) -> Result<WorkspaceTransferManifest> {
        self.verify()?;
        Ok(self.manifest.clone())
    }
    pub fn chunk(&self, input: &ReadWorkspaceTransferChunk) -> Result<Vec<u8>> {
        self.verify_root()?;
        transfer::validate_path(&input.path).map_err(|_| Error::Output)?;
        let file = self
            .manifest
            .files
            .iter()
            .find(|f| f.path == input.path)
            .ok_or(Error::Output)?;
        let chunk = file
            .chunks
            .iter()
            .find(|c| c.offset == input.offset && c.sha256 == input.expected_sha256)
            .ok_or(Error::Output)?;
        let (parent, name) = self.parent(&input.path)?;
        let source = open_file(&parent, name)?;
        self.require_stamp(&input.path, &source.metadata()?)?;
        let mut bytes = vec![0; usize::try_from(chunk.size).map_err(|_| Error::Output)?];
        source.read_exact_at(&mut bytes, chunk.offset)?;
        self.require_stamp(&input.path, &source.metadata()?)?;
        self.verify_root()?;
        if cowork_domain::hash(&bytes) != chunk.sha256 {
            return Err(Error::Output);
        }
        Ok(bytes)
    }
    fn verify(&self) -> Result<()> {
        self.verify_root()?;
        for path in self.stamps.keys().filter(|p| !p.is_empty()) {
            let (parent, name) = self.parent(path)?;
            let metadata = open_file(&parent, name)?.metadata()?;
            self.require_stamp(path, &metadata)?;
        }
        Ok(())
    }
    fn verify_root(&self) -> Result<()> {
        let current = self.physical.workspace()?;
        self.require_stamp("", &current.open(".")?.into_std().metadata()?)?;
        self.require_stamp("", &self.root.open(".")?.into_std().metadata()?)
    }
    fn require_stamp(&self, path: &str, metadata: &std::fs::Metadata) -> Result<()> {
        if self.stamps.get(path) != Some(&Stamp::of(metadata)) {
            return Err(Error::Output);
        }
        Ok(())
    }
    fn parent<'a>(&self, path: &'a str) -> Result<(Dir, &'a str)> {
        transfer::validate_path(path).map_err(|_| Error::Output)?;
        let mut components: Vec<_> = path.split('/').collect();
        let name = components.pop().ok_or(Error::Output)?;
        let mut directory = self.root.try_clone()?;
        let mut relative = String::new();
        for component in components {
            if !relative.is_empty() {
                relative.push('/');
            }
            relative.push_str(component);
            directory = directory.open_dir_nofollow(component)?;
            self.require_stamp(&relative, &directory.open(".")?.into_std().metadata()?)?;
        }
        Ok((directory, name))
    }
}
struct Builder {
    maximum_entries: usize,
    dev: u64,
    directories: Vec<String>,
    files: Vec<WorkspaceTransferFile>,
    stamps: BTreeMap<String, Stamp>,
    total_bytes: u64,
    entries: usize,
}
impl Builder {
    fn directory(&mut self, directory: &Dir, path: &str) -> Result<()> {
        let native = directory.open(".")?.into_std();
        let before = native.metadata()?;
        if !before.is_dir() || before.dev() != self.dev || before.uid() != 1000 {
            return Err(Error::Output);
        }
        let mut names = Vec::new();
        for entry in directory.entries()? {
            self.entries = self.entries.checked_add(1).ok_or(Error::Output)?;
            if self.entries > self.maximum_entries {
                return Err(Error::ResultExport {
                    reason: "result_portable_entry_limit_exceeded",
                });
            }
            let name = entry?
                .file_name()
                .into_string()
                .map_err(|_| Error::Output)?;
            names.push(name);
        }
        names.sort();
        for name in names {
            let relative = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}/{name}")
            };
            transfer::validate_path(&relative).map_err(|_| Error::ResultExport {
                reason: "result_portable_path_unsupported",
            })?;
            let metadata = directory.symlink_metadata(&name)?;
            if metadata.is_dir() {
                let child = directory.open_dir_nofollow(&name)?;
                self.directory(&child, &relative)?;
                self.directories.push(relative);
            } else if metadata.is_file() {
                self.file(directory, &name, &relative)?;
            } else {
                return Err(Error::ResultExport {
                    reason: if metadata.file_type().is_symlink() {
                        "result_symlink_unsupported"
                    } else {
                        "result_special_file_unsupported"
                    },
                });
            }
        }
        // Only a physically stopped workspace reaches this conversion. It
        // becomes root-owned/read-only before any portable output is published.
        readonly(&native, true)?;
        self.stamps
            .insert(path.into(), Stamp::of(&native.metadata()?));
        Ok(())
    }
    fn file(&mut self, parent: &Dir, name: &str, path: &str) -> Result<()> {
        let file = open_file(parent, name)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != 1000
            || metadata.dev() != self.dev
            || metadata.nlink() != 1
            || metadata.len() > transfer::MAX_FILE_BYTES
        {
            return Err(Error::Output);
        }
        self.total_bytes = self
            .total_bytes
            .checked_add(metadata.len())
            .ok_or(Error::Output)?;
        if self.total_bytes > transfer::MAX_TOTAL_BYTES {
            return Err(Error::Output);
        }
        let before = Stamp::of(&metadata);
        let mut full = Sha256::new();
        let mut chunks = Vec::new();
        let mut offset = 0;
        while offset < metadata.len() {
            let size = (metadata.len() - offset).min(transfer::CHUNK_BYTES);
            let mut bytes = vec![0; usize::try_from(size).map_err(|_| Error::Output)?];
            file.read_exact_at(&mut bytes, offset)?;
            full.update(&bytes);
            chunks.push(WorkspaceTransferChunk {
                offset,
                size,
                sha256: cowork_domain::hash(&bytes),
            });
            offset += size;
        }
        if Stamp::of(&file.metadata()?) != before {
            return Err(Error::Output);
        }
        readonly(&file, false)?;
        let sealed = Stamp::of(&file.metadata()?);
        if Stamp::of(&open_file(parent, name)?.metadata()?) != sealed {
            return Err(Error::Output);
        }
        self.stamps.insert(path.into(), sealed);
        self.files.push(WorkspaceTransferFile {
            path: path.into(),
            size: metadata.len(),
            sha256: format!("{:x}", full.finalize()),
            chunks,
        });
        Ok(())
    }
}
fn open_file(parent: &Dir, name: &str) -> Result<File> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    use cap_std::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    Ok(parent.open_with(name, &options)?.into_std())
}
fn readonly(file: &File, directory: bool) -> Result<()> {
    if unsafe { libc::fchown(file.as_raw_fd(), 0, 0) } < 0
        || unsafe { libc::fchmod(file.as_raw_fd(), if directory { 0o500 } else { 0o400 }) } < 0
    {
        return Err(io::Error::last_os_error().into());
    }
    file.sync_all()?;
    Ok(())
}
