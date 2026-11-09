//! A private document launch spelling bound to the original directory object.
//! A revision digest describes this binding; only original native handles establish it.
use super::*;
#[cfg(all(test, windows))]
mod native_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity(u64, [u8; 16]);

pub(super) struct DocumentRoot {
    original: PathBuf,
    launch: PathBuf,
    directory: fs::File,
    identity: Identity,
    invalidated: AtomicBool,
    pub(super) source: Option<Arc<DocumentRoot>>,
    _custody: Option<Arc<dyn Send + Sync>>,
    // Keep the exact external-workspace lock through its last live reference.
    _workspace_owner: Option<fs::File>,
}

fn directory(path: &Path) -> Result<fs::File> {
    directory_with_access(path, false)
}

fn directory_with_access(path: &Path, _metadata_only: bool) -> Result<fs::File> {
    namespace(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES;
        // Keep this original directory object; refuse deletion/rename while held.
        if _metadata_only {
            options.access_mode(FILE_READ_ATTRIBUTES);
        }
        options
            .share_mode(3)
            .custom_flags(0x0200_0000 | 0x0020_0000);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_dir() {
        return Err(invalid("The private document directory changed"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if file.metadata()?.file_attributes() & 0x400 != 0 {
            return Err(invalid(
                "The private document directory contains a reparse point",
            ));
        }
    }
    Ok(file)
}

fn identity(file: &fs::File) -> Result<Identity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok(Identity(
            metadata.dev(),
            u128::from(metadata.ino()).to_le_bytes(),
        ))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
        };
        let mut info = FILE_ID_INFO::default();
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&mut info as *mut FILE_ID_INFO).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Identity(info.VolumeSerialNumber, info.FileId.Identifier))
    }
}

#[cfg(windows)]
fn conventional(path: &Path) -> PathBuf {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if !wide.starts_with(&[92, 92, 63, 92]) {
        return path.into();
    }
    let rest = &wide[4..];
    let value = if rest.starts_with(&[85, 78, 67, 92]) {
        [vec![92, 92], rest[4..].to_vec()].concat()
    } else if rest.get(1) == Some(&58) && rest.get(2) == Some(&92) {
        rest.to_vec()
    } else {
        return path.into();
    };
    std::ffi::OsString::from_wide(&value).into()
}
#[cfg(not(windows))]
fn conventional(path: &Path) -> PathBuf {
    path.into()
}

fn all_budgets_fit(root: &Path) -> bool {
    document_working_directory_fits(root)
        && latex_working_directories_fit(root)
        && office_profile_path_fits(root)
}

fn reject_overlap(
    source: &DocumentRoot,
    workspace: &Path,
    workspace_identity: Identity,
) -> Result<()> {
    // Path strings locate fresh handles; original object identities decide overlap.
    // Check both directions before acquiring a workspace lock or changing any path.
    for ancestor in workspace.ancestors() {
        if identity(&directory_with_access(ancestor, true)?)? == source.identity {
            return Err(invalid(
                "Document workspace must be outside the original data directory",
            ));
        }
    }
    for ancestor in source.original.ancestors() {
        if identity(&directory_with_access(ancestor, true)?)? == workspace_identity {
            return Err(invalid(
                "Original data directory must be outside the document workspace",
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn short_spelling(root: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetShortPathNameW(long: *const u16, short: *mut u16, capacity: u32) -> u32;
    }
    // std canonicalization supplies the extended input spelling for long Win32 paths.
    let input = fs::canonicalize(root)
        .ok()?
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let capacity = unsafe { GetShortPathNameW(input.as_ptr(), std::ptr::null_mut(), 0) };
    if capacity == 0 || capacity > 32_768 {
        return None;
    }
    let mut output = vec![0u16; capacity as usize];
    let count = unsafe { GetShortPathNameW(input.as_ptr(), output.as_mut_ptr(), capacity) };
    if count == 0 || count >= capacity {
        return None;
    }
    output.truncate(count as usize);
    Some(conventional(&PathBuf::from(std::ffi::OsString::from_wide(
        &output,
    ))))
}
#[cfg(not(windows))]
fn short_spelling(_: &Path) -> Option<PathBuf> {
    None
}

fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
}

impl DocumentRoot {
    #[cfg(all(test, windows))]
    pub(super) fn without_short_spelling(root: &Path) -> Result<Arc<Self>> {
        Self::select(root, directory(root)?, None)
    }
    /// Called only after Toolbox::open acquired its original exclusive root lease.
    pub(super) fn open(root: &Path) -> Result<Arc<Self>> {
        let file = directory(root)?;
        let default = conventional(root);
        let candidate = if all_budgets_fit(&default) {
            None
        } else {
            short_spelling(root)
        };
        Self::select(root, file, candidate)
    }

    fn select(root: &Path, file: fs::File, candidate: Option<PathBuf>) -> Result<Arc<Self>> {
        let original_identity = identity(&file)?;
        let mut launch = conventional(root);
        if identity(&directory(&launch)?)? != original_identity {
            return Err(invalid("The private document directory changed"));
        }
        if let Some(candidate) = candidate {
            if !candidate.is_absolute() || identity(&directory(&candidate)?)? != original_identity {
                return Err(invalid(
                    "The document launch spelling changes its original directory",
                ));
            }
            if path_bytes(&candidate).len() < path_bytes(&launch).len() {
                launch = candidate;
            }
        }
        let binding = Arc::new(Self {
            original: root.into(),
            launch,
            directory: file,
            identity: original_identity,
            invalidated: AtomicBool::new(false),
            source: None,
            _custody: None,
            _workspace_owner: None,
        });
        binding.verify()?;
        Ok(binding)
    }

    pub(super) fn workspace(
        source: Arc<Self>,
        root: &Path,
        custody: Arc<dyn Send + Sync>,
    ) -> Result<Arc<Self>> {
        source.verify()?;
        if !root.is_absolute() {
            return Err(invalid("Document workspace must be absolute"));
        }
        // Native caller must create and privately own this directory first.
        // In particular, do not create/chmod a rejected ancestor of user data.
        let selected_directory = directory(root)?;
        // Office expands aliases internally. Require a physically short directory.
        let physical = conventional(&fs::canonicalize(root)?);
        if !all_budgets_fit(&physical) {
            return Err(invalid("Document workspace physical path is too long"));
        }
        let original_directory = directory(&physical)?;
        let workspace_identity = identity(&original_directory)?;
        if identity(&selected_directory)? != workspace_identity {
            return Err(invalid("The selected document workspace changed"));
        }
        reject_overlap(&source, &physical, workspace_identity)?;
        source.verify()?;
        let owner = root_owner::acquire(&physical)?;
        let mut binding = Self::select(&physical, original_directory, None)?;
        let fields = Arc::get_mut(&mut binding).expect("new document binding has one owner");
        fields.source = Some(source);
        fields._custody = Some(custody);
        fields._workspace_owner = Some(owner);
        binding.verify()?;
        Ok(binding)
    }

    pub(super) fn verify(&self) -> Result<()> {
        if self.invalidated.load(Ordering::Acquire) {
            return Err(invalid(
                "The private document directory changed; restart and prepare a new action",
            ));
        }
        let unchanged = (|| -> Result<bool> {
            if let Some(source) = &self.source {
                source.verify()?;
            }
            Ok(identity(&self.directory)? == self.identity
                && identity(&directory(&self.original)?)? == self.identity
                && identity(&directory(&self.launch)?)? == self.identity)
        })()
        .unwrap_or(false);
        if !unchanged {
            self.invalidated.store(true, Ordering::Release);
            return Err(invalid(
                "The private document directory changed; restart and prepare a new action",
            ));
        }
        Ok(())
    }

    pub(super) fn path(&self) -> Result<&Path> {
        self.verify()?;
        Ok(&self.launch)
    }

    pub(super) fn physical_path_fits(&self) -> Result<bool> {
        self.verify()?;
        let physical = conventional(&fs::canonicalize(&self.original)?);
        if identity(&directory(&physical)?)? != self.identity {
            return Err(invalid("The original document directory changed"));
        }
        Ok(all_budgets_fit(&physical))
    }

    pub(super) fn revision(&self) -> Value {
        let file_id = self
            .identity
            .1
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        json!([
            "document-root/2",
            path_bytes(&self.original),
            path_bytes(&self.launch),
            format!("{:016x}:{file_id}", self.identity.0),
            self.verify().is_ok(),
            self.source.as_ref().map(|source| source.revision())
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Scratch {
        path: PathBuf,
        original: PathBuf,
    }
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("cdr-{}", Uuid::new_v4().simple()));
            fs::create_dir(&path).unwrap();
            let original = fs::canonicalize(&path).unwrap();
            Self { path, original }
        }
        fn root(&self, name: &str) -> PathBuf {
            let root = self.path.join(name);
            fs::create_dir(&root).unwrap();
            root
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            if self.path.is_absolute()
                && fs::canonicalize(&self.path).ok().as_ref() == Some(&self.original)
                && self
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("cdr-")
            {
                let _ = fs::remove_dir_all(&self.original);
            }
        }
    }
    #[test]
    fn original_directory_is_held_without_relocating_files() {
        let scratch = Scratch::new();
        let root = scratch.root("root");
        fs::write(root.join("original"), b"keep").unwrap();
        let binding = DocumentRoot::open(&root).unwrap();
        assert_eq!(
            identity(&directory(binding.path().unwrap()).unwrap()).unwrap(),
            identity(&directory(&root).unwrap()).unwrap()
        );
        assert_eq!(
            fs::read(binding.path().unwrap().join("original")).unwrap(),
            b"keep"
        );
    }
    #[test]
    fn identical_copied_bytes_do_not_authorize_a_different_directory() {
        let scratch = Scratch::new();
        let root = scratch.root("root");
        let other = scratch.root("other");
        for path in [&root, &other] {
            fs::write(path.join("same"), b"same bytes").unwrap();
        }
        assert!(DocumentRoot::select(&root, directory(&root).unwrap(), Some(other)).is_err());
    }
    #[test]
    fn absent_short_spelling_keeps_the_original_path_and_budgets() {
        let scratch = Scratch::new();
        let root = scratch.root("root");
        let binding = DocumentRoot::select(&root, directory(&root).unwrap(), None).unwrap();
        assert_eq!(binding.path().unwrap(), conventional(&root));
        assert_eq!(
            all_budgets_fit(binding.path().unwrap()),
            all_budgets_fit(&conventional(&root))
        );
    }
    #[test]
    fn root_revision_distinguishes_original_objects() {
        let scratch = Scratch::new();
        let one = scratch.root("one");
        let two = scratch.root("two");
        assert_ne!(
            DocumentRoot::open(&one).unwrap().revision(),
            DocumentRoot::open(&two).unwrap().revision()
        );
    }
    fn config(root: PathBuf) -> Config {
        Config {
            root,
            programs: BTreeMap::new(),
            environment: BTreeMap::new(),
            github_token: None,
        }
    }
    fn tree(path: &Path) -> BTreeMap<PathBuf, (bool, Vec<u8>, u32)> {
        fn visit(root: &Path, path: &Path, rows: &mut BTreeMap<PathBuf, (bool, Vec<u8>, u32)>) {
            let metadata = fs::metadata(path).unwrap();
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode()
            };
            #[cfg(not(unix))]
            let mode = 0;
            let bytes = if metadata.is_file() {
                if path.file_name().unwrap() == ".toolbox-owner.lock" {
                    // Windows byte-range locking prevents reads, not metadata inspection.
                    format!(
                        "{:?}:{}",
                        identity(&fs::File::open(path).unwrap()).unwrap(),
                        metadata.len()
                    )
                    .into_bytes()
                } else {
                    fs::read(path).unwrap()
                }
            } else {
                Vec::new()
            };
            rows.insert(
                path.strip_prefix(root).unwrap().into(),
                (metadata.is_dir(), bytes, mode),
            );
            if metadata.is_dir() {
                for entry in fs::read_dir(path).unwrap() {
                    visit(root, &entry.unwrap().path(), rows);
                }
            }
        }
        let mut rows = BTreeMap::new();
        visit(path, path, &mut rows);
        rows
    }
    fn reject_overlap_without_changes(workspace_inside_source: bool) {
        let scratch = Scratch::new();
        let outer = scratch.root("outer");
        let (mut owner, workspace) = if workspace_inside_source {
            let owner = Toolbox::open(config(outer.clone())).unwrap();
            let workspace = outer.join("w");
            fs::create_dir(&workspace).unwrap();
            (owner, workspace)
        } else {
            let source = outer.join("document-stages/data");
            fs::create_dir_all(&source).unwrap();
            (Toolbox::open(config(source)).unwrap(), outer.clone())
        };
        fs::write(
            owner.config.root.join("original-source.txt"),
            b"original source must survive",
        )
        .unwrap();
        fs::write(
            workspace.join("workspace-marker.txt"),
            b"caller-owned marker must survive",
        )
        .unwrap();
        assert!(
            all_budgets_fit(&conventional(&fs::canonicalize(&workspace).unwrap())),
            "the overlap test must not pass because of an unrelated path-budget rejection"
        );
        let before = tree(&outer);
        assert!(!workspace.join(".toolbox-owner.lock").exists());
        let result = Arc::get_mut(&mut owner)
            .unwrap()
            .configure_document_workspace(
                workspace.clone(),
                Arc::new(directory(&scratch.path).unwrap()),
            );
        assert!(
            result.is_err(),
            "overlapping roots must be rejected before installation"
        );
        assert_eq!(
            tree(&outer),
            before,
            "rejected selection must not create, clean or chmod any path"
        );
        assert!(!workspace.join(".toolbox-owner.lock").exists());
        assert!(owner.documents.root.source.is_none());
    }
    #[test]
    fn source_inside_workspace_is_rejected_before_any_mutation() {
        reject_overlap_without_changes(false);
    }
    #[test]
    fn workspace_inside_source_is_rejected_before_any_mutation() {
        reject_overlap_without_changes(true);
    }
    #[test]
    fn missing_workspace_is_rejected_without_creating_it() {
        let scratch = Scratch::new();
        let original = scratch.root("data");
        let workspace = scratch.path.join("missing");
        let mut owner = Toolbox::open(config(original)).unwrap();
        let before = tree(&scratch.path);
        assert!(
            Arc::get_mut(&mut owner)
                .unwrap()
                .configure_document_workspace(
                    workspace,
                    Arc::new(directory(&scratch.path).unwrap())
                )
                .is_err()
        );
        assert_eq!(tree(&scratch.path), before);
    }
    #[test]
    fn separate_workspace_has_a_probe_home_and_is_selected_only_once() {
        let scratch = Scratch::new();
        let original = scratch.root("data");
        let workspace = scratch.root("work");
        let mut owner = Toolbox::open(config(original.clone())).unwrap();
        Arc::get_mut(&mut owner)
            .unwrap()
            .configure_document_workspace(
                workspace.clone(),
                Arc::new(directory(&scratch.path).unwrap()),
            )
            .unwrap();
        assert!(workspace.join("home").is_dir());
        assert_eq!(
            owner.documents.root.source.as_ref().unwrap().identity,
            identity(&directory(&original).unwrap()).unwrap()
        );
        assert!(
            Arc::get_mut(&mut owner)
                .unwrap()
                .configure_document_workspace(
                    workspace,
                    Arc::new(directory(&scratch.path).unwrap())
                )
                .is_err()
        );
    }
    #[test]
    fn different_sources_cannot_share_one_live_workspace() {
        let scratch = Scratch::new();
        let one = scratch.root("one");
        let two = scratch.root("two");
        let workspace = scratch.root("work");
        let mut first = Toolbox::open(config(one)).unwrap();
        let mut second = Toolbox::open(config(two)).unwrap();
        Arc::get_mut(&mut first)
            .unwrap()
            .configure_document_workspace(
                workspace.clone(),
                Arc::new(directory(&scratch.path).unwrap()),
            )
            .unwrap();
        let sentinel = workspace.join("document-stages/sentinel");
        fs::write(&sentinel, b"active").unwrap();
        assert!(matches!(
            Arc::get_mut(&mut second)
                .unwrap()
                .configure_document_workspace(
                    workspace.clone(),
                    Arc::new(directory(&scratch.path).unwrap())
                ),
            Err(Error::RootInUse)
        ));
        assert_eq!(fs::read(sentinel).unwrap(), b"active");
        drop(first);
        Arc::get_mut(&mut second)
            .unwrap()
            .configure_document_workspace(workspace, Arc::new(directory(&scratch.path).unwrap()))
            .unwrap();
    }
    #[test]
    fn prior_document_revision_is_rejected_after_startup_workspace_selection() {
        let scratch = Scratch::new();
        let original = scratch.root("data");
        let workspace = scratch.root("work");
        let mut owner = Toolbox::open(config(original)).unwrap();
        let prior = owner.document_engine_revision();
        Arc::get_mut(&mut owner)
            .unwrap()
            .configure_document_workspace(
                workspace.clone(),
                Arc::new(directory(&scratch.path).unwrap()),
            )
            .unwrap();
        assert_ne!(owner.document_engine_revision(), prior);
        let result = owner.execute_document(
            DocumentJob {
                engine_revision: Some(prior),
                tool: "document_read".into(),
                operation: "text".into(),
                inputs: vec![],
                output_name: None,
                output_directory: false,
                spec: Value::Null,
                options: Value::Null,
                timeout: Duration::from_secs(10),
            },
            Arc::new(AtomicBool::new(false)),
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("reviewed document runtime changed")
        );
        assert!(owner.imports.lock().unwrap().is_empty());
        assert_eq!(
            fs::read_dir(workspace.join("document-stages"))
                .unwrap()
                .count(),
            0
        );
        assert!(!owner.documents.state.lock().unwrap().probing);
    }
    #[test]
    fn a_lost_launch_spelling_stays_invalid_after_restoration() {
        let scratch = Scratch::new();
        let root = scratch.root("root");
        let other = scratch.root("other");
        let mut binding = DocumentRoot::open(&root).unwrap();
        let original_revision = binding.revision();
        let original_spelling = binding.path().unwrap().to_path_buf();
        // Model an observed name change with two actual, distinct own directory objects.
        Arc::get_mut(&mut binding).unwrap().launch = other;
        assert!(binding.path().is_err());
        assert_ne!(binding.revision(), original_revision);
        Arc::get_mut(&mut binding).unwrap().launch = original_spelling;
        assert!(binding.path().is_err());
    }
    #[cfg(windows)]
    #[test]
    fn the_last_original_directory_reference_prevents_rename() {
        let scratch = Scratch::new();
        let root = scratch.root("root");
        let moved = scratch.path.join("moved");
        let binding = DocumentRoot::open(&root).unwrap();
        let retained = Arc::clone(&binding);
        drop(binding);
        assert!(fs::rename(&root, &moved).is_err());
        assert!(retained.path().is_ok());
        drop(retained);
        fs::rename(&root, &moved).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn observed_replacement_invalidates_the_original_binding_permanently() {
        let scratch = Scratch::new();
        let root = scratch.root("root");
        let moved = scratch.path.join("moved");
        let binding = DocumentRoot::open(&root).unwrap();
        let original_revision = binding.revision();
        fs::rename(&root, &moved).unwrap();
        fs::create_dir(&root).unwrap();
        assert!(binding.path().is_err());
        assert_ne!(binding.revision(), original_revision);
        fs::remove_dir(&root).unwrap();
        fs::rename(&moved, &root).unwrap();
        assert!(binding.path().is_err());
    }
}
