//! Native reservations are denial policy, never a capability to create or adopt a workspace.
use super::{Dir, Error, FileBroker, Result, storage_error};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

/// Reserved direct-child initialization names under native Profile and legacy LocalAppData.
/// This prefix is exclusively denial policy; a matching name proves no custody.
pub const DOCUMENT_WORKSPACE_INIT_PREFIX: &str = ".localai-cowork-doc-init-";

/// Fixed per-user document scratch namespace. No environment or renderer override.
/// This query neither creates nor changes the returned directory.
pub fn document_workspace_base() -> Result<Option<PathBuf>> {
    #[cfg(test)]
    if let Some(value) = TEST_BASE.with(|base| base.borrow().clone()) {
        return Ok(value);
    }
    native_document_workspace_base()
}

#[cfg(windows)]
fn known_folder_path(folder: &windows_sys::core::GUID) -> Result<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, ptr};
    use windows_sys::Win32::{System::Com::CoTaskMemFree, UI::Shell::SHGetKnownFolderPath};
    let mut raw = ptr::null_mut();
    // NULL token selects the current native user; flags 0 do not request creation.
    // The Shell owns the terminated allocation; free it on success AND failure.
    let result = unsafe { SHGetKnownFolderPath(folder, 0, ptr::null_mut(), &mut raw) };
    let path = if result >= 0 && !raw.is_null() {
        let mut length = 0;
        // SHGetKnownFolderPath promises a terminated UTF-16 string.
        unsafe {
            while *raw.add(length) != 0 {
                length += 1;
            }
        }
        let value = unsafe { OsString::from_wide(std::slice::from_raw_parts(raw, length)) };
        Some(PathBuf::from(value))
    } else {
        None
    };
    unsafe {
        CoTaskMemFree(raw.cast());
    }
    let path =
        path.ok_or_else(|| Error::Validation("Native Known Folder is unavailable".into()))?;
    if !path.is_absolute() {
        return Err(Error::Forbidden);
    }
    Ok(path)
}

#[cfg(windows)]
fn native_document_workspace_base() -> Result<Option<PathBuf>> {
    use windows_sys::Win32::UI::Shell::FOLDERID_Profile;
    Ok(Some(
        known_folder_path(&FOLDERID_Profile)?
            .join(".localai-cowork-documents")
            .join("work"),
    ))
}

#[cfg(windows)]
fn native_legacy_document_workspace_base() -> Result<PathBuf> {
    use windows_sys::Win32::UI::Shell::FOLDERID_LocalAppData;
    Ok(known_folder_path(&FOLDERID_LocalAppData)?
        .join("LocalAI-Cowork-Documents")
        .join("work"))
}

// Legacy native namespaces remain denial policy only: no migration, adoption or cleanup.
fn reservation_bases() -> Result<Vec<PathBuf>> {
    #[cfg(test)]
    if let Some(primary) = TEST_BASE.with(|base| base.borrow().clone()) {
        let mut bases: Vec<_> = primary.into_iter().collect();
        bases.extend(TEST_ADDITIONAL_BASES.with(|bases| bases.borrow().clone()));
        return Ok(bases);
    }
    #[cfg(windows)]
    {
        Ok(vec![
            native_document_workspace_base()?.ok_or(Error::Forbidden)?,
            native_legacy_document_workspace_base()?,
        ])
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

#[cfg(not(windows))]
fn native_document_workspace_base() -> Result<Option<PathBuf>> {
    Ok(None)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DirectoryId(u64, u128);

#[cfg(windows)]
fn native_id(file: &impl std::os::windows::io::AsRawHandle) -> Result<DirectoryId> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };
    let mut value = std::mem::MaybeUninit::<FILE_ID_INFO>::uninit();
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle().cast(),
            FileIdInfo,
            value.as_mut_ptr().cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if result == 0 {
        return Err(storage_error(std::io::Error::last_os_error()));
    }
    let value = unsafe { value.assume_init() };
    Ok(DirectoryId(
        value.VolumeSerialNumber,
        u128::from_le_bytes(value.FileId.Identifier),
    ))
}

fn file_id(file: &fs::File) -> Result<DirectoryId> {
    #[cfg(windows)]
    {
        native_id(file)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata().map_err(storage_error)?;
        Ok(DirectoryId(metadata.dev(), metadata.ino() as u128))
    }
}

fn dir_id(dir: &Dir) -> Result<DirectoryId> {
    #[cfg(windows)]
    {
        native_id(dir)
    }
    #[cfg(unix)]
    {
        use cap_fs_ext::MetadataExt;
        let metadata = dir.dir_metadata().map_err(storage_error)?;
        Ok(DirectoryId(metadata.dev(), metadata.ino() as u128))
    }
}

fn aliased(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn directory(path: &Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        };
        // Metadata only. These handles grant no content access and do not own the workspace.
        options
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(7)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path).map_err(storage_error)?;
    let metadata = file.metadata().map_err(storage_error)?;
    if !metadata.is_dir() || aliased(&metadata) {
        return Err(Error::Forbidden);
    }
    Ok(file)
}

fn normalized(path: &Path) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(PathBuf::from(
            path.to_str().ok_or(Error::Forbidden)?.to_lowercase(),
        ))
    }
    #[cfg(not(windows))]
    {
        Ok(path.to_owned())
    }
}

struct ProjectedPath {
    path: PathBuf,
    anchor: PathBuf,
    file: fs::File,
    exists: bool,
}

// Resolve only existing native ancestors, append only validated missing components.
// NotFound is the only absence result; permissions, files and ambiguous aliases fail closed.
fn project(path: &Path, reject_aliases: bool) -> Result<ProjectedPath> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(Error::Forbidden);
    }
    if reject_aliases {
        for ancestor in path.ancestors() {
            match fs::symlink_metadata(ancestor) {
                Ok(metadata) if !metadata.is_dir() || aliased(&metadata) => {
                    return Err(Error::Forbidden);
                }
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(storage_error(error)),
            }
        }
    }
    let mut cursor = path;
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(cursor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = cursor.file_name().ok_or(Error::Forbidden)?;
                #[cfg(windows)]
                {
                    let value = name.to_str().ok_or(Error::Forbidden)?;
                    // Absent suffixes cannot be canonicalized. Reject Win32 normalization,
                    // streams and device spellings rather than guessing their eventual target.
                    if value.ends_with(['.', ' '])
                        || value
                            .chars()
                            .any(|c| c.is_control() || ":<>\"|?*".contains(c))
                    {
                        return Err(Error::Forbidden);
                    }
                    super::relative_path(value, false)?;
                }
                missing.push(name.to_owned());
                cursor = cursor.parent().ok_or(Error::Forbidden)?;
            }
            Err(error) => return Err(storage_error(error)),
        }
    }
    let canonical = fs::canonicalize(cursor).map_err(storage_error)?;
    let file = directory(&canonical)?;
    if file_id(&file)? != file_id(&directory(cursor)?)? {
        return Err(Error::Forbidden);
    }
    let mut projected = canonical.clone();
    for name in missing.iter().rev() {
        projected.push(name);
    }
    Ok(ProjectedPath {
        path: normalized(&projected)?,
        anchor: canonical,
        file,
        exists: missing.is_empty(),
    })
}

fn ancestor_ids(path: &Path) -> Result<Vec<DirectoryId>> {
    path.ancestors()
        .map(|ancestor| file_id(&directory(ancestor)?))
        .collect()
}

fn selected_ancestor_ids(path: &Path) -> Result<Vec<DirectoryId>> {
    let mut identities = Vec::new();
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if !metadata.is_dir() || aliased(&metadata) => {
                return Err(Error::Forbidden);
            }
            Ok(_) => identities.push(file_id(&directory(ancestor)?)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(storage_error(error)),
        }
    }
    Ok(identities)
}

struct WorkspaceReservation {
    requested: PathBuf,
    workspace: PathBuf,
    init_parent: PathBuf,
    comparison: PathBuf,
    anchor: PathBuf,
    original: fs::File,
}

impl WorkspaceReservation {
    fn new(workspace: PathBuf) -> Result<Self> {
        project(&workspace, true)?;
        // Reserve the fixed private parent too: unpublished crash-recovery siblings
        // belong to the same private namespace as the workspace, even before it exists.
        let requested = workspace.parent().ok_or(Error::Forbidden)?.to_owned();
        let init_parent = requested.parent().ok_or(Error::Forbidden)?.to_owned();
        let initial = project(&requested, true)?;
        Ok(Self {
            requested,
            workspace,
            init_parent,
            comparison: initial.path,
            anchor: initial.anchor,
            original: initial.file,
        })
    }

    fn check(&self, path: &Path, held: Option<&Dir>) -> Result<()> {
        project(&self.workspace, true)?;
        let current = project(&self.requested, true)?;
        if current.path != self.comparison
            || file_id(&directory(&self.anchor)?)? != file_id(&self.original)?
        {
            return Err(Error::Forbidden);
        }
        // Keep the originally selected namespace in scope too. Canonicalization
        // alone must not turn an outward link inside private data into a grant.
        let selected = selected_ancestor_ids(path)?;
        let candidate = project(path, false)?;
        let local = project(&self.init_parent, true)?;
        // Reserve all direct initialization siblings even while they do not exist.
        // No enumeration, initialization or adoption of those objects occurs here.
        if candidate
            .path
            .strip_prefix(&local.path)
            .ok()
            .and_then(|p| p.components().next())
            .is_some_and(|c| {
                c.as_os_str()
                    .to_str()
                    .is_some_and(|n| n.starts_with(DOCUMENT_WORKSPACE_INIT_PREFIX))
            })
        {
            return Err(Error::Forbidden);
        }
        // The native parent identity also covers alternate volume spellings.
        if local.exists {
            for ancestor in candidate.path.ancestors() {
                if ancestor
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(DOCUMENT_WORKSPACE_INIT_PREFIX))
                {
                    let parent = project(ancestor.parent().ok_or(Error::Forbidden)?, false)?;
                    if parent.exists && file_id(&parent.file)? == file_id(&local.file)? {
                        return Err(Error::Forbidden);
                    }
                }
            }
        }
        if candidate.path.starts_with(&current.path) || current.path.starts_with(&candidate.path) {
            return Err(Error::Forbidden);
        }
        // Native ancestry also covers alternative volume/short-name spellings.
        let reserved_ancestors = ancestor_ids(&current.anchor)?;
        if candidate.exists && reserved_ancestors.contains(&file_id(&candidate.file)?) {
            return Err(Error::Forbidden);
        }
        if let Some(held) = held {
            let identity = dir_id(held)?;
            if !candidate.exists
                || identity != file_id(&candidate.file)?
                || reserved_ancestors.contains(&identity)
            {
                return Err(Error::Forbidden);
            }
        }
        if current.exists
            && (selected.contains(&file_id(&current.file)?)
                || ancestor_ids(&candidate.anchor)?.contains(&file_id(&current.file)?))
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}

pub(super) struct DocumentWorkspaceReservation {
    reservations: Vec<WorkspaceReservation>,
}

impl DocumentWorkspaceReservation {
    pub(super) fn native() -> Result<Option<Self>> {
        let reservations = reservation_bases()?
            .into_iter()
            .map(WorkspaceReservation::new)
            .collect::<Result<Vec<_>>>()?;
        Ok((!reservations.is_empty()).then_some(Self { reservations }))
    }

    pub(super) fn check(&self, path: &Path, held: Option<&Dir>) -> Result<()> {
        // Check every original reservation afresh, including revoked reconciliation.
        for reservation in &self.reservations {
            reservation.check(path, held)?;
        }
        Ok(())
    }
}

impl FileBroker {
    pub(super) fn require_not_document_workspace(
        &self,
        path: &Path,
        held: Option<&Dir>,
    ) -> Result<()> {
        if let Some(reservation) = &self.document_workspace {
            reservation.check(path, held)?;
        }
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    // Private per-thread native-policy substitution. Never present in production.
    static TEST_BASE: std::cell::RefCell<Option<Option<PathBuf>>> = const { std::cell::RefCell::new(None) };
    static TEST_ADDITIONAL_BASES: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
mod tests;
