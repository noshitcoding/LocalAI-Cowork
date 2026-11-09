//! Operator-only reconciliation of a pre-ownership profile creation window.
//! No inspection creates a launch-capable capsule or an ordinary ownership marker.
use super::*;
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::Threading::*,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeObject {
    pub volume_serial: u64,
    pub file_id: [u8; 16],
    pub created: u64,
}
impl NativeObject {
    pub fn observe(file: &File) -> Result<Self> {
        native(file)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrphanSnapshot {
    pub format: u32,
    pub root: PathBuf,
    pub root_identity: NativeObject,
    pub original_journal_identity: NativeObject,
    pub original_journal_sha256: String,
    pub owner_sid: String,
    pub profile: String,
    pub profile_sid: String,
    pub profile_directory: Option<PathBuf>,
    pub original_profile_tree: BTreeMap<String, ProfileEntry>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileEntry {
    pub identity: NativeObject,
    pub directory: bool,
    pub size: u64,
    pub content_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrphanRetirement {
    pub format: u32,
    pub original: OrphanSnapshot,
    pub profile_storage_absent: bool,
    pub job_quiescent: bool,
    pub capsule_closed: bool,
    pub execution_adopted: bool,
}
impl OrphanRetirement {
    pub fn valid_closure(&self) -> bool {
        self.format == 1
            && self.original.format == 1
            && self.job_quiescent
            && self.capsule_closed
            && !self.execution_adopted
            && match &self.original.profile_directory {
                Some(path) => {
                    path.is_absolute()
                        && self.profile_storage_absent
                        && self.original.original_profile_tree.contains_key("AC")
                }
                None => {
                    !self.profile_storage_absent && self.original.original_profile_tree.is_empty()
                }
            }
    }
}

/// The owner lock, original control bytes and ordinary capsule directory remain
/// held until the operator's original intent/receipt has been persisted outside it.
pub struct OrphanInspection {
    root: File,
    journal: File,
    _lock: File,
    _gate: identity::ProfileGate,
    profile_handles: Vec<File>,
    pub snapshot: OrphanSnapshot,
}
fn native(file: &File) -> Result<NativeObject> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || info.nNumberOfLinks != 1 {
        return Err(Error::Invalid("linked or ambiguous original object"));
    }
    let mut id = FILE_ID_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(NativeObject {
        volume_serial: id.VolumeSerialNumber,
        file_id: id.FileId.Identifier,
        created: (u64::from(info.ftCreationTime.dwHighDateTime) << 32)
            | u64::from(info.ftCreationTime.dwLowDateTime),
    })
}
fn open_directory(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if !file.metadata()?.is_dir() {
        return Err(Error::Invalid("ordinary original directory required"));
    }
    native(&file)?;
    Ok(file)
}
fn private_control(file: &File, directory: bool) -> Result<()> {
    let mut owner = std::ptr::null_mut();
    let mut acl = std::ptr::null_mut();
    let mut descriptor = std::ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut acl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32).into());
    }
    struct Allocation(PSECURITY_DESCRIPTOR);
    impl Drop for Allocation {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    let _allocation = Allocation(descriptor);
    if owner.is_null()
        || acl.is_null()
        || unsafe { IsValidSid(owner) } == 0
        || unsafe { IsValidAcl(acl) } == 0
        || files::sid_string(owner)? != files::owner_sid()?
    {
        return Err(Error::Invalid(
            "original control namespace is not operator-owned",
        ));
    }
    for index in 0..unsafe { (*acl).AceCount } {
        let mut ace = std::ptr::null_mut();
        if unsafe { GetAce(acl, u32::from(index), &mut ace) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 || header.AceType == 1 {
            continue;
        }
        let offset = std::mem::size_of::<ACE_HEADER>() + 4;
        if header.AceType != 0 || usize::from(header.AceSize) < offset + 8 {
            return Err(Error::Invalid("unsupported original control ACL"));
        }
        let sid = unsafe { ace.cast::<u8>().add(offset) };
        let count = unsafe { *sid.add(1) } as usize;
        if count > 15
            || offset + 8 + 4 * count > usize::from(header.AceSize)
            || unsafe { IsValidSid(sid.cast()) } == 0
        {
            return Err(Error::Invalid("invalid original control ACL"));
        }
        let trusted = unsafe {
            EqualSid(sid.cast(), owner) != 0 || IsWellKnownSid(sid.cast(), WinLocalSystemSid) != 0
        };
        let mask = unsafe {
            *ace.cast::<u8>()
                .add(std::mem::size_of::<ACE_HEADER>())
                .cast::<u32>()
        };
        if !trusted && (!directory || mask & (0x0000_0156 | 0x000d_0000 | 0x5000_0000) != 0) {
            return Err(Error::Invalid(
                "original control ACL grants foreign mutation",
            ));
        }
    }
    Ok(())
}
fn no_impersonation() -> Result<()> {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } != 0 {
        unsafe {
            CloseHandle(token);
        }
        return Err(Error::Invalid(
            "profile reconciliation must use the original process account",
        ));
    }
    if io::Error::last_os_error().raw_os_error() != Some(ERROR_NO_TOKEN as i32) {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
fn contents(root: &Path, allow_closed: bool) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        if name != "identity.json"
            && name != "session.lock"
            && !(allow_closed && (name == "closed" || name == "operator-retired.json"))
        {
            return Err(Error::Invalid(
                "capsule is not in the original pre-ownership phase",
            ));
        }
    }
    Ok(())
}
fn profile_tree(path: &Path) -> Result<(BTreeMap<String, ProfileEntry>, Vec<File>)> {
    // DeleteAppContainerProfile owns the package storage, not the capsule VHD.
    // Bind the whole default profile tree and reject reparse/hardlink surprises.
    if !local_absolute(path) || path.file_name() != Some(std::ffi::OsStr::new("AC")) {
        return Err(Error::Invalid("unsupported original profile storage"));
    }
    let package = path
        .parent()
        .ok_or(Error::Invalid("profile package parent missing"))?;
    let mut parents = Vec::new();
    for parent in package.ancestors().skip(1) {
        parents.push(open_directory(parent)?);
        if parents.len() > 32 {
            return Err(Error::Invalid("profile namespace is too deep"));
        }
    }
    let mut tree = BTreeMap::new();
    let mut handles = Vec::new();
    let mut total_bytes = 0u64;
    let mut queue = vec![(package.to_path_buf(), String::new(), 0)];
    while let Some((path, relative, depth)) = queue.pop() {
        if tree.len() >= 256 || depth > 8 {
            return Err(Error::Invalid(
                "orphan profile exceeds the inspection limit",
            ));
        }
        let metadata = fs::symlink_metadata(&path)?;
        let file = if metadata.is_dir() {
            open_directory(&path)?
        } else {
            let file = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&path)?;
            ordinary_file(&file, 32 * 1024 * 1024)?;
            file
        };
        let size = if metadata.is_dir() {
            0
        } else {
            file.metadata()?.len()
        };
        total_bytes = total_bytes
            .checked_add(size)
            .ok_or(Error::Invalid("orphan profile is oversized"))?;
        if total_bytes > 32 * 1024 * 1024 {
            return Err(Error::Invalid("orphan profile is oversized"));
        }
        let content_sha256 = if metadata.is_dir() {
            None
        } else {
            let mut bytes = Vec::new();
            (&file).take(size + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 != size {
                return Err(Error::Invalid("original profile bytes changed"));
            }
            Some(digest(&bytes))
        };
        tree.insert(
            relative.clone(),
            ProfileEntry {
                identity: native(&file)?,
                directory: metadata.is_dir(),
                size,
                content_sha256,
            },
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(&path)? {
                let entry = entry?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::Invalid("invalid profile entry"))?;
                if name.contains(['/', '\\', ':']) {
                    return Err(Error::Invalid("invalid profile entry"));
                }
                let next = if relative.is_empty() {
                    name
                } else {
                    format!("{relative}/{name}")
                };
                queue.push((entry.path(), next, depth + 1));
            }
        }
        handles.push(file);
    }
    if !tree.contains_key("AC") {
        return Err(Error::Invalid("original profile folder is absent"));
    }
    handles.extend(parents);
    Ok((tree, handles))
}
impl OrphanInspection {
    /// Caller must hold its original private parent namespace/runner ownership.
    /// A mounted VHD is admitted only by the separate exact-volume runner adapter.
    pub fn open(root: &Path) -> Result<Self> {
        no_impersonation()?;
        if !local_absolute(root) {
            return Err(Error::Invalid("absolute local original capsule required"));
        }
        let directory = open_directory(root)?;
        private_control(&directory, true)?;
        let root = fs::canonicalize(root)?;
        let root = root.as_path();
        if native(&open_directory(root)?)? != native(&directory)? {
            return Err(Error::Invalid("original capsule namespace changed"));
        }
        let _lock = lock(root)?;
        contents(root, false)?;
        let journal = OpenOptions::new()
            .read(true)
            .access_mode(GENERIC_READ | READ_CONTROL)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(root.join("identity.json"))?;
        ordinary_file(&journal, 1024)?;
        private_control(&journal, false)?;
        let mut bytes = Vec::new();
        (&journal).take(1025).read_to_end(&mut bytes)?;
        let value: Journal = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        let suffix = value
            .profile
            .strip_prefix("LocalAI.Cowork.V2.")
            .ok_or(Error::Invalid("foreign profile intent"))?;
        if value.version != 1
            || suffix.len() != 32
            || !suffix
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || root.file_name().and_then(|v| v.to_str())
                != Some(format!("capsule-{suffix}").as_str())
        {
            return Err(Error::Invalid(
                "original profile intent does not name this capsule",
            ));
        }
        let gate = identity::profile_gate(&value.profile)?;
        identity::ensure_job_closed(&value.profile)?;
        let (sid, profile_directory) = identity::observed_registration(&value.profile)?;
        let (tree, profile_handles) = match &profile_directory {
            Some(path) => {
                if !path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|v| v.to_str())
                    .is_some_and(|v| v.eq_ignore_ascii_case(&value.profile))
                {
                    return Err(Error::Invalid(
                        "native profile namespace does not match the original intent",
                    ));
                }
                profile_tree(path)?
            }
            None => (BTreeMap::new(), Vec::new()),
        };
        let snapshot = OrphanSnapshot {
            format: 1,
            root: root.to_path_buf(),
            root_identity: native(&directory)?,
            original_journal_identity: native(&journal)?,
            original_journal_sha256: digest(&bytes),
            owner_sid: files::owner_sid()?,
            profile: value.profile,
            profile_sid: sid,
            profile_directory,
            original_profile_tree: tree,
        };
        if serde_json::to_vec(&snapshot)
            .map_err(io::Error::other)?
            .len()
            > 64 * 1024
        {
            return Err(Error::Invalid(
                "original profile proof exceeds the operator record limit",
            ));
        }
        Ok(Self {
            root: directory,
            journal,
            _lock,
            _gate: gate,
            profile_handles,
            snapshot,
        })
    }
    pub fn retire(mut self, expected: &OrphanSnapshot) -> Result<OrphanRetirement> {
        no_impersonation()?;
        if &self.snapshot != expected
            || native(&self.root)? != expected.root_identity
            || native(&self.journal)? != expected.original_journal_identity
            || files::owner_sid()? != expected.owner_sid
        {
            return Err(Error::Invalid(
                "operator plan does not match the original physical objects",
            ));
        }
        contents(&expected.root, false)?;
        identity::ensure_job_closed(&expected.profile)?;
        // Product profile mutation is serialized by _gate. The trusted operator
        // account must not externally replace OS profile storage during retirement.
        // Windows' deletion API requires closing its profile-storage handles.
        self.profile_handles.clear();
        if let Some(path) = &expected.profile_directory {
            let (tree, check_handles) = profile_tree(path)?;
            if tree != expected.original_profile_tree {
                return Err(Error::Invalid("original profile storage changed"));
            }
            drop(check_handles);
            identity::recover_profile(&expected.profile)?;
            if !matches!(fs::symlink_metadata(path), Err(ref e) if e.kind() == io::ErrorKind::NotFound)
            {
                return Err(Error::Invalid(
                    "original profile storage removal remains unconfirmed",
                ));
            }
        } else if !expected.original_profile_tree.is_empty() {
            return Err(Error::Invalid("invalid original absence-only inspection"));
        }
        let (sid, registration) = identity::observed_registration(&expected.profile)?;
        if sid != expected.profile_sid || registration.is_some() {
            return Err(Error::Invalid(
                "original profile registration removal remains unconfirmed",
            ));
        }
        identity::ensure_job_closed(&expected.profile)?;
        let result = OrphanRetirement {
            format: 1,
            original: expected.clone(),
            profile_storage_absent: expected.profile_directory.is_some(),
            job_quiescent: true,
            capsule_closed: true,
            execution_adopted: false,
        };
        write_marker(
            &expected.root,
            "operator-retired.json",
            &serde_json::to_vec(&result).map_err(io::Error::other)?,
        )?;
        mark_closed(&expected.root)?;
        Ok(result)
    }

    /// Called only for an already persisted original operator intent. This never
    /// invokes deletion again: a reappeared/replaced profile stays unresolved.
    pub fn reconcile_absent(expected: &OrphanSnapshot) -> Result<OrphanRetirement> {
        no_impersonation()?;
        let root = open_directory(&expected.root)?;
        private_control(&root, true)?;
        let _lock = lock(&expected.root)?;
        contents(&expected.root, true)?;
        let journal = OpenOptions::new()
            .read(true)
            .access_mode(GENERIC_READ | READ_CONTROL)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(expected.root.join("identity.json"))?;
        ordinary_file(&journal, 1024)?;
        private_control(&journal, false)?;
        let mut bytes = Vec::new();
        (&journal).take(1025).read_to_end(&mut bytes)?;
        let origin: Journal = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if expected.format != 1
            || native(&root)? != expected.root_identity
            || native(&journal)? != expected.original_journal_identity
            || digest(&bytes) != expected.original_journal_sha256
            || origin.version != 1
            || origin.profile != expected.profile
            || files::owner_sid()? != expected.owner_sid
        {
            return Err(Error::Invalid("original reconciliation objects changed"));
        }
        let _gate = identity::profile_gate(&expected.profile)?;
        let (sid, registration) = identity::observed_registration(&expected.profile)?;
        if sid != expected.profile_sid || registration.is_some() {
            return Err(Error::Invalid("original reconciliation identity changed"));
        }
        identity::ensure_job_closed(&expected.profile)?;
        if let Some(folder) = &expected.profile_directory {
            if !matches!(fs::symlink_metadata(folder), Err(ref e) if e.kind() == io::ErrorKind::NotFound)
            {
                return Err(Error::Invalid(
                    "profile is present; no deletion replay is permitted",
                ));
            }
        } else if !expected.original_profile_tree.is_empty() {
            return Err(Error::Invalid("invalid original absence-only inspection"));
        }
        let result = OrphanRetirement {
            format: 1,
            original: expected.clone(),
            profile_storage_absent: expected.profile_directory.is_some(),
            job_quiescent: true,
            capsule_closed: true,
            execution_adopted: false,
        };
        match read_marker(&expected.root, "operator-retired.json", 128 * 1024) {
            Ok(bytes) => {
                let original: OrphanRetirement =
                    serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                if original != result {
                    return Err(Error::Invalid("original retirement receipt changed"));
                }
            }
            Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
                write_marker(
                    &expected.root,
                    "operator-retired.json",
                    &serde_json::to_vec(&result).map_err(io::Error::other)?,
                )?;
            }
            Err(e) => return Err(e),
        }
        mark_closed(&expected.root)?;
        Ok(result)
    }
}

fn local_absolute(path: &Path) -> bool {
    path.is_absolute()
        && matches!(path.components().next(), Some(Component::Prefix(prefix))
        if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)))
}

pub(super) fn recover_confirmed(root: &Path, profile: &str) -> Result<()> {
    let receipt: OrphanRetirement =
        serde_json::from_slice(&read_marker(root, "operator-retired.json", 128 * 1024)?)
            .map_err(io::Error::other)?;
    if receipt.original.root != root
        || receipt.original.profile != profile
        || !receipt.valid_closure()
    {
        return Err(Error::Invalid("original operator retirement is invalid"));
    }
    // The caller holds the capsule lock; inspect the already closed objects here
    // without reconstructing a capsule or acquiring that lock recursively.
    let directory = open_directory(root)?;
    private_control(&directory, true)?;
    let journal = OpenOptions::new()
        .read(true)
        .access_mode(GENERIC_READ | READ_CONTROL)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(root.join("identity.json"))?;
    private_control(&journal, false)?;
    ordinary_file(&journal, 1024)?;
    if native(&directory)? != receipt.original.root_identity
        || files::owner_sid()? != receipt.original.owner_sid
        || native(&journal)? != receipt.original.original_journal_identity
        || digest(&read_marker(root, "identity.json", 1024)?)
            != receipt.original.original_journal_sha256
        || read_marker(root, "closed", 128)? != CLOSED
    {
        return Err(Error::Invalid(
            "original operator closure no longer matches",
        ));
    }
    let _gate = identity::profile_gate(profile)?;
    identity::ensure_job_closed(profile)?;
    let (sid, registration) = identity::observed_registration(profile)?;
    if sid != receipt.original.profile_sid
        || registration.is_some()
        || receipt.original.profile_directory.as_ref().is_some_and(|folder|
            !matches!(fs::symlink_metadata(folder), Err(ref e) if e.kind() == io::ErrorKind::NotFound))
    {
        return Err(Error::Invalid("retired original profile is present again"));
    }
    Ok(())
}

/// The caller has verified the exact VHD/volume parent. Validate this already-
/// opened capsule and marker instead of rejecting its legitimate mount ancestor.
pub fn retired_marker(root: &Path) -> Result<OrphanRetirement> {
    let directory = open_directory(root)?;
    private_control(&directory, true)?;
    let _lock = lock(root)?;
    let marker = OpenOptions::new()
        .read(true)
        .access_mode(GENERIC_READ | READ_CONTROL)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(root.join("operator-retired.json"))?;
    ordinary_file(&marker, 128 * 1024)?;
    native(&marker)?;
    private_control(&marker, false)?;
    let mut bytes = Vec::new();
    (&marker).take(128 * 1024 + 1).read_to_end(&mut bytes)?;
    let receipt: OrphanRetirement = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if !receipt.valid_closure()
        || native(&directory)? != receipt.original.root_identity
        || root.canonicalize()? != receipt.original.root
        || files::owner_sid()? != receipt.original.owner_sid
    {
        return Err(Error::Invalid("original native retirement marker differs"));
    }
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_orphan_inspection_retirement_and_lost_reply_are_native_and_never_launch() {
        // Journal-before-OSCreate is a legitimate original pre-ownership phase.
        // Native missing-registration proof may be explicitly settled, with no
        // deletion and no fabricated observation of an unknown storage folder.
        {
            let parent = tempfile::tempdir().unwrap();
            let suffix = uuid::Uuid::new_v4().simple().to_string();
            let root = fs::canonicalize(parent.path())
                .unwrap()
                .join(format!("capsule-{suffix}"));
            fs::create_dir(&root).unwrap();
            files::owner_only(&root).unwrap();
            let profile = format!("LocalAI.Cowork.V2.{suffix}");
            write_marker(
                &root,
                "identity.json",
                &serde_json::to_vec(&Journal {
                    version: 1,
                    profile: profile.clone(),
                })
                .unwrap(),
            )
            .unwrap();
            let inspection = OrphanInspection::open(&root).unwrap();
            let original = inspection.snapshot.clone();
            assert!(
                original.profile_directory.is_none() && original.original_profile_tree.is_empty()
            );
            let result = inspection.retire(&original).unwrap();
            assert!(
                result.valid_closure()
                    && !result.profile_storage_absent
                    && !result.execution_adopted
            );
            assert_eq!(retired_marker(&root).unwrap(), result);
            assert_eq!(
                OrphanInspection::reconcile_absent(&original).unwrap(),
                result
            );
            super::super::recover(&root).unwrap();
            assert!(!root.join("owned").exists());
        }
        let parent = tempfile::tempdir().unwrap();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let root = fs::canonicalize(parent.path())
            .unwrap()
            .join(format!("capsule-{suffix}"));
        fs::create_dir(&root).unwrap();
        files::owner_only(&root).unwrap();
        let original_owner = lock(&root).unwrap();
        let profile = format!("LocalAI.Cowork.V2.{suffix}");
        write_marker(
            &root,
            "identity.json",
            &serde_json::to_vec(&Journal {
                version: 1,
                profile: profile.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        let mut identity = Identity::create(&profile).unwrap();
        let folder = PathBuf::from(identity.profile_folder().unwrap());
        assert!(folder.is_dir());
        assert!(
            OrphanInspection::open(&root).is_err(),
            "a live original capsule lock was adopted"
        );
        identity.close_own_preownership_fixture_handles().unwrap();
        drop(original_owner);
        let inspection = OrphanInspection::open(&root).unwrap();
        let mut wrong = inspection.snapshot.clone();
        wrong.profile_sid.push('0');
        assert!(
            fs::write(root.join("identity.json"), b"changed").is_err(),
            "original journal was writable while held"
        );
        assert!(
            fs::rename(&root, parent.path().join("foreign-name")).is_err(),
            "original root was renameable while held"
        );
        assert!(inspection.retire(&wrong).is_err());
        assert!(
            folder.is_dir(),
            "a mismatched operator plan deleted a profile"
        );
        let inspection = OrphanInspection::open(&root).unwrap();
        let original = inspection.snapshot.clone();
        let result = inspection.retire(&original).unwrap();
        assert_eq!(result.original, original);
        assert!(result.profile_storage_absent && result.job_quiescent && !result.execution_adopted);
        assert!(!folder.exists());
        assert!(
            !root.join("owned").exists()
                && !root.join("work").exists()
                && !root.join("bundle").exists()
        );
        let before = fs::read(root.join("operator-retired.json")).unwrap();
        assert_eq!(retired_marker(&root).unwrap(), result);
        assert_eq!(
            OrphanInspection::reconcile_absent(&original).unwrap(),
            result
        );
        super::super::recover(&root).unwrap();
        assert_eq!(
            fs::read(root.join("operator-retired.json")).unwrap(),
            before
        );
        // A fresh profile under the old name is a different effect. Its physical
        // existence prevents even an absence-only replay of the old operation.
        let mut replacement = Identity::create(&profile).unwrap();
        assert!(OrphanInspection::reconcile_absent(&original).is_err());
        assert!(super::super::recover(&root).is_err());
        assert!(PathBuf::from(replacement.profile_folder().unwrap()).is_dir());
        replacement.close().unwrap();
        identity.close().unwrap();
    }
}
