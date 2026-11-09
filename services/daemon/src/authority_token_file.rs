//! Local process authorities are loaded only from their held private files.
//! File names, values and OS diagnostics never become startup error messages.
#[cfg(windows)]
use std::time::{Duration, Instant};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};
use zeroize::Zeroizing;

const MAX_BYTES: u64 = 128;
const UNAVAILABLE: &str = "Local authority files are invalid or unavailable";

pub(super) struct AuthorityTokens {
    daemon: Zeroizing<String>,
    native: Zeroizing<String>,
    _files: Vec<File>,
    _directory: Directory,
}
// Independent encrypted-team-key custody. Never exposed as an API authority.
pub(super) struct PrivateValue {
    value: Zeroizing<String>,
    _file: File,
    _directory: Directory,
}
impl PrivateValue {
    pub(super) fn value(&self) -> &str {
        &self.value
    }
}
impl AuthorityTokens {
    pub(super) fn daemon(&self) -> &str {
        &self.daemon
    }
    pub(super) fn native(&self) -> &str {
        &self.native
    }
}
struct Directory {
    path: PathBuf,
    file: File,
    _namespace: Vec<File>,
}
pub(super) struct PrivateDirectory(Directory);
impl PrivateDirectory {
    pub(super) fn path(&self) -> &Path {
        &self.0.path
    }
    // Native document startup retains this original custody; no renderer/API access.
    pub(super) fn original_directory(&self) -> &File {
        &self.0.file
    }
    pub(super) fn verify(&self) -> io::Result<()> {
        self.0.check_identity()
    }
    pub(super) fn child(&self, name: &str, create: bool) -> io::Result<File> {
        if !matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        ) || Path::new(name).components().count() != 1
        {
            return Err(invalid());
        }
        self.0.child(name, create)
    }
}
pub(super) fn private_directory(path: &Path, web: Option<&Path>) -> io::Result<PrivateDirectory> {
    directory(path, web).map(PrivateDirectory)
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, UNAVAILABLE)
}
fn ordinary(metadata: &std::fs::Metadata, directory: bool) -> io::Result<()> {
    if metadata.file_type().is_symlink()
        || metadata.is_dir() != directory
        || (!directory && !metadata.is_file())
    {
        return Err(invalid());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(invalid());
        }
    }
    Ok(())
}
#[cfg(unix)]
fn namespace_permissions(metadata: &std::fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let owner = metadata.uid();
    let current = unsafe { libc::geteuid() };
    // Every ancestor must be controlled by this principal or the privileged OS
    // owner. A trusted sticky directory (notably /tmp) protects its already
    // trusted child entries even when other users may create their own entries.
    if (owner != 0 && owner != current)
        || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
    {
        return Err(invalid());
    }
    Ok(())
}
fn ensure_directory(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            ordinary(&metadata, true)?;
            #[cfg(unix)]
            {
                namespace_permissions(&metadata)?;
                if let Some(parent) = path.parent() {
                    ensure_directory(parent)?;
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            ensure_directory(path.parent().ok_or_else(invalid)?)?;
            #[cfg(windows)]
            let created = super::private_secret_file::create_private_directory(path);
            #[cfg(unix)]
            let created = {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new().mode(0o700).create(path)
            };
            match created {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    ordinary(&std::fs::symlink_metadata(path)?, true)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}
fn open_directory(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .access_mode(0x0002_0081)
            .share_mode(3)
            .custom_flags(0x0220_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    ordinary(&file.metadata()?, true)?;
    Ok(file)
}
fn directory(data: &Path, web: Option<&Path>) -> io::Result<Directory> {
    if data.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid());
    }
    let absolute = std::path::absolute(data)?;
    ensure_directory(&absolute)?;
    let mut namespace = Vec::new();
    // Check the original namespace before canonicalization can hide a link.
    for path in absolute.ancestors() {
        let metadata = std::fs::symlink_metadata(path)?;
        ordinary(&metadata, true)?;
        #[cfg(unix)]
        namespace_permissions(&metadata)?;
        #[cfg(windows)]
        namespace.push(open_directory(path)?);
    }
    let path = absolute.canonicalize()?;
    if let Some(web) = web {
        let web = web.canonicalize()?;
        if path.starts_with(web) {
            return Err(invalid());
        }
    }
    let file = open_directory(&path)?;
    #[cfg(windows)]
    super::private_secret_file::validate_directory(&file)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err(invalid());
        }
    }
    let directory = Directory {
        path,
        file,
        _namespace: std::mem::take(&mut namespace),
    };
    directory.check_identity()?;
    Ok(directory)
}
impl Directory {
    fn check_identity(&self) -> io::Result<()> {
        ordinary(&std::fs::symlink_metadata(&self.path)?, true)?;
        let current = open_directory(&self.path)?;
        if identity(&current)? != identity(&self.file)? {
            return Err(invalid());
        }
        Ok(())
    }
    #[cfg(unix)]
    fn child(&self, name: &str, create: bool) -> io::Result<File> {
        use std::{
            ffi::CString,
            os::fd::{AsRawFd, FromRawFd},
        };
        let name = CString::new(name).map_err(|_| invalid())?;
        let flags = libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | if create {
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL
            } else {
                libc::O_RDONLY
            };
        let fd = unsafe { libc::openat(self.file.as_raw_fd(), name.as_ptr(), flags, 0o600) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(windows)]
    fn child(&self, name: &str, create: bool) -> io::Result<File> {
        use std::os::windows::fs::OpenOptionsExt;
        if create {
            return super::private_secret_file::create_private_file(&self.path.join(name));
        }
        OpenOptions::new()
            .read(true)
            .share_mode(1)
            .custom_flags(0x0020_0000)
            .open(self.path.join(name))
    }
}
#[cfg(unix)]
fn identity(file: &File) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}
#[cfg(windows)]
fn information(
    file: &File,
) -> io::Result<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info)
}
#[cfg(windows)]
fn identity(file: &File) -> io::Result<(u64, u64)> {
    let info = information(file)?;
    Ok((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}
fn validate_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    ordinary(&metadata, false)?;
    if metadata.len() > MAX_BYTES {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(invalid());
        }
    }
    #[cfg(windows)]
    {
        if information(file)?.nNumberOfLinks != 1 {
            return Err(invalid());
        }
        super::private_secret_file::validate(file)?;
    }
    Ok(())
}
fn read(file: &mut File) -> io::Result<Zeroizing<String>> {
    validate_file(file)?;
    let mut raw = Zeroizing::new(Vec::new());
    Read::by_ref(file)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut raw)?;
    if raw.len() as u64 > MAX_BYTES || raw.len() as u64 != file.metadata()?.len() {
        return Err(invalid());
    }
    let text = std::str::from_utf8(&raw).map_err(|_| invalid())?.trim();
    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    validate_file(file)?;
    Ok(Zeroizing::new(text.to_owned()))
}
fn existing(directory: &Directory, name: &str) -> io::Result<Option<(File, Zeroizing<String>)>> {
    #[cfg(unix)]
    {
        match directory.child(name, false) {
            Ok(mut file) => {
                let value = read(&mut file)?;
                Ok(Some((file, value)))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    #[cfg(windows)]
    let deadline = Instant::now() + Duration::from_secs(5);
    #[cfg(windows)]
    loop {
        match directory.child(name, false) {
            Ok(mut file) => {
                let value = read(&mut file)?;
                return Ok(Some((file, value)));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            #[cfg(windows)]
            Err(error) if error.raw_os_error() == Some(32) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(error) => return Err(error),
        }
    }
}
struct Pending {
    file: Option<File>,
    path: PathBuf,
    published: bool,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        if let Some(file) = &self.file {
            #[cfg(windows)]
            {
                use std::os::windows::io::AsRawHandle;
                use windows_sys::Win32::Storage::FileSystem::{
                    FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
                };
                let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
                unsafe {
                    SetFileInformationByHandle(
                        file.as_raw_handle(),
                        FileDispositionInfo,
                        (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                        std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
                    );
                }
            }
            #[cfg(unix)]
            {
                if std::fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
                    use std::os::unix::fs::MetadataExt;
                    file.metadata().is_ok_and(|held| {
                        held.dev() == metadata.dev() && held.ino() == metadata.ino()
                    })
                }) {
                    let _ = std::fs::remove_file(&self.path);
                }
            }
        }
    }
}
fn prepare(directory: &Directory) -> io::Result<Pending> {
    let name = format!(".authority-{}.tmp", uuid::Uuid::new_v4().simple());
    let file = directory.child(&name, true)?;
    let mut pending = Pending {
        file: Some(file),
        path: directory.path.join(name),
        published: false,
    };
    use ring::rand::SecureRandom;
    let mut random = Zeroizing::new([0u8; 32]);
    ring::rand::SystemRandom::new()
        .fill(random.as_mut())
        .map_err(|_| invalid())?;
    let value = Zeroizing::new(
        random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    );
    let file = pending.file.as_mut().ok_or_else(invalid)?;
    file.write_all(value.as_bytes())?;
    file.sync_all()?;
    validate_file(file)?;
    Ok(pending)
}
#[cfg(windows)]
fn rename_owned(file: &File, target: &Path) -> io::Result<()> {
    use std::{
        mem::{offset_of, size_of},
        os::windows::{ffi::OsStrExt, io::AsRawHandle},
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_RENAME_INFO, FileRenameInfo, SetFileInformationByHandle,
    };
    // Win32 also resolves the absolute DOS path, which needs a guaranteed NUL
    // terminator. FileNameLength counts only the actual path bytes. Allocation
    // alignment must never be the source of a coincidental terminator.
    let name: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let bytes = offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2;
    let mut buffer = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let allocation = buffer.as_mut_ptr().cast::<u8>();
    let info = allocation.cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = ((name.len() - 1) * 2) as u32;
        // FILE_RENAME_INFO ends in a C flexible array represented as [u16; 1].
        // Derive the destination from the entire backing allocation. A mutable
        // borrow of FileName would describe only one u16, not this full buffer.
        let destination = allocation
            .add(offset_of!(FILE_RENAME_INFO, FileName))
            .cast::<u16>();
        std::ptr::copy_nonoverlapping(name.as_ptr(), destination, name.len());
        if SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfo,
            info.cast(),
            bytes as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
#[cfg(unix)]
fn rename_owned(directory: &Directory, source: &Path, name: &str) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::{
            ffi::CString,
            os::{fd::AsRawFd, unix::ffi::OsStrExt},
        };
        let source = CString::new(source.file_name().ok_or_else(invalid)?.as_bytes())
            .map_err(|_| invalid())?;
        let target = CString::new(name).map_err(|_| invalid())?;
        if unsafe {
            libc::renameat2(
                directory.file.as_raw_fd(),
                source.as_ptr(),
                directory.file.as_raw_fd(),
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        directory.file.sync_all()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (directory, source, name);
        Err(io::Error::new(io::ErrorKind::Unsupported, UNAVAILABLE))
    }
}
fn publish(
    directory: &Directory,
    name: &str,
    mut pending: Pending,
) -> io::Result<(File, Zeroizing<String>)> {
    let file = pending.file.as_ref().ok_or_else(invalid)?;
    let expected = identity(file)?;
    #[cfg(windows)]
    let transition = {
        use std::os::windows::fs::OpenOptionsExt;
        // This owned read handle bridges the creator's WRITE/DELETE access and
        // the final read-only guard. The final guard denies write AND delete.
        OpenOptions::new()
            .read(true)
            .share_mode(7)
            .custom_flags(0x0020_0000)
            .open(&pending.path)?
    };
    #[cfg(windows)]
    let renamed = rename_owned(file, &directory.path.join(name));
    #[cfg(unix)]
    let renamed = rename_owned(directory, &pending.path, name);
    match renamed {
        Ok(()) => {
            pending.published = true;
            file.sync_all()?;
            drop(pending.file.take());
            let (file, value) = existing(directory, name)?.ok_or_else(invalid)?;
            if identity(&file)? != expected {
                return Err(invalid());
            }
            #[cfg(windows)]
            {
                if identity(&transition)? != expected {
                    return Err(invalid());
                }
                drop(transition);
            }
            Ok((file, value))
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            existing(directory, name)?.ok_or_else(invalid)
        }
        Err(error) => Err(error),
    }
}
fn load(data: &Path, web: Option<&Path>) -> io::Result<AuthorityTokens> {
    let directory = directory(data, web)?;
    // Validate all existing authorities before creating any missing counterpart.
    let daemon = existing(&directory, "daemon.token")?;
    let native = existing(&directory, "native.token")?;
    let daemon_pending = if daemon.is_none() {
        Some(prepare(&directory)?)
    } else {
        None
    };
    let native_pending = if native.is_none() {
        Some(prepare(&directory)?)
    } else {
        None
    };
    let (daemon_file, daemon) = match daemon {
        Some(value) => value,
        None => publish(
            &directory,
            "daemon.token",
            daemon_pending.ok_or_else(invalid)?,
        )?,
    };
    let (native_file, native) = match native {
        Some(value) => value,
        None => publish(
            &directory,
            "native.token",
            native_pending.ok_or_else(invalid)?,
        )?,
    };
    if daemon.eq_ignore_ascii_case(&native) {
        return Err(invalid());
    }
    directory.check_identity()?;
    Ok(AuthorityTokens {
        daemon,
        native,
        _files: vec![daemon_file, native_file],
        _directory: directory,
    })
}
pub(super) fn load_or_create(data: &Path, web: Option<&Path>) -> anyhow::Result<AuthorityTokens> {
    load(data, web).map_err(|_| anyhow::anyhow!(UNAVAILABLE))
}
pub(super) fn private_value(data: &Path, web: Option<&Path>) -> anyhow::Result<PrivateValue> {
    let load = || -> io::Result<PrivateValue> {
        let directory = directory(data, web)?;
        let (file, value) = match existing(&directory, "master.key")? {
            Some(value) => value,
            None => publish(&directory, "master.key", prepare(&directory)?)?,
        };
        directory.check_identity()?;
        file.try_lock_shared().map_err(|_| invalid())?;
        Ok(PrivateValue {
            value,
            _file: file,
            _directory: directory,
        })
    };
    load().map_err(|_| anyhow::anyhow!("Encrypted team key custody is invalid or unavailable"))
}

/// Original registry recovery never provisions a missing legacy key.
pub(super) fn existing_private_value(
    data: &Path,
    web: Option<&Path>,
) -> anyhow::Result<PrivateValue> {
    let load = || -> io::Result<PrivateValue> {
        let directory = directory(data, web)?;
        let (file, value) = existing(&directory, "master.key")?.ok_or_else(invalid)?;
        directory.check_identity()?;
        file.try_lock_shared().map_err(|_| invalid())?;
        Ok(PrivateValue {
            value,
            _file: file,
            _directory: directory,
        })
    };
    load().map_err(|_| {
        anyhow::anyhow!("Original encrypted team key is unavailable; no replacement was created")
    })
}

/// Explicit bounded operator input. Its original handle and directory namespace
/// are pinned while reading; secret bytes and OS paths never enter diagnostics.
pub(super) fn read_operator_file(
    path: &Path,
    data: &Path,
    web: Option<&Path>,
    limit: u64,
) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    let load = || -> io::Result<Zeroizing<Vec<u8>>> {
        if !path.is_absolute()
            || path.components().any(|c| matches!(c, Component::ParentDir))
            || limit == 0
            || limit > 1024 * 1024
        {
            return Err(invalid());
        }
        let parent = path.parent().ok_or_else(invalid)?;
        ordinary(&std::fs::symlink_metadata(parent)?, true)?;
        if !parent.canonicalize()?.starts_with(data.canonicalize()?) {
            return Err(invalid());
        }
        let directory = directory(parent, web)?;
        let data = data.canonicalize()?;
        if !directory.path.starts_with(data) {
            return Err(invalid());
        }
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(invalid)?;
        let mut file = directory.child(name, false)?;
        let check = |file: &File| -> io::Result<()> {
            let metadata = file.metadata()?;
            ordinary(&metadata, false)?;
            if metadata.len() == 0 || metadata.len() > limit {
                return Err(invalid());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.uid() != unsafe { libc::geteuid() }
                    || metadata.mode() & 0o077 != 0
                    || metadata.nlink() != 1
                {
                    return Err(invalid());
                }
            }
            #[cfg(windows)]
            {
                if information(file)?.nNumberOfLinks != 1 {
                    return Err(invalid());
                }
                super::private_secret_file::validate(file)?;
            }
            Ok(())
        };
        check(&file)?;
        let original = identity(&file)?;
        let length = file.metadata()?.len();
        let mut bytes = Zeroizing::new(Vec::new());
        Read::by_ref(&mut file)
            .take(limit + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != length {
            return Err(invalid());
        }
        check(&file)?;
        directory.check_identity()?;
        if identity(&directory.child(name, false)?)? != original {
            return Err(invalid());
        }
        Ok(bytes)
    };
    load().map_err(|_| anyhow::anyhow!("Private operator configuration is invalid or unavailable"))
}

struct OriginalFileMaterial {
    id: cowork_secrets::KeyId,
    material: Zeroizing<[u8; 32]>,
}
impl cowork_secrets::KeyStore for OriginalFileMaterial {
    fn load(&self, id: &cowork_secrets::KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        if id != &self.id {
            return Err(cowork_secrets::Error::Missing);
        }
        Ok(Zeroizing::new(*self.material))
    }
}
fn original_file_proof(
    file: &mut File,
    proof: &cowork_secrets::KeyProof,
) -> io::Result<cowork_storage::server_keys::LegacyMasterFileProof> {
    validate_file(file)?;
    let mut raw = Zeroizing::new(Vec::new());
    Read::by_ref(file)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut raw)?;
    if raw.len() as u64 != file.metadata()?.len() || raw.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    let text = std::str::from_utf8(&raw).map_err(|_| invalid())?.trim();
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let id = super::team_key_file::locator(text).map_err(|_| invalid())?;
    let mut material = Zeroizing::new([0u8; 32]);
    for (index, byte) in material.iter_mut().enumerate() {
        let digit = |v: u8| (v as char).to_digit(16).ok_or_else(invalid);
        *byte = ((digit(text.as_bytes()[index * 2])? << 4) | digit(text.as_bytes()[index * 2 + 1])?)
            as u8;
    }
    cowork_secrets::verify_key(&OriginalFileMaterial { id, material }, proof)
        .map_err(|_| invalid())?;
    validate_file(file)?;
    let (first, second) = identity(file)?;
    let sha256 = ring::digest::digest(&ring::digest::SHA256, &raw)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(cowork_storage::server_keys::LegacyMasterFileProof {
        sha256,
        file_identity: [first, second],
    })
}
pub(super) fn inspect_legacy_master_file(
    data: &Path,
    web: Option<&Path>,
    proof: &cowork_secrets::KeyProof,
) -> anyhow::Result<cowork_storage::server_keys::LegacyMasterFileProof> {
    let inspect = || -> io::Result<_> {
        let directory = directory(&data.join("team-keys"), web)?;
        let mut file = directory.child("master.key", false)?;
        let result = original_file_proof(&mut file, proof)?;
        directory.check_identity()?;
        if identity(&directory.child("master.key", false)?)?
            != (result.file_identity[0], result.file_identity[1])
        {
            return Err(invalid());
        }
        Ok(result)
    };
    inspect().map_err(|_| anyhow::anyhow!("Original plaintext master file cannot be proved"))
}
/// Fixed original file only; never recursive deletion or a caller-selected path.
/// Unlinking is not a claim of secure erasure from backups or an SSD.
pub(super) fn retire_legacy_master_file(
    data: &Path,
    web: Option<&Path>,
    proof: &cowork_secrets::KeyProof,
    original: &cowork_storage::server_keys::LegacyMasterFileProof,
) -> anyhow::Result<()> {
    let retire = || -> io::Result<()> {
        let directory = directory(&data.join("team-keys"), web)?;
        #[cfg(windows)]
        let opened = {
            use std::os::windows::fs::OpenOptionsExt;
            OpenOptions::new()
                .read(true)
                .access_mode(0x8001_0000)
                .share_mode(1)
                .custom_flags(0x0020_0000)
                .open(directory.path.join("master.key"))
        };
        #[cfg(unix)]
        let opened = directory.child("master.key", false);
        let mut file = match opened {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                directory.check_identity()?;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        file.try_lock().map_err(|_| invalid())?;
        if original_file_proof(&mut file, proof)? != *original {
            return Err(invalid());
        }
        directory.check_identity()?;
        #[cfg(windows)]
        let observed = {
            use std::os::windows::fs::OpenOptionsExt;
            OpenOptions::new()
                .read(true)
                .share_mode(7)
                .custom_flags(0x0020_0000)
                .open(directory.path.join("master.key"))?
        };
        #[cfg(unix)]
        let observed = directory.child("master.key", false)?;
        if identity(&observed)? != (original.file_identity[0], original.file_identity[1]) {
            return Err(invalid());
        }
        drop(observed);
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
            };
            let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
            if unsafe {
                SetFileInformationByHandle(
                    file.as_raw_handle(),
                    FileDispositionInfo,
                    (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                    std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            drop(file);
        }
        #[cfg(unix)]
        {
            use std::{ffi::CString, os::fd::AsRawFd};
            let name = CString::new("master.key").map_err(|_| invalid())?;
            if unsafe { libc::unlinkat(directory.file.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(io::Error::last_os_error());
            }
            directory.file.sync_all()?;
        }
        directory.check_identity()?;
        match std::fs::symlink_metadata(directory.path.join("master.key")) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            _ => Err(invalid()),
        }
    };
    retire().map_err(|_| {
        anyhow::anyhow!(
            "Original plaintext master removal is unconfirmed; reconcile the retained operation"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_master_retirement_requires_exact_proof_and_no_live_file_holder() {
        let fixture = tempfile::tempdir().unwrap();
        let data = fixture.path().join("protected-data");
        let key_directory = data.join("team-keys");
        write_fixture(&key_directory, "master.key", value('A').as_bytes());
        let (keys, id) = super::super::team_key_file::open_existing(&data, None).unwrap();
        assert_ne!(
            id,
            super::super::team_key_file::locator(&value('a')).unwrap()
        );
        let proof = cowork_secrets::prove_key(keys.as_ref(), &id).unwrap();
        let file = inspect_legacy_master_file(&data, None, &proof).unwrap();
        assert!(retire_legacy_master_file(&data, None, &proof, &file).is_err());
        assert!(key_directory.join("master.key").is_file());
        drop(keys);
        retire_legacy_master_file(&data, None, &proof, &file).unwrap();
        assert!(!key_directory.join("master.key").exists());
        retire_legacy_master_file(&data, None, &proof, &file).unwrap();
        write_fixture(&key_directory, "master.key", value('b').as_bytes());
        assert!(retire_legacy_master_file(&data, None, &proof, &file).is_err());
        assert_eq!(
            std::fs::read(key_directory.join("master.key")).unwrap(),
            value('b').as_bytes()
        );
    }
    fn value(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }
    fn write_fixture(data: &Path, name: &str, bytes: &[u8]) {
        let directory = directory(data, None).unwrap();
        let mut file = directory.child(name, true).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }
    fn pair(data: &Path) {
        write_fixture(data, "daemon.token", value('a').as_bytes());
        write_fixture(data, "native.token", value('b').as_bytes());
    }
    #[cfg(windows)]
    #[test]
    fn newly_published_file_names_are_exact_at_every_buffer_tail_alignment() {
        let root = tempfile::tempdir().unwrap();
        for width in 0..32 {
            for suffix in ["x", "é"] {
                let data = root
                    .path()
                    .join(format!("private-{}", suffix.repeat(width)));
                let tokens = load_or_create(&data, None).unwrap_or_else(|error| {
                    let names = std::fs::read_dir(&data)
                        .map(|entries| {
                            entries
                                .map(|entry| entry.unwrap().file_name())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    panic!(
                        "private filename fixture width={width} suffix={suffix}: {names:?}; {error}"
                    );
                });
                assert_eq!(tokens.daemon.len(), 64);
                assert_eq!(tokens.native.len(), 64);
                let mut names = std::fs::read_dir(&data)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .collect::<Vec<_>>();
                names.sort();
                assert_eq!(names, ["daemon.token", "native.token"]);
            }
        }
    }
    #[cfg(unix)]
    fn public(path: &Path, write: bool) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if write {
                0o777
            } else if path.is_dir() {
                0o755
            } else {
                0o644
            }),
        )
        .unwrap();
    }
    #[cfg(windows)]
    fn public(path: &Path, write: bool) {
        use std::{
            os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
            ptr,
        };
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{Authorization::*, *},
        };
        let file = OpenOptions::new()
            .access_mode(0x0006_0000)
            .custom_flags(if path.is_dir() { 0x0200_0000 } else { 0 })
            .open(path)
            .unwrap();
        let mut descriptor = ptr::null_mut();
        let mut owner = ptr::null_mut();
        assert_eq!(
            unsafe {
                GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION,
                    &mut owner,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut descriptor,
                )
            },
            0
        );
        let mut sid = ptr::null_mut();
        assert_ne!(unsafe { ConvertSidToStringSidW(owner, &mut sid) }, 0);
        let mut length = 0;
        while unsafe { *sid.add(length) } != 0 {
            length += 1;
            assert!(length < 256);
        }
        let owner = String::from_utf16(unsafe { std::slice::from_raw_parts(sid, length) }).unwrap();
        unsafe {
            LocalFree(sid.cast());
            LocalFree(descriptor);
        }
        let sddl: Vec<_> = format!(
            "D:P(A;;FA;;;{owner})(A;;{};;;WD)",
            if write { "FA" } else { "FR" }
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let mut descriptor = ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    1,
                    &mut descriptor,
                    ptr::null_mut(),
                )
            },
            0
        );
        let mut present = 0;
        let mut acl = ptr::null_mut();
        let mut defaulted = 0;
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted)
            },
            0
        );
        assert_eq!(
            unsafe {
                SetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    acl,
                    ptr::null_mut(),
                )
            },
            0
        );
        unsafe {
            LocalFree(descriptor);
        }
    }
    #[test]
    fn new_private_pair_is_durable_and_restart_preserves_values_and_guards() {
        let fixture = tempfile::tempdir().unwrap();
        let data = fixture.path().join("new-data");
        let first = load_or_create(&data, None).unwrap();
        assert!(first.daemon().len() == 64 && first.native().len() == 64);
        assert!(!first.daemon().eq_ignore_ascii_case(first.native()));
        let restarted = load_or_create(&data, None).unwrap();
        assert!(first.daemon() == restarted.daemon() && first.native() == restarted.native());
        assert!(std::fs::read(data.join("daemon.token")).unwrap() == first.daemon().as_bytes());
        #[cfg(windows)]
        {
            assert!(
                OpenOptions::new()
                    .write(true)
                    .open(data.join("daemon.token"))
                    .is_err()
            );
            assert!(std::fs::rename(data.join("daemon.token"), data.join("replacement")).is_err());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert!(std::fs::metadata(&data).unwrap().mode() & 0o077 == 0);
            assert!(std::fs::metadata(data.join("daemon.token")).unwrap().mode() & 0o077 == 0);
        }
    }
    #[test]
    fn existing_hex_case_trim_and_directory_listing_remain_compatible_without_rewrite() {
        let fixture = tempfile::tempdir().unwrap();
        let data = fixture.path().join("existing-data");
        let bytes = format!(" \t{}\r\n", value('A')).into_bytes();
        write_fixture(&data, "daemon.token", &bytes);
        write_fixture(&data, "native.token", value('b').as_bytes());
        public(&data, false);
        let loaded = load_or_create(&data, None).unwrap();
        assert!(loaded.daemon() == value('A'));
        assert!(std::fs::read(data.join("daemon.token")).unwrap() == bytes);
    }
    #[test]
    fn malformed_or_oversized_existing_file_never_creates_missing_counterpart_or_echoes_input() {
        for bytes in [
            vec![b'a'; 129],
            vec![b'a'; 63],
            vec![b'Z'; 64],
            vec![0xff; 64],
            format!("{}\n{}", value('a'), value('b')).into_bytes(),
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let data = fixture.path().join("private-input-marker");
            write_fixture(&data, "native.token", &bytes);
            let error = match load_or_create(&data, None) {
                Ok(_) => panic!("Invalid fixture accepted"),
                Err(error) => error.to_string(),
            };
            assert!(error == UNAVAILABLE);
            assert!(!data.join("daemon.token").exists());
            assert!(std::fs::read(data.join("native.token")).unwrap() == bytes);
        }
    }
    #[test]
    fn identical_case_folded_authorities_are_refused_without_rotation() {
        let fixture = tempfile::tempdir().unwrap();
        let data = fixture.path().join("duplicates");
        write_fixture(&data, "daemon.token", value('a').as_bytes());
        write_fixture(&data, "native.token", value('A').as_bytes());
        assert!(load_or_create(&data, None).is_err());
        assert!(std::fs::read(data.join("native.token")).unwrap() == value('A').as_bytes());
    }
    #[test]
    fn public_file_and_writable_data_directory_are_refused_without_repair() {
        for directory_failure in [false, true] {
            let fixture = tempfile::tempdir().unwrap();
            let data = fixture.path().join("unsafe-data");
            pair(&data);
            let file_path = data.join("daemon.token");
            public(if directory_failure { &data } else { &file_path }, true);
            assert!(load_or_create(&data, None).is_err());
            assert!(std::fs::read(data.join("daemon.token")).unwrap() == value('a').as_bytes());
            assert!(load_or_create(&data, None).is_err());
        }
    }
    #[test]
    fn hardlink_aliases_and_served_web_namespaces_are_refused() {
        let fixture = tempfile::tempdir().unwrap();
        let data = fixture.path().join("linked-data");
        pair(&data);
        std::fs::hard_link(data.join("daemon.token"), fixture.path().join("alias")).unwrap();
        assert!(load_or_create(&data, None).is_err());
        let served = fixture.path().join("served-data");
        std::fs::create_dir(&served).unwrap();
        assert!(load_or_create(&served, Some(fixture.path())).is_err());
        assert!(!served.join("daemon.token").exists() && !served.join("native.token").exists());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_file_and_ancestor_are_refused_before_authority_creation() {
        use std::os::unix::fs::symlink;
        let fixture = tempfile::tempdir().unwrap();
        let data = fixture.path().join("data");
        pair(&data);
        std::fs::remove_file(data.join("native.token")).unwrap();
        symlink(data.join("daemon.token"), data.join("native.token")).unwrap();
        assert!(load_or_create(&data, None).is_err());
        let alias = fixture.path().join("directory-alias");
        symlink(&data, &alias).unwrap();
        assert!(load_or_create(&alias, None).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn mutable_parent_that_can_rename_a_private_child_is_refused_without_repair() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        let parent = fixture.path().join("mutable-parent");
        let data = parent.join("private-child");
        pair(&data);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(load_or_create(&data, None).is_err());
        let moved = parent.join("moved-child");
        std::fs::rename(&data, &moved).unwrap();
        assert!(std::fs::read(moved.join("daemon.token")).unwrap() == value('a').as_bytes());
        assert!(parent.metadata().unwrap().permissions().mode() & 0o777 == 0o777);
        assert!(load_or_create(&moved, None).is_err());
        let missing = parent.join("new-child");
        assert!(load_or_create(&missing, None).is_err());
        assert!(!missing.exists());
    }
    #[cfg(unix)]
    #[test]
    fn private_child_of_root_owned_sticky_tmp_is_startable() {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata("/tmp").unwrap();
        assert!(metadata.uid() == 0 && metadata.mode() & 0o1000 != 0);
        let fixture = tempfile::tempdir_in("/tmp").unwrap();
        let data = fixture.path().join("private-child");
        let authorities = load_or_create(&data, None).unwrap();
        assert!(authorities.daemon().len() == 64 && authorities.native().len() == 64);
    }
    #[cfg(windows)]
    #[test]
    fn native_junction_ancestor_is_refused() {
        use std::os::windows::process::CommandExt;
        let fixture = tempfile::tempdir().unwrap();
        let target = fixture.path().join("owned-target");
        pair(&target);
        let alias = fixture.path().join("directory-alias");
        let result = std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(&alias)
            .arg(&target)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert!(load_or_create(&alias, None).is_err());
    }
    #[test]
    fn concurrent_startups_adopt_complete_durable_winners_without_partial_publication() {
        let fixture = tempfile::tempdir().unwrap();
        let data = fixture.path().join("concurrent-data");
        let start = std::sync::Arc::new(std::sync::Barrier::new(4));
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observer = {
            let path = data.clone();
            let barrier = start.clone();
            let finished = finished.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let mut observed = 0;
                loop {
                    for name in ["daemon.token", "native.token"] {
                        match std::fs::read(path.join(name)) {
                            Ok(raw) => {
                                assert!(raw.len() == 64 && raw.iter().all(u8::is_ascii_hexdigit));
                                observed += 1;
                            }
                            Err(error) => assert!(error.kind() == io::ErrorKind::NotFound),
                        }
                    }
                    if finished.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    std::thread::yield_now();
                }
                assert!(observed > 0);
            })
        };
        let mut threads = Vec::new();
        for _ in 0..2 {
            let path = data.clone();
            let barrier = start.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                load_or_create(&path, None)
            }));
        }
        start.wait();
        let first = threads.remove(0).join().unwrap().unwrap();
        let second = threads.remove(0).join().unwrap().unwrap();
        finished.store(true, std::sync::atomic::Ordering::SeqCst);
        observer.join().unwrap();
        assert!(first.daemon() == second.daemon() && first.native() == second.native());
        for name in ["daemon.token", "native.token"] {
            let raw = std::fs::read(data.join(name)).unwrap();
            assert!(raw.len() == 64 && raw.iter().all(u8::is_ascii_hexdigit));
        }
        assert!(std::fs::read_dir(&data).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            name == "daemon.token" || name == "native.token"
        }));
    }
}
