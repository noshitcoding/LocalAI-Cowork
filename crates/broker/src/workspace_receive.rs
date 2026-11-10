//! Private, resumable snapshot storage. This is not an authorization endpoint.
//!
//! A trusted runner must supply an already authorized plan/recipient binding and
//! private directories on reserved, bounded volumes. Neither a digest nor this
//! local journal grants execution rights. Workloads never receive this directory.
use super::*;
use cowork_domain::workspace_transfer as transfer;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Seek, SeekFrom},
};

const INTENT: &str = "intent.json";
const PACK: &str = "payload.pack";
const PROGRESS: &str = "progress.bits";
const LOCK: &str = "transfer.lock";
const MAX_INTENT_BYTES: u64 = transfer::MAX_MANIFEST_BYTES as u64 + 4096;

/// Compared with the caller's independently resolved authority on every reopen.
/// Constructing this value does not authenticate any of its fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferBinding {
    pub plan_id: String,
    pub plan_sha256: String,
    pub recipient_id: String,
    pub recipient_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeObject {
    volume: u64,
    id: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    format_version: u32,
    binding: TransferBinding,
    manifest: WorkspaceTransferManifest,
    directory: NativeObject,
    payload: NativeObject,
    progress: NativeObject,
    lock: NativeObject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkReceipt {
    pub receipt_sha256: String,
    pub path: String,
    pub offset: u64,
    pub size: u64,
    pub sha256: String,
}

/// Owns only transfer storage. It has no project database or execution authority.
pub struct WorkspaceReceiver {
    directory: Dir,
    intent: Intent,
}

/// Holds the receiver lock and verified payload descriptor until materialization
/// completes. No path or unverified stage can be substituted for this capability.
pub struct VerifiedTransfer<'a> {
    receiver: &'a WorkspaceReceiver,
    payload: File,
    _lock: ResourceLock,
}

/// Full verification with owned native descriptors, suitable for a separate
/// controller SQL commit without blocking an async worker on the file scan.
pub struct OwnedVerifiedTransfer {
    receiver: Arc<WorkspaceReceiver>,
    payload: File,
    _lock: ResourceLock,
}

/// Private completed tree, ready for the runner's separate admission check.
pub struct ReceivedWorkspace {
    directory: Dir,
    manifest_sha256: String,
}
impl ReceivedWorkspace {
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
    pub fn into_directory(self) -> Dir {
        self.directory
    }
}

impl WorkspaceReceiver {
    /// The caller must first reserve and secure this empty private directory.
    /// A partial create is deliberately not adopted without a complete intent.
    pub fn create(
        directory: Dir,
        binding: TransferBinding,
        manifest: WorkspaceTransferManifest,
    ) -> Result<Self> {
        validate_binding(&binding)?;
        transfer::validate(&manifest)?;
        let root = directory_identity(&directory)?;
        let lock_file = create_file(&directory, LOCK)?;
        let lock_id = file_identity(&lock_file)?;
        let _lock = acquire(lock_file)?;
        require_entries(&directory, &[LOCK])?;
        let payload = create_file(&directory, PACK)?;
        allocate(&payload, manifest.total_bytes)?;
        let progress = create_file(&directory, PROGRESS)?;
        let progress_len = chunk_count(&manifest).div_ceil(8);
        (&progress)
            .write_all(&vec![0; progress_len])
            .map_err(storage_error)?;
        progress.sync_all().map_err(storage_error)?;
        let intent = Intent {
            format_version: 1,
            binding,
            manifest,
            directory: root,
            payload: file_identity(&payload)?,
            progress: file_identity(&progress)?,
            lock: lock_id,
        };
        let bytes = serde_json::to_vec(&intent).map_err(storage_error)?;
        if bytes.len() as u64 > MAX_INTENT_BYTES {
            return Err(Error::Forbidden);
        }
        let mut file = create_file(&directory, INTENT)?;
        file.write_all(&bytes).map_err(storage_error)?;
        file.sync_all().map_err(storage_error)?;
        sync(&directory)?;
        Ok(Self { directory, intent })
    }

    /// Expected binding and manifest come from the trusted local plan resolver,
    /// never from values discovered in the received journal itself.
    pub fn reopen(
        directory: Dir,
        binding: &TransferBinding,
        manifest: &WorkspaceTransferManifest,
    ) -> Result<Self> {
        validate_binding(binding)?;
        transfer::validate(manifest)?;
        let intent = read_intent(&directory)?;
        if intent.format_version != 1 || &intent.binding != binding || &intent.manifest != manifest
        {
            return Err(Error::Forbidden);
        }
        let receiver = Self { directory, intent };
        let (_lock, _payload, _progress) = receiver.locked()?;
        Ok(receiver)
    }

    /// Returns only declared chunks that have not been durably acknowledged.
    /// Marked chunks are rehashed; corrupted confirmed content is an error.
    pub fn missing_chunks(&self) -> Result<Vec<ReadWorkspaceTransferChunk>> {
        let (_lock, mut payload, mut progress) = self.locked()?;
        let bits = read_bits(&mut progress, chunk_count(&self.intent.manifest))?;
        let mut missing = Vec::new();
        let mut index = 0;
        let mut base = 0;
        for file in &self.intent.manifest.files {
            for chunk in &file.chunks {
                if marked(&bits, index) {
                    verify_chunk(&mut payload, base + chunk.offset, chunk)?;
                } else {
                    missing.push(ReadWorkspaceTransferChunk {
                        path: file.path.clone(),
                        offset: chunk.offset,
                        expected_sha256: chunk.sha256.clone(),
                    });
                }
                index += 1;
            }
            base += file.size;
        }
        Ok(missing)
    }

    /// Out-of-order writes and lost-reply replay return the same receipt. An
    /// acknowledged block is never overwritten, including after corruption.
    pub fn accept_chunk(
        &self,
        request: &ReadWorkspaceTransferChunk,
        bytes: &[u8],
    ) -> Result<ChunkReceipt> {
        let (chunk, index, pack_offset) = self.locate(request)?;
        if bytes.len() as u64 != chunk.size || hash(bytes) != chunk.sha256 {
            return Err(Error::FileConflict);
        }
        let (_lock, mut payload, mut progress) = self.locked()?;
        let bits = read_bits(&mut progress, chunk_count(&self.intent.manifest))?;
        if marked(&bits, index) {
            verify_chunk(&mut payload, pack_offset, chunk)?;
        } else {
            payload
                .seek(SeekFrom::Start(pack_offset))
                .map_err(storage_error)?;
            payload.write_all(bytes).map_err(storage_error)?;
            payload.sync_all().map_err(storage_error)?;
            // Persist the acknowledgement only after the payload is durable.
            progress
                .seek(SeekFrom::Start((index / 8) as u64))
                .map_err(storage_error)?;
            progress
                .write_all(&[bits[index / 8] | (1 << (index % 8))])
                .map_err(storage_error)?;
            progress.sync_all().map_err(storage_error)?;
        }
        let receipt_sha256 = canonical_hash(
            &json!({"format_version":1,"binding":self.intent.binding,
            "manifest_sha256":self.intent.manifest.sha256,"path":request.path,"offset":chunk.offset,"size":chunk.size,"sha256":chunk.sha256}),
        );
        Ok(ChunkReceipt {
            receipt_sha256,
            path: request.path.clone(),
            offset: chunk.offset,
            size: chunk.size,
            sha256: chunk.sha256.clone(),
        })
    }

    /// Rechecks every block and complete-file hash. The progress map is never
    /// sufficient evidence that an input is ready for dispatch.
    pub fn verify(&self) -> Result<VerifiedTransfer<'_>> {
        let (lock, mut payload, mut progress) = self.locked()?;
        let count = chunk_count(&self.intent.manifest);
        let bits = read_bits(&mut progress, count)?;
        if (0..count).any(|i| !marked(&bits, i)) {
            return Err(Error::Validation("workspace_transfer_incomplete".into()));
        }
        let mut base = 0;
        for file in &self.intent.manifest.files {
            let mut digest = Sha256::new();
            for chunk in &file.chunks {
                digest.update(verify_chunk(&mut payload, base + chunk.offset, chunk)?);
            }
            if format!("{:x}", digest.finalize()) != file.sha256 {
                return Err(Error::FileConflict);
            }
            base += file.size;
        }
        Ok(VerifiedTransfer {
            receiver: self,
            payload,
            _lock: lock,
        })
    }

    pub fn verify_owned(self: Arc<Self>) -> Result<OwnedVerifiedTransfer> {
        let (payload, lock) = {
            let verified = self.verify()?;
            (verified.payload, verified._lock)
        };
        Ok(OwnedVerifiedTransfer {
            receiver: self,
            payload,
            _lock: lock,
        })
    }

    fn locate(
        &self,
        request: &ReadWorkspaceTransferChunk,
    ) -> Result<(&WorkspaceTransferChunk, usize, u64)> {
        transfer::validate_path(&request.path)?;
        let mut base = 0;
        let mut index = 0;
        for file in &self.intent.manifest.files {
            if file.path == request.path {
                for chunk in &file.chunks {
                    if chunk.offset == request.offset && chunk.sha256 == request.expected_sha256 {
                        return Ok((chunk, index, base + chunk.offset));
                    }
                    index += 1;
                }
                return Err(Error::Forbidden);
            }
            index += file.chunks.len();
            base += file.size;
        }
        Err(Error::Forbidden)
    }

    fn locked(&self) -> Result<(ResourceLock, File, File)> {
        if directory_identity(&self.directory)? != self.intent.directory {
            return Err(Error::FileConflict);
        }
        let lock_file = open_file(&self.directory, LOCK, true)?;
        if file_identity(&lock_file)? != self.intent.lock {
            return Err(Error::FileConflict);
        }
        let lock = acquire(lock_file)?;
        require_entries(&self.directory, &[INTENT, PACK, PROGRESS, LOCK])?;
        if read_intent(&self.directory)? != self.intent {
            return Err(Error::FileConflict);
        }
        let payload = open_file(&self.directory, PACK, true)?;
        let progress = open_file(&self.directory, PROGRESS, true)?;
        if file_identity(&payload)? != self.intent.payload
            || file_identity(&progress)? != self.intent.progress
            || payload.metadata().map_err(storage_error)?.len() != self.intent.manifest.total_bytes
        {
            return Err(Error::FileConflict);
        }
        check_allocation(&payload, self.intent.manifest.total_bytes)?;
        Ok((lock, payload, progress))
    }
}

impl VerifiedTransfer<'_> {
    /// Read only a declared block from the already verified private pack. The
    /// original payload descriptor/lock remain held and the block is rehashed.
    pub fn read_chunk(&mut self, request: &ReadWorkspaceTransferChunk) -> Result<Vec<u8>> {
        let (chunk, _, offset) = self.receiver.locate(request)?;
        verify_chunk(&mut self.payload, offset, chunk)
    }
    /// Writes only to an empty private working-volume capability supplied by the
    /// trusted runtime. A failed/partial tree is never returned or adopted.
    /// Destination reservation, lifetime and later execution admission belong to
    /// that runtime; preallocation here does not replace its filesystem limit.
    pub fn materialize(mut self, destination: Dir) -> Result<ReceivedWorkspace> {
        directory_identity(&destination)?;
        require_entries(&destination, &[])?;
        let manifest = &self.receiver.intent.manifest;
        for path in &manifest.directories {
            let (parent, name) = parent(&destination, path)?;
            parent.create_dir(name).map_err(storage_error)?;
            #[cfg(unix)]
            {
                use cap_std::fs::PermissionsExt;
                parent
                    .set_permissions(name, cap_std::fs::Permissions::from_mode(0o700))
                    .map_err(storage_error)?;
            }
            sync(&parent)?;
        }
        let mut base = 0;
        for input in &manifest.files {
            let (parent, name) = parent(&destination, &input.path)?;
            let mut file = create_file(&parent, name)?;
            allocate(&file, input.size)?;
            let mut digest = Sha256::new();
            for chunk in &input.chunks {
                let bytes = verify_chunk(&mut self.payload, base + chunk.offset, chunk)?;
                file.write_all(&bytes).map_err(storage_error)?;
                digest.update(bytes);
            }
            if format!("{:x}", digest.finalize()) != input.sha256 {
                return Err(Error::FileConflict);
            }
            file.sync_all().map_err(storage_error)?;
            sync(&parent)?;
            base += input.size;
        }
        sync(&destination)?;
        Ok(ReceivedWorkspace {
            directory: destination,
            manifest_sha256: manifest.sha256.clone(),
        })
    }
}

impl OwnedVerifiedTransfer {
    pub fn read_chunk(&mut self, request: &ReadWorkspaceTransferChunk) -> Result<Vec<u8>> {
        let (chunk, _, offset) = self.receiver.locate(request)?;
        verify_chunk(&mut self.payload, offset, chunk)
    }
}

fn validate_binding(binding: &TransferBinding) -> Result<()> {
    for label in [&binding.plan_id, &binding.recipient_id] {
        if label.is_empty() || label.len() > 200 || label.chars().any(char::is_control) {
            return Err(Error::Forbidden);
        }
    }
    if !transfer::valid_sha256(&binding.plan_sha256) || binding.recipient_generation == 0 {
        return Err(Error::Forbidden);
    }
    Ok(())
}
fn chunk_count(manifest: &WorkspaceTransferManifest) -> usize {
    manifest.files.iter().map(|f| f.chunks.len()).sum()
}
fn marked(bits: &[u8], index: usize) -> bool {
    bits[index / 8] & (1 << (index % 8)) != 0
}
fn read_bits(file: &mut File, count: usize) -> Result<Vec<u8>> {
    let len = count.div_ceil(8);
    if file.metadata().map_err(storage_error)?.len() != len as u64 {
        return Err(Error::FileConflict);
    }
    file.rewind().map_err(storage_error)?;
    let mut bits = vec![0; len];
    file.read_exact(&mut bits).map_err(storage_error)?;
    if !count.is_multiple_of(8) && bits.last().is_some_and(|v| *v >> (count % 8) != 0) {
        return Err(Error::FileConflict);
    }
    Ok(bits)
}
fn verify_chunk(file: &mut File, offset: u64, chunk: &WorkspaceTransferChunk) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset)).map_err(storage_error)?;
    let mut bytes = vec![0; chunk.size as usize];
    file.read_exact(&mut bytes).map_err(storage_error)?;
    if hash(&bytes) != chunk.sha256 {
        return Err(Error::FileConflict);
    }
    Ok(bytes)
}
fn read_intent(directory: &Dir) -> Result<Intent> {
    let bytes = read_regular_limit(directory, INTENT, MAX_INTENT_BYTES)?.ok_or(Error::NotFound)?;
    serde_json::from_slice(&bytes).map_err(storage_error)
}
fn create_file(directory: &Dir, name: &str) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NONBLOCK);
    }
    let file = directory
        .open_with(name, &options)
        .map_err(storage_error)?
        .into_std();
    file_identity(&file)?;
    Ok(file)
}
fn open_file(directory: &Dir, name: &str, write: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(write).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = directory
        .open_with(name, &options)
        .map_err(storage_error)?
        .into_std();
    file_identity(&file)?;
    Ok(file)
}
fn acquire(file: File) -> Result<ResourceLock> {
    match file.try_lock() {
        Ok(()) => Ok(ResourceLock(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(Error::WorkspaceBusy),
        Err(std::fs::TryLockError::Error(e)) => Err(storage_error(e)),
    }
}
fn require_entries(directory: &Dir, allowed: &[&str]) -> Result<()> {
    let mut found = Vec::new();
    for entry in directory.entries().map_err(storage_error)? {
        let entry = entry.map_err(storage_error)?;
        let name = entry.file_name();
        if !allowed.iter().any(|v| name == *v) {
            return Err(Error::FileConflict);
        }
        found.push(name);
    }
    if found.len() != allowed.len() {
        return Err(Error::FileConflict);
    }
    Ok(())
}
fn parent<'a>(root: &Dir, path: &'a str) -> Result<(Dir, &'a str)> {
    let (directory, name) = path.rsplit_once('/').unwrap_or(("", path));
    Ok((open_directory(root, Path::new(directory))?, name))
}
fn sync(directory: &Dir) -> Result<()> {
    #[cfg(unix)]
    {
        sync_directory(directory)?;
    }
    // Windows file data is flushed above. Crash durability of directory metadata
    // still needs the managed-runtime volume/platform qualification; no claim of
    // a Windows host filesystem transaction is made here.
    let _ = directory;
    Ok(())
}

#[cfg(unix)]
pub(super) fn file_identity(file: &File) -> Result<NativeObject> {
    let meta = file.metadata().map_err(storage_error)?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(Error::Forbidden);
    }
    Ok(NativeObject {
        volume: meta.dev(),
        id: meta.ino().to_le_bytes().to_vec(),
    })
}
#[cfg(unix)]
pub(super) fn directory_identity(directory: &Dir) -> Result<NativeObject> {
    let meta = directory.dir_metadata().map_err(storage_error)?;
    if !meta.is_dir() || meta.is_symlink() {
        return Err(Error::Forbidden);
    }
    Ok(NativeObject {
        volume: meta.dev(),
        id: meta.ino().to_le_bytes().to_vec(),
    })
}
#[cfg(windows)]
fn native_identity(
    handle: windows_sys::Win32::Foundation::HANDLE,
    directory: bool,
) -> Result<NativeObject> {
    use windows_sys::Win32::Storage::FileSystem::*;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let mut id = FILE_ID_INFO::default();
    // SAFETY: borrowed descriptor remains live; output structs have SDK layout.
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0
        || unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                (&mut id as *mut FILE_ID_INFO).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
    {
        return Err(storage_error(std::io::Error::last_os_error()));
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
        || (!directory
            && (info.nNumberOfLinks != 1
                || info.dwFileAttributes
                    & (FILE_ATTRIBUTE_SPARSE_FILE | FILE_ATTRIBUTE_COMPRESSED)
                    != 0))
    {
        return Err(Error::Forbidden);
    }
    Ok(NativeObject {
        volume: id.VolumeSerialNumber,
        id: id.FileId.Identifier.to_vec(),
    })
}
#[cfg(windows)]
pub(super) fn file_identity(file: &File) -> Result<NativeObject> {
    use std::os::windows::io::AsRawHandle;
    native_identity(file.as_raw_handle(), false)
}
#[cfg(windows)]
pub(super) fn directory_identity(directory: &Dir) -> Result<NativeObject> {
    use std::os::windows::io::AsRawHandle;
    native_identity(directory.as_raw_handle(), true)
}
#[cfg(target_os = "linux")]
fn allocate(file: &File, bytes: u64) -> Result<()> {
    use std::os::fd::AsRawFd;
    if bytes > 0 {
        // No glibc emulation or sparse set_len fallback on unsupported storage.
        if unsafe { libc::fallocate(file.as_raw_fd(), 0, 0, bytes as libc::off_t) } != 0 {
            return Err(storage_error(std::io::Error::last_os_error()));
        }
    }
    file.set_len(bytes).map_err(storage_error)?;
    file.sync_all().map_err(storage_error)?;
    check_allocation(file, bytes)
}
#[cfg(target_os = "linux")]
fn check_allocation(file: &File, bytes: u64) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata().map_err(storage_error)?;
    if meta.len() != bytes || meta.blocks().saturating_mul(512) < bytes {
        return Err(Error::Forbidden);
    }
    Ok(())
}
#[cfg(windows)]
fn allocate(file: &File, bytes: u64) -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::*;
    let info = FILE_ALLOCATION_INFO {
        AllocationSize: bytes as i64,
    };
    // SAFETY: live writable handle and correctly sized SDK allocation structure.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileAllocationInfo,
            (&info as *const FILE_ALLOCATION_INFO).cast(),
            std::mem::size_of::<FILE_ALLOCATION_INFO>() as u32,
        )
    } == 0
    {
        return Err(storage_error(std::io::Error::last_os_error()));
    }
    file.set_len(bytes).map_err(storage_error)?;
    file.sync_all().map_err(storage_error)?;
    check_allocation(file, bytes)
}
#[cfg(windows)]
fn check_allocation(file: &File, bytes: u64) -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::*;
    file_identity(file)?;
    let mut info = FILE_STANDARD_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStandardInfo,
            (&mut info as *mut FILE_STANDARD_INFO).cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    } == 0
    {
        return Err(storage_error(std::io::Error::last_os_error()));
    }
    if info.EndOfFile != bytes as i64 || info.AllocationSize < bytes as i64 {
        return Err(Error::Forbidden);
    }
    Ok(())
}
#[cfg(not(any(target_os = "linux", windows)))]
fn allocate(_: &File, _: u64) -> Result<()> {
    Err(Error::Forbidden)
}
#[cfg(not(any(target_os = "linux", windows)))]
fn check_allocation(_: &File, _: u64) -> Result<()> {
    Err(Error::Forbidden)
}
