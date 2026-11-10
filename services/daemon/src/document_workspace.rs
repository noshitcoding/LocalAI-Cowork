//! Native profile-bound document scratch. Locator strings never grant authority.
use super::authority_token_file::{self, PrivateDirectory};
use base64::Engine;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    sync::Arc,
};

const PARENT_MARKER: &str = ".document-parent.json";
const BASE_MARKER: &str = ".document-base.json";
const SLOT_MARKER: &str = ".document-slot.json";
const SOURCE_MARKER: &str = ".document-workspace.json";
const MAX_MARKER: u64 = 4096;
const UNAVAILABLE: &str = "Private document workspace is invalid or unavailable; no existing workspace was adopted or repaired";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    volume: u64,
    file: [u8; 16],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentMarker {
    format: String,
    directory: Identity,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BaseMarker {
    format: String,
    directory: Identity,
    parent: Identity,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    format: String,
    source: Identity,
    parent: Identity,
    base: Identity,
    slot: Identity,
    locator: String,
}

struct Custody {
    _directories: Vec<PrivateDirectory>,
    _markers: Vec<File>,
}

trait DirectoryParent {
    fn path(&self) -> &Path;
    fn original_directory(&self) -> &File;
    fn verify_parent(&self) -> io::Result<Identity>;
}
impl DirectoryParent for PrivateDirectory {
    fn path(&self) -> &Path {
        PrivateDirectory::path(self)
    }
    fn original_directory(&self) -> &File {
        PrivateDirectory::original_directory(self)
    }
    fn verify_parent(&self) -> io::Result<Identity> {
        verify_directory(self)
    }
}
#[cfg(windows)]
struct NativeNamespaceParent {
    path: std::path::PathBuf,
    file: File,
    _namespace: Vec<File>,
}
#[cfg(windows)]
impl DirectoryParent for NativeNamespaceParent {
    fn path(&self) -> &Path {
        &self.path
    }
    fn original_directory(&self) -> &File {
        &self.file
    }
    fn verify_parent(&self) -> io::Result<Identity> {
        let original = identity(&self.file)?;
        if identity(&namespace_directory(&self.path)?)? != original {
            return Err(invalid());
        }
        Ok(original)
    }
}
#[cfg(windows)]
fn namespace_directory(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    ordinary(path, true)?;
    // LIST_DIRECTORY is deliberate: attributes-only handles are not NoRename custody.
    let file = OpenOptions::new()
        .access_mode(0x0012_00a1)
        .share_mode(3)
        .custom_flags(0x0220_0000)
        .open(path)?;
    let metadata = file.metadata()?;
    use std::os::windows::fs::MetadataExt;
    if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
        return Err(invalid());
    }
    Ok(file)
}
#[cfg(windows)]
fn native_namespace_parent(path: &Path) -> io::Result<NativeNamespaceParent> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(invalid());
    }
    // This fixed native namespace may have other ACL trustees. It is never
    // adopted as private, repaired, or used as proof of child ownership.
    let mut namespace = Vec::new();
    for ancestor in path.ancestors() {
        namespace.push(namespace_directory(ancestor)?);
    }
    let file = namespace.first().ok_or_else(invalid)?.try_clone()?;
    let path = path.canonicalize()?;
    let parent = NativeNamespaceParent {
        path,
        file,
        _namespace: namespace,
    };
    parent.verify_parent()?;
    Ok(parent)
}
#[cfg(not(windows))]
fn native_namespace_parent(path: &Path) -> io::Result<PrivateDirectory> {
    private_existing(path, false)
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, UNAVAILABLE)
}
fn at_stage<T>(name: &'static str, result: io::Result<T>) -> io::Result<T> {
    result.map_err(|error| io::Error::new(error.kind(), format!("{name}: {error}")))
}
fn identity(file: &File) -> io::Result<Identity> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
        };
        let mut value = FILE_ID_INFO::default();
        // The held original object, not a path digest or legacy 64-bit file index.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&mut value as *mut FILE_ID_INFO).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Identity {
            volume: value.VolumeSerialNumber,
            file: value.FileId.Identifier,
        })
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok(Identity {
            volume: metadata.dev(),
            file: u128::from(metadata.ino()).to_le_bytes(),
        })
    }
}
fn ordinary(path: &Path, directory: bool) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_symlink()
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
fn metadata_directory(path: &Path, share_delete: bool) -> io::Result<File> {
    ordinary(path, true)?;
    let mut options = OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // These are identity witnesses, not claimed NoRename guards.
        options
            .access_mode(0x0002_0080)
            .share_mode(if share_delete { 7 } else { 3 })
            .custom_flags(0x0220_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = share_delete;
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_dir() {
        return Err(invalid());
    }
    Ok(file)
}
fn verify_directory(directory: &PrivateDirectory) -> io::Result<Identity> {
    directory.verify()?;
    let original = identity(directory.original_directory())?;
    if identity(&metadata_directory(directory.path(), false)?)? != original {
        return Err(invalid());
    }
    Ok(original)
}
fn strict_directory(directory: &PrivateDirectory) -> io::Result<()> {
    verify_directory(directory)?;
    #[cfg(windows)]
    super::private_secret_file::validate(directory.original_directory())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = directory.original_directory().metadata()?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(invalid());
        }
    }
    Ok(())
}
fn private_existing(path: &Path, strict: bool) -> io::Result<PrivateDirectory> {
    ordinary(path, true)?; // Never let private_directory create a missing recorded slot.
    let directory = authority_token_file::private_directory(path, None)?;
    if strict {
        strict_directory(&directory)?;
    } else {
        verify_directory(&directory)?;
    }
    Ok(directory)
}
fn reject_overlap(data: &PrivateDirectory, candidate: &Path) -> io::Result<()> {
    reject_overlap_identity(data.path(), verify_directory(data)?, candidate)
}
fn reject_overlap_identity(
    source_path: &Path,
    source: Identity,
    candidate: &Path,
) -> io::Result<()> {
    if !candidate.is_absolute() {
        return Err(invalid());
    }
    let mut target = None;
    for ancestor in candidate.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => {
                let file = metadata_directory(ancestor, false)?;
                let current = identity(&file)?;
                if current == source {
                    return Err(invalid());
                }
                if ancestor == candidate {
                    target = Some(current);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if let Some(target) = target {
        for ancestor in source_path.ancestors() {
            if identity(&metadata_directory(ancestor, false)?)? == target {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
fn validate_marker(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_MARKER {
        return Err(invalid());
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        super::private_secret_file::validate(file)?;
        let mut value = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut value) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if value.nNumberOfLinks != 1 {
            return Err(invalid());
        }
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
    Ok(())
}
fn marker_witness(path: &Path) -> io::Result<File> {
    ordinary(path, false)?;
    let mut options = OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .access_mode(0x0002_0080)
            .share_mode(7)
            .custom_flags(0x0020_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path)
}
fn read_marker<T: DeserializeOwned>(
    directory: &PrivateDirectory,
    name: &str,
) -> io::Result<(T, File)> {
    verify_directory(directory)?;
    ordinary(&directory.path().join(name), false)?;
    let mut file = directory.child(name, false)?;
    validate_marker(&file)?;
    let original = identity(&file)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_MARKER + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MARKER || file.metadata()?.len() != bytes.len() as u64 {
        return Err(invalid());
    }
    let value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    validate_marker(&file)?;
    verify_directory(directory)?;
    if identity(&directory.child(name, false)?)? != original {
        return Err(invalid());
    }
    Ok((value, file))
}

#[cfg(windows)]
fn rename_original(
    file: &File,
    _parent: &impl DirectoryParent,
    _source: &str,
    target: &str,
) -> io::Result<()> {
    use std::{
        mem::{offset_of, size_of},
        os::windows::{ffi::OsStrExt, io::AsRawHandle},
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_RENAME_INFO, FileRenameInfo, SetFileInformationByHandle,
    };
    let destination = _parent.path().join(target);
    let name: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let bytes = offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2;
    let mut buffer = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    // Keep the original source handle; never reopen a name to choose the renamed object.
    unsafe {
        let allocation = buffer.as_mut_ptr().cast::<u8>();
        let info = allocation.cast::<FILE_RENAME_INFO>();
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = ((name.len() - 1) * 2) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            allocation
                .add(offset_of!(FILE_RENAME_INFO, FileName))
                .cast::<u16>(),
            name.len(),
        );
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
#[cfg(target_os = "linux")]
fn rename_original(
    file: &File,
    parent: &impl DirectoryParent,
    source: &str,
    target: &str,
) -> io::Result<()> {
    use std::{ffi::CString, os::fd::AsRawFd};
    let path = parent.path().join(source);
    let observed = if fs::symlink_metadata(&path)?.is_dir() {
        metadata_directory(&path, true)?
    } else {
        marker_witness(&path)?
    };
    if identity(&observed)? != identity(file)? {
        return Err(invalid());
    }
    let source = CString::new(source).map_err(|_| invalid())?;
    let target = CString::new(target).map_err(|_| invalid())?;
    // Linux selects the child by name relative to the retained parent FD, within
    // its private single-owner namespace. This is not rename-by-child-handle.
    // Callers additionally compare the published object's full original identity.
    if unsafe {
        libc::renameat2(
            parent.original_directory().as_raw_fd(),
            source.as_ptr(),
            parent.original_directory().as_raw_fd(),
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    parent.original_directory().sync_all()
}
#[cfg(not(any(windows, target_os = "linux")))]
fn rename_original(
    _file: &File,
    _parent: &impl DirectoryParent,
    _source: &str,
    _target: &str,
) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, UNAVAILABLE))
}

fn publish_marker<T: Serialize + DeserializeOwned>(
    directory: &PrivateDirectory,
    name: &str,
    value: &T,
) -> io::Result<File> {
    verify_directory(directory)?;
    let temporary = format!(".document-publish-{}.tmp", uuid::Uuid::new_v4().simple());
    let mut file = directory.child(&temporary, true)?;
    let bytes = serde_json::to_vec(value).map_err(|_| invalid())?;
    if bytes.len() as u64 > MAX_MARKER {
        return Err(invalid());
    }
    file.write_all(&bytes)?;
    file.sync_all()?;
    validate_marker(&file)?;
    let original = identity(&file)?;
    rename_original(&file, directory, &temporary, name)?;
    let witness = marker_witness(&directory.path().join(name))?;
    if identity(&witness)? != original {
        return Err(invalid());
    }
    drop(file);
    let (_, held) = read_marker::<T>(directory, name)?;
    if identity(&held)? != identity(&witness)? {
        return Err(invalid());
    }
    Ok(held)
}
fn new_private_child(parent: &impl DirectoryParent, name: &str) -> io::Result<PrivateDirectory> {
    parent.verify_parent()?;
    let path = parent.path().join(name);
    #[cfg(windows)]
    let original = super::private_secret_file::create_private_directory_handle(
        parent.original_directory(),
        name,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&path)?;
    }
    let directory = private_existing(&path, true)?;
    #[cfg(windows)]
    if verify_directory(&directory)? != identity(&original)? {
        return Err(invalid());
    }
    parent.verify_parent()?;
    Ok(directory)
}
fn publish_directory(
    parent: &impl DirectoryParent,
    name: &str,
    target: &str,
    directory: PrivateDirectory,
) -> io::Result<PrivateDirectory> {
    parent.verify_parent()?;
    let original = metadata_directory(directory.path(), true)?;
    let expected = verify_directory(&directory)?;
    if identity(&original)? != expected {
        return Err(invalid());
    }
    // Drop NoRename custody for our own unpublished directory, retaining the original
    // identity witness and original parent. Only the native rename is allowed here.
    drop(directory);
    #[cfg(windows)]
    let publisher = {
        use std::os::windows::fs::OpenOptionsExt;
        OpenOptions::new()
            .access_mode(0x0003_0080)
            .share_mode(3)
            .custom_flags(0x0220_0000)
            .open(parent.path().join(name))?
    };
    #[cfg(not(windows))]
    let publisher = original.try_clone()?;
    if identity(&publisher)? != identity(&original)? {
        return Err(invalid());
    }
    rename_original(&publisher, parent, name, target)?;
    drop(publisher);
    let published = private_existing(&parent.path().join(target), true)?;
    if verify_directory(&published)? != identity(&original)? {
        return Err(invalid());
    }
    Ok(published)
}

fn load_parent(
    parent: &Path,
    base_name: &str,
) -> io::Result<(PrivateDirectory, PrivateDirectory, Vec<File>)> {
    let parent = private_existing(parent, true)?;
    let parent_id = verify_directory(&parent)?;
    let (marker, parent_file) = read_marker::<ParentMarker>(&parent, PARENT_MARKER)?;
    if marker.format != "cowork-document-parent/1" || marker.directory != parent_id {
        return Err(invalid());
    }
    let base = private_existing(&parent.path().join(base_name), true)?;
    let base_id = verify_directory(&base)?;
    let (marker, base_file) = read_marker::<BaseMarker>(&base, BASE_MARKER)?;
    if marker.format != "cowork-document-base/1"
        || marker.directory != base_id
        || marker.parent != parent_id
    {
        return Err(invalid());
    }
    Ok((parent, base, vec![parent_file, base_file]))
}
fn ensure_parent(base: &Path) -> io::Result<(PrivateDirectory, PrivateDirectory, Vec<File>)> {
    let parent_path = base.parent().ok_or_else(invalid)?;
    let base_name = base
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(invalid)?;
    match fs::symlink_metadata(parent_path) {
        Ok(_) => return load_parent(parent_path, base_name),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    // Prepare the complete empty private namespace outside its final name. A crash
    // leaves only an unreferenced denied staging sibling, never a half-owned base.
    let native_parent = at_stage(
        "native parent custody",
        native_namespace_parent(parent_path.parent().ok_or_else(invalid)?),
    )?;
    let stage_name = format!(
        "{}{}",
        cowork_broker::DOCUMENT_WORKSPACE_INIT_PREFIX,
        uuid::Uuid::new_v4().simple()
    );
    let stage = at_stage(
        "atomic private parent creation",
        new_private_child(&native_parent, &stage_name),
    )?;
    let parent_id = verify_directory(&stage)?;
    let parent_marker = publish_marker(
        &stage,
        PARENT_MARKER,
        &ParentMarker {
            format: "cowork-document-parent/1".into(),
            directory: parent_id.clone(),
        },
    )?;
    let child = new_private_child(&stage, base_name)?;
    let child_id = verify_directory(&child)?;
    let base_marker = publish_marker(
        &child,
        BASE_MARKER,
        &BaseMarker {
            format: "cowork-document-base/1".into(),
            directory: child_id,
            parent: parent_id,
        },
    )?;
    // Close descendant handles before a directory rename on Windows.
    drop(base_marker);
    drop(child);
    drop(parent_marker);
    let target = parent_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(invalid)?;
    match publish_directory(&native_parent, &stage_name, target, stage) {
        Ok(published) => drop(published),
        Err(_) if fs::symlink_metadata(parent_path).is_ok() => {}
        Err(error) => return Err(error),
    }
    load_parent(parent_path, base_name)
}
fn valid_locator(value: &str) -> bool {
    value.len() == 22
        && base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value)
            .is_ok_and(|bytes| {
                bytes.len() == 16
                    && base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes) == value
            })
}
fn same_binding(source: &Binding, slot: &Binding) -> bool {
    source.source == slot.source
        && source.parent == slot.parent
        && source.base == slot.base
        && source.slot == slot.slot
        && source.locator == slot.locator
}
fn configure_at(
    data: &Path,
    root: &Path,
    web: Option<&Path>,
    base_path: &Path,
    toolbox: &mut Arc<cowork_toolbox::Toolbox>,
) -> io::Result<()> {
    let data = at_stage("data custody", private_existing(data, false))?;
    let source = at_stage("source custody", private_existing(root, false))?;
    if let Some(web) = web
        && data.path().starts_with(web.canonicalize()?)
    {
        return Err(invalid());
    }
    let source_id = verify_directory(&source)?;
    let prior = match fs::symlink_metadata(source.path().join(SOURCE_MARKER)) {
        Ok(_) => {
            let (binding, file) = read_marker::<Binding>(&source, SOURCE_MARKER)?;
            if binding.format != "cowork-document-source/1"
                || binding.source != source_id
                || !valid_locator(&binding.locator)
            {
                return Err(invalid());
            }
            Some((binding, file))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    // Includes the whole data directory and the globally denied private parent.
    at_stage(
        "data separation",
        reject_overlap(&data, base_path.parent().ok_or_else(invalid)?),
    )?;
    at_stage(
        "source separation",
        reject_overlap(&source, base_path.parent().ok_or_else(invalid)?),
    )?;
    if let Some(web) = web {
        let web = web.canonicalize()?;
        let directory = metadata_directory(&web, false)?;
        reject_overlap_identity(
            &web,
            identity(&directory)?,
            base_path.parent().ok_or_else(invalid)?,
        )?;
    }
    let (parent, base, mut files) = at_stage("private workspace base", ensure_parent(base_path))?;
    let parent_id = verify_directory(&parent)?;
    let base_id = verify_directory(&base)?;
    let (slot, source_file, slot_file) = if let Some((binding, source_file)) = prior {
        if binding.parent != parent_id || binding.base != base_id {
            return Err(invalid());
        }
        let slot = private_existing(&base.path().join(&binding.locator), true)?;
        if verify_directory(&slot)? != binding.slot {
            return Err(invalid());
        }
        let (record, file) = read_marker::<Binding>(&slot, SLOT_MARKER)?;
        if record.format != "cowork-document-slot/1" || !same_binding(&binding, &record) {
            return Err(invalid());
        }
        (slot, source_file, file)
    } else {
        let mut fresh = None;
        for _ in 0..8 {
            let locator = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(uuid::Uuid::new_v4().as_bytes());
            match new_private_child(&base, &locator) {
                Ok(slot) => {
                    fresh = Some((locator, slot));
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        let (locator, slot) = fresh.ok_or_else(invalid)?;
        let supported = at_stage(
            "physical workspace validation",
            cowork_toolbox::Toolbox::document_workspace_path_supported(slot.path())
                .map_err(|error| io::Error::other(error.to_string())),
        )?;
        if !supported {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Physical private document workspace exceeds the native path budget",
            ));
        }
        let mut binding = Binding {
            format: "cowork-document-slot/1".into(),
            source: source_id,
            parent: parent_id,
            base: base_id,
            slot: verify_directory(&slot)?,
            locator,
        };
        let slot_file = at_stage(
            "private slot marker publication",
            publish_marker(&slot, SLOT_MARKER, &binding),
        )?;
        binding.format = "cowork-document-source/1".into();
        let source_file = at_stage(
            "source marker publication",
            publish_marker(&source, SOURCE_MARKER, &binding),
        )?;
        (slot, source_file, slot_file)
    };
    files.push(source_file);
    files.push(slot_file);
    let target = slot.path().to_path_buf();
    let custody = Arc::new(Custody {
        _directories: vec![data, source, parent, base, slot],
        _markers: files,
    });
    Arc::get_mut(toolbox)
        .ok_or_else(invalid)?
        .configure_document_workspace(target, custody)
        .map_err(|_| invalid())
}
pub(super) fn configure(
    data: &Path,
    root: &Path,
    web: Option<&Path>,
    toolbox: &mut Arc<cowork_toolbox::Toolbox>,
) -> anyhow::Result<()> {
    if !toolbox
        .document_workspace_required()
        .map_err(|_| anyhow::anyhow!(UNAVAILABLE))?
    {
        return Ok(());
    }
    let base = cowork_broker::document_workspace_base()
        .map_err(|_| anyhow::anyhow!(UNAVAILABLE))?
        .ok_or_else(|| anyhow::anyhow!(UNAVAILABLE))?;
    configure_at(data, root, web, &base, toolbox)
        .map_err(|error| anyhow::anyhow!("{UNAVAILABLE}: {error}"))
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod process_tests;
#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests;
