use super::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn owner(data: &Path) -> Arc<cowork_toolbox::Toolbox> {
    let _guard = authority_token_file::private_directory(data, None).unwrap();
    let _root_guard = authority_token_file::private_directory(&data.join("toolbox"), None).unwrap();
    cowork_toolbox::Toolbox::open(cowork_toolbox::Config {
        root: data.join("toolbox"),
        programs: BTreeMap::new(),
        environment: BTreeMap::new(),
        github_token: None,
    })
    .unwrap()
}
fn binding(data: &Path) -> Binding {
    let directory = private_existing(&data.join("toolbox"), false).unwrap();
    read_marker::<Binding>(&directory, SOURCE_MARKER).unwrap().0
}
fn slot(base: &Path, data: &Path) -> PathBuf {
    base.join(binding(data).locator)
}

#[test]
fn native_short_profile_does_not_create_a_locator() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let mut toolbox = owner(&data);
    assert!(!toolbox.document_workspace_required().unwrap());
    configure(&data, &data.join("toolbox"), None, &mut toolbox).unwrap();
    assert!(!data.join("toolbox").join(SOURCE_MARKER).exists());
}
#[test]
fn profile_restart_reuses_original_slot_and_revision() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let base = temp.path().join("private/work");
    let mut toolbox = owner(&data);
    let before = toolbox.document_engine_revision();
    configure_at(&data, &data.join("toolbox"), None, &base, &mut toolbox).unwrap();
    let bound = binding(&data);
    let revision = toolbox.document_engine_revision();
    assert_ne!(revision, before);
    let original = fs::read(data.join("toolbox").join(SOURCE_MARKER)).unwrap();
    drop(toolbox);
    let mut reopened = owner(&data);
    configure_at(&data, &data.join("toolbox"), None, &base, &mut reopened).unwrap();
    assert!(same_binding(&bound, &binding(&data)));
    assert_eq!(reopened.document_engine_revision(), revision);
    assert_eq!(
        fs::read(data.join("toolbox").join(SOURCE_MARKER)).unwrap(),
        original
    );
}
#[test]
fn distinct_profiles_keep_separate_live_slots() {
    let temp = tempfile::tempdir().unwrap();
    let one = temp.path().join("one");
    let two = temp.path().join("two");
    let base = temp.path().join("private/work");
    let mut first = owner(&one);
    let mut second = owner(&two);
    configure_at(&one, &one.join("toolbox"), None, &base, &mut first).unwrap();
    let first_slot = slot(&base, &one);
    let sentinel = first_slot.join("document-stages/original");
    fs::write(&sentinel, b"running first profile").unwrap();
    configure_at(&two, &two.join("toolbox"), None, &base, &mut second).unwrap();
    assert_ne!(binding(&one).locator, binding(&two).locator);
    assert_ne!(binding(&one).source, binding(&two).source);
    assert_eq!(fs::read(sentinel).unwrap(), b"running first profile");
}
#[test]
fn copied_profile_is_rejected_before_opening_the_recorded_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let one = temp.path().join("one");
    let two = temp.path().join("two");
    let base = temp.path().join("private/work");
    let mut first = owner(&one);
    configure_at(&one, &one.join("toolbox"), None, &base, &mut first).unwrap();
    let sentinel = slot(&base, &one).join("document-stages/original");
    fs::write(&sentinel, b"original unchanged").unwrap();
    let mut copied = owner(&two);
    fs::copy(
        one.join("toolbox").join(SOURCE_MARKER),
        two.join("toolbox").join(SOURCE_MARKER),
    )
    .unwrap();
    let uncreated = temp.path().join("not-created/work");
    let before = fs::read(two.join("toolbox").join(SOURCE_MARKER)).unwrap();
    assert!(configure_at(&two, &two.join("toolbox"), None, &uncreated, &mut copied).is_err());
    assert!(!uncreated.parent().unwrap().exists());
    assert_eq!(fs::read(sentinel).unwrap(), b"original unchanged");
    assert_eq!(
        fs::read(two.join("toolbox").join(SOURCE_MARKER)).unwrap(),
        before
    );
}
#[test]
fn unknown_private_parent_is_not_adopted_or_changed() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let mut toolbox = owner(&data);
    let parent = temp.path().join("foreign");
    let _foreign = authority_token_file::private_directory(&parent, None).unwrap();
    let sentinel = parent.join("original.txt");
    fs::write(&sentinel, b"unrelated private data").unwrap();
    let before = fs::read_dir(&parent)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert!(
        configure_at(
            &data,
            &data.join("toolbox"),
            None,
            &parent.join("work"),
            &mut toolbox
        )
        .is_err()
    );
    assert_eq!(fs::read(sentinel).unwrap(), b"unrelated private data");
    assert_eq!(
        fs::read_dir(&parent)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>(),
        before
    );
    assert!(!data.join("toolbox").join(SOURCE_MARKER).exists());
}
#[test]
fn source_ancestor_of_private_parent_is_rejected_before_creation() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let mut toolbox = owner(&data);
    let base = data.join("private/work");
    let original = data.join("original.txt");
    fs::write(&original, b"preserved").unwrap();
    assert!(configure_at(&data, &data.join("toolbox"), None, &base, &mut toolbox).is_err());
    assert!(!base.parent().unwrap().exists());
    assert_eq!(fs::read(original).unwrap(), b"preserved");
    assert!(!data.join("toolbox").join(SOURCE_MARKER).exists());
}
#[test]
fn private_parent_ancestor_of_source_is_rejected_without_changes() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("private");
    let data = parent.join("data");
    let mut toolbox = owner(&data);
    let sentinel = parent.join("original.txt");
    fs::write(&sentinel, b"do not touch").unwrap();
    assert!(
        configure_at(
            &data,
            &data.join("toolbox"),
            None,
            &parent.join("work"),
            &mut toolbox
        )
        .is_err()
    );
    assert_eq!(fs::read(sentinel).unwrap(), b"do not touch");
    assert!(!parent.join("work").exists());
    assert!(!parent.join(PARENT_MARKER).exists());
}
#[test]
fn workspace_under_served_web_root_is_rejected_before_creation() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let mut toolbox = owner(&data);
    let web = temp.path().join("web");
    fs::create_dir(&web).unwrap();
    let base = web.join("private/work");
    assert!(
        configure_at(
            &data,
            &data.join("toolbox"),
            Some(&web),
            &base,
            &mut toolbox
        )
        .is_err()
    );
    assert!(!base.parent().unwrap().exists());
    assert!(!data.join("toolbox").join(SOURCE_MARKER).exists());
}
#[test]
fn missing_recorded_slot_is_not_recreated() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let base = temp.path().join("private/work");
    let mut toolbox = owner(&data);
    configure_at(&data, &data.join("toolbox"), None, &base, &mut toolbox).unwrap();
    let slot = slot(&base, &data);
    let before = fs::read(data.join("toolbox").join(SOURCE_MARKER)).unwrap();
    drop(toolbox);
    assert!(slot.starts_with(temp.path()) && slot.parent() == Some(base.as_path()));
    fs::remove_dir_all(&slot).unwrap();
    let mut reopened = owner(&data);
    assert!(configure_at(&data, &data.join("toolbox"), None, &base, &mut reopened).is_err());
    assert!(!slot.exists());
    assert_eq!(
        fs::read(data.join("toolbox").join(SOURCE_MARKER)).unwrap(),
        before
    );
}
#[test]
fn corrupted_slot_marker_is_rejected_before_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let base = temp.path().join("private/work");
    let mut toolbox = owner(&data);
    configure_at(&data, &data.join("toolbox"), None, &base, &mut toolbox).unwrap();
    let slot = slot(&base, &data);
    drop(toolbox);
    let sentinel = slot.join("document-stages/original");
    fs::write(&sentinel, b"unconfirmed original").unwrap();
    fs::write(slot.join(SLOT_MARKER), b"{broken").unwrap();
    let mut reopened = owner(&data);
    assert!(configure_at(&data, &data.join("toolbox"), None, &base, &mut reopened).is_err());
    assert_eq!(fs::read(sentinel).unwrap(), b"unconfirmed original");
    assert_eq!(fs::read(slot.join(SLOT_MARKER)).unwrap(), b"{broken");
}
#[test]
fn marker_publication_never_overwrites_an_existing_original() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("private");
    let directory = authority_token_file::private_directory(&root, None).unwrap();
    let first = ParentMarker {
        format: "first".into(),
        directory: verify_directory(&directory).unwrap(),
    };
    let held = publish_marker(&directory, "original.json", &first).unwrap();
    let bytes = fs::read(root.join("original.json")).unwrap();
    assert!(
        publish_marker(
            &directory,
            "original.json",
            &ParentMarker {
                format: "replacement".into(),
                directory: first.directory
            }
        )
        .is_err()
    );
    assert_eq!(fs::read(root.join("original.json")).unwrap(), bytes);
    drop(held);
}
#[test]
fn concurrent_distinct_profiles_publish_one_complete_parent() {
    let temp = tempfile::tempdir().unwrap();
    let one = temp.path().join("one");
    let two = temp.path().join("two");
    let base = temp.path().join("private/work");
    let first = owner(&one);
    let second = owner(&two);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let run = |data: PathBuf, mut owner: Arc<cowork_toolbox::Toolbox>| {
        let base = base.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            configure_at(&data, &data.join("toolbox"), None, &base, &mut owner).unwrap();
            owner
        })
    };
    let first = run(one.clone(), first);
    let second = run(two.clone(), second);
    let _first = first.join().unwrap();
    let _second = second.join().unwrap();
    assert_ne!(binding(&one).locator, binding(&two).locator);
    load_parent(base.parent().unwrap(), "work").unwrap();
}

#[cfg(windows)]
fn own_directory_acl(file: &File, replace: Option<&str>) -> Vec<u8> {
    use std::{os::windows::io::AsRawHandle, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{Authorization::*, *},
    };
    if let Some(sddl) = replace {
        let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    1,
                    &mut descriptor,
                    ptr::null_mut(),
                )
            },
            0
        );
        let mut present = 0;
        let mut dacl = ptr::null_mut();
        let mut defaulted = 0;
        let parsed = unsafe {
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)
        };
        let result = if parsed != 0 {
            unsafe {
                SetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    dacl,
                    ptr::null_mut(),
                )
            }
        } else {
            u32::MAX
        };
        unsafe { LocalFree(descriptor) };
        assert_ne!(parsed, 0);
        assert_eq!(result, 0);
    }
    let mut descriptor = ptr::null_mut();
    let result = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(result, 0);
    let length = unsafe { GetSecurityDescriptorLength(descriptor) } as usize;
    let bytes = unsafe { std::slice::from_raw_parts(descriptor.cast::<u8>(), length) }.to_vec();
    unsafe { LocalFree(descriptor) };
    bytes
}

#[cfg(windows)]
fn fixture_owner_from_parent(parent: &File, path: &Path) {
    use std::{
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{Authorization::*, *},
    };
    let mut descriptor = ptr::null_mut();
    let mut owner = ptr::null_mut();
    assert_eq!(
        unsafe {
            GetSecurityInfo(
                parent.as_raw_handle(),
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
    let file = OpenOptions::new()
        .access_mode(0x0008_0000)
        .open(path)
        .unwrap();
    let result = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    unsafe { LocalFree(descriptor) };
    assert_eq!(result, 0);
}

#[cfg(windows)]
#[test]
fn broad_native_parent_is_unchanged_and_only_created_children_are_private() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let mut toolbox = owner(&data);
    let native = temp.path().join("native");
    fs::create_dir(&native).unwrap();
    let access = OpenOptions::new()
        .access_mode(0x0006_0080)
        .share_mode(3)
        .custom_flags(0x0220_0000)
        .open(&native)
        .unwrap();
    // Only this newly created synthetic directory is changed, never LocalAppData/TEMP.
    let before = own_directory_acl(
        &access,
        Some("D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;WD)"),
    );
    assert!(authority_token_file::private_directory(&native, None).is_err());
    let base = native.join("private/work");
    configure_at(&data, &data.join("toolbox"), None, &base, &mut toolbox).unwrap();
    assert_eq!(own_directory_acl(&access, None), before);
    let slot = slot(&base, &data);
    let private = private_existing(&slot, true).unwrap();
    strict_directory(&private).unwrap();
    // Core uses ordinary std::fs creation: verify inherited ACLs on actual descendants.
    let descendant = slot.join("home/inherited-private.txt");
    fs::write(&descendant, b"private engine descendant").unwrap();
    // Windows inherits the DACL, but chooses a new object's owner from the token.
    // Normalize only this synthetic child's owner; preserve its inherited DACL.
    fixture_owner_from_parent(private.original_directory(), &descendant);
    super::super::private_secret_file::validate(&File::open(&descendant).unwrap()).unwrap();
    assert_eq!(fs::read(descendant).unwrap(), b"private engine descendant");
}

#[cfg(windows)]
#[test]
fn atomic_directory_creation_retains_original_and_never_opens_existing_child() {
    let temp = tempfile::tempdir().unwrap();
    let parent = native_namespace_parent(temp.path()).unwrap();
    let original = super::super::private_secret_file::create_private_directory_handle(
        parent.original_directory(),
        "created",
    )
    .unwrap();
    let path = temp.path().join("created");
    // The atomic CREATE handle is the only handle to this child at this point.
    assert!(fs::rename(&path, temp.path().join("moved")).is_err());
    let rebound = private_existing(&path, true).unwrap();
    assert_eq!(
        identity(&original).unwrap(),
        verify_directory(&rebound).unwrap()
    );
    let sentinel = path.join("original.txt");
    fs::write(&sentinel, b"original atomic child").unwrap();
    assert_eq!(
        super::super::private_secret_file::create_private_directory_handle(
            parent.original_directory(),
            "created"
        )
        .unwrap_err()
        .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(sentinel).unwrap(), b"original atomic child");
    for invalid in [
        "",
        ".",
        "..",
        "created/.",
        "..\\outside",
        "stream:name",
        "ending.",
        "ending ",
    ] {
        assert_eq!(
            super::super::private_secret_file::create_private_directory_handle(
                parent.original_directory(),
                invalid
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    assert_eq!(
        fs::read_dir(temp.path()).unwrap().count(),
        1,
        "invalid native names must not create another object"
    );
}
