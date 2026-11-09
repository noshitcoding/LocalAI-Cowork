use super::*;
use cowork_managed_runtime::staging::StagingArea;

#[path = "archive.rs"]
mod archive;

fn private_parent() -> tempfile::TempDir {
    let parent = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(windows)]
    set_acl(
        parent.path(),
        "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)",
    );
    parent
}
fn open(path: &std::path::Path) -> Dir {
    Dir::open_ambient_dir(path, ambient_authority()).unwrap()
}

#[test]
fn protected_staging_copies_and_rehashes_all_components_independently_of_source() {
    let f = Fixture::new();
    let source = f.directory();
    let parent = private_parent();
    let area = StagingArea::create(open(parent.path()), "attempt-1", 36).unwrap();
    let mut staged = area
        .stage(&open(source.path()), &f.trust, Platform::LinuxX86_64)
        .unwrap();
    assert_eq!(staged.manifest().components.len(), 9);
    assert_eq!(
        staged.manifest_sha256(),
        sha(&std::fs::read(source.path().join("manifest.json")).unwrap())
    );
    let stage_path = parent.path().join("attempt-1");
    assert_eq!(std::fs::read_dir(&stage_path).unwrap().count(), 11);
    for i in 0..9 {
        assert_eq!(
            std::fs::read(stage_path.join(format!("component-{i}.bin"))).unwrap(),
            b"test"
        );
    }
    std::fs::write(source.path().join("payload/base_rootfs.payload"), b"fail").unwrap();
    drop(source);
    staged.verify().unwrap();
    // Both the original management directory and every payload remain held.
    // No installation, extraction or workload is performed by this fixture.
}

#[test]
fn failed_signature_quota_and_payloads_never_yield_a_staged_capability() {
    let f = Fixture::new();
    let parent = private_parent();
    let source = f.directory();
    let area = StagingArea::create(open(parent.path()), "over-budget", 35).unwrap();
    assert!(
        area.stage(&open(source.path()), &f.trust, Platform::LinuxX86_64)
            .is_err()
    );
    assert_eq!(
        std::fs::read_dir(parent.path().join("over-budget"))
            .unwrap()
            .count(),
        0
    );
    let other = Fixture::new();
    let area = StagingArea::create(open(parent.path()), "bad-signature", 36).unwrap();
    assert!(
        area.stage(&open(source.path()), &other.trust, Platform::LinuxX86_64)
            .is_err()
    );
    assert_eq!(
        std::fs::read_dir(parent.path().join("bad-signature"))
            .unwrap()
            .count(),
        0
    );
    std::fs::write(source.path().join("payload/base_rootfs.payload"), b"fail").unwrap();
    let area = StagingArea::create(open(parent.path()), "bad-payload", 36).unwrap();
    assert!(
        area.stage(&open(source.path()), &f.trust, Platform::LinuxX86_64)
            .is_err()
    );
    // The bounded failed attempt is retained. An existing installation/source
    // is neither removed nor silently replaced, and duplicate names fail.
    assert_eq!(
        std::fs::read_dir(parent.path().join("bad-payload"))
            .unwrap()
            .count(),
        11
    );
    assert!(StagingArea::create(open(parent.path()), "bad-payload", 36).is_err());
    assert_eq!(
        std::fs::read(source.path().join("payload/base_rootfs.payload")).unwrap(),
        b"fail"
    );
}

#[test]
fn extra_members_and_source_hardlinks_are_not_copied_as_trusted_components() {
    let f = Fixture::new();
    let source = f.directory();
    let parent = private_parent();
    std::fs::write(source.path().join("payload/extra"), b"test").unwrap();
    assert!(
        StagingArea::create(open(parent.path()), "extra", 36)
            .unwrap()
            .stage(&open(source.path()), &f.trust, Platform::LinuxX86_64)
            .is_err()
    );
    std::fs::remove_file(source.path().join("payload/extra")).unwrap();
    let link = parent.path().join("foreign-link");
    std::fs::hard_link(source.path().join("payload/base_rootfs.payload"), &link).unwrap();
    assert!(
        StagingArea::create(open(parent.path()), "linked", 36)
            .unwrap()
            .stage(&open(source.path()), &f.trust, Platform::LinuxX86_64)
            .is_err()
    );
    assert_eq!(std::fs::read(&link).unwrap(), b"test");
}

#[test]
fn cli_stages_only_with_explicit_private_parent_and_never_installs() {
    let f = Fixture::new();
    let source = f.directory();
    let parent = private_parent();
    let config = tempfile::tempdir().unwrap();
    let key = config.path().join("trusted.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let invoke = |name: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_cowork-runtime-package"))
            .arg(source.path())
            .arg(&key)
            .arg("fixture-only")
            .arg("linux_x86_64")
            .arg("--stage")
            .arg(parent.path())
            .arg(name)
            .arg("36")
            .output()
            .unwrap()
    };
    let result = invoke("cli-attempt");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let receipt: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(receipt["staged"], true);
    assert_eq!(receipt["installed"], false);
    assert_eq!(receipt["qualified"], false);
    assert_eq!(receipt["execution_authority"], false);
    assert_eq!(receipt["native_custody_reconstructible"], false);
    assert_eq!(
        receipt["manifest_sha256"],
        sha(&std::fs::read(source.path().join("manifest.json")).unwrap())
    );
    assert_eq!(
        std::fs::read_dir(parent.path().join("cli-attempt"))
            .unwrap()
            .count(),
        11
    );
    assert!(!invoke("cli-attempt").status.success());
    assert!(!invoke("../escape").status.success());
}

#[test]
fn valid_source_package_exchange_cannot_reuse_the_earlier_inspection_receipt() {
    let mut f = Fixture::new();
    let source = f.directory();
    let parent = private_parent();
    let original = verify_directory(&open(source.path()), &f.trust, Platform::LinuxX86_64).unwrap();
    f.manifest["release_sequence"] = json!(2);
    f.manifest["version"] = json!("fixture.2");
    let replacement = serde_json::to_vec_pretty(&f.manifest).unwrap();
    std::fs::write(source.path().join("manifest.json"), &replacement).unwrap();
    std::fs::write(
        source.path().join("manifest.sig.json"),
        f.sign(&replacement),
    )
    .unwrap();
    // Both packages are valid under exactly the same independent trust root.
    // Swap deterministically after A's actual inspection and before B's stage.
    let later = verify_directory(&open(source.path()), &f.trust, Platform::LinuxX86_64).unwrap();
    assert_ne!(original.manifest_sha256(), later.manifest_sha256());
    let mut staged = StagingArea::create(open(parent.path()), "exchange", 36)
        .unwrap()
        .stage(&open(source.path()), &f.trust, Platform::LinuxX86_64)
        .unwrap();
    let receipt = serde_json::to_value(staged.receipt().unwrap()).unwrap();
    assert_eq!(receipt["manifest_sha256"], later.manifest_sha256());
    assert_ne!(receipt["manifest_sha256"], original.manifest_sha256());
    assert_eq!(receipt["manifest"]["release_sequence"], 2);
    assert_eq!(receipt["manifest"]["version"], "fixture.2");
    assert_eq!(
        std::fs::read(parent.path().join("exchange/manifest.json")).unwrap(),
        replacement
    );
    assert_eq!(receipt["installed"], false);
    assert_eq!(receipt["execution_authority"], false);
}

#[cfg(target_os = "linux")]
#[test]
fn identical_replacement_extra_member_and_permission_changes_invalidate_original_stage() {
    let f = Fixture::new();
    let source = f.directory();
    let parent = private_parent();
    let mut staged = StagingArea::create(open(parent.path()), "attempt", 36)
        .unwrap()
        .stage(&open(source.path()), &f.trust, Platform::LinuxX86_64)
        .unwrap();
    let path = parent.path().join("attempt");
    std::fs::write(path.join("unexpected"), b"x").unwrap();
    assert!(staged.verify().is_err());
    std::fs::remove_file(path.join("unexpected")).unwrap();
    staged.verify().unwrap();
    let file = path.join("component-0.bin");
    let old = path.join("original-held.bin");
    std::fs::rename(&file, &old).unwrap();
    std::fs::write(&file, b"test").unwrap();
    assert!(staged.verify().is_err());
    std::fs::remove_file(&file).unwrap();
    std::fs::rename(&old, &file).unwrap();
    staged.verify().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(staged.verify().is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    staged.verify().unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(staged.verify().is_err());
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    staged.verify().unwrap();
    std::fs::write(&file, b"fail").unwrap();
    assert!(staged.verify().is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn replaced_directory_symlink_fifo_and_unprotected_parent_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let parent = private_parent();
    let f = Fixture::new();
    let source = f.directory();
    let mut staged = StagingArea::create(open(parent.path()), "attempt", 36)
        .unwrap()
        .stage(&open(source.path()), &f.trust, Platform::LinuxX86_64)
        .unwrap();
    let path = parent.path().join("attempt");
    let old = parent.path().join("held-original");
    std::fs::rename(&path, &old).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(staged.verify().is_err());
    std::fs::remove_dir(&path).unwrap();
    symlink(&old, &path).unwrap();
    assert!(staged.verify().is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&old, &path).unwrap();
    staged.verify().unwrap();
    drop(staged);
    let component = source.path().join("payload/base_rootfs.payload");
    std::fs::remove_file(&component).unwrap();
    let fifo = std::ffi::CString::new(component.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let start = Instant::now();
    assert!(
        StagingArea::create(open(parent.path()), "fifo", 36)
            .unwrap()
            .stage(&open(source.path()), &f.trust, Platform::LinuxX86_64)
            .is_err()
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(StagingArea::create(open(parent.path()), "unprotected", 36).is_err());
    assert!(!parent.path().join("unprotected").exists());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the private 8-MiB filesystem fixture; never uses the host free disk"]
fn actual_reserved_storage_exhaustion_cannot_publish_staging() {
    use std::os::unix::fs::MetadataExt;
    let parent = std::path::PathBuf::from(
        std::env::var_os("COWORK_STAGING_ENOSPC_PARENT")
            .expect("explicit private bounded filesystem fixture required"),
    );
    let meta = std::fs::symlink_metadata(&parent).unwrap();
    assert!(parent.is_absolute() && meta.is_dir() && !meta.file_type().is_symlink());
    assert_eq!(meta.uid(), unsafe { libc::geteuid() });
    assert_eq!(meta.mode() & 0o077, 0);
    assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
    let mut f = Fixture::new();
    let bytes = vec![0x58; 1024 * 1024];
    for component in f.manifest["components"].as_array_mut().unwrap() {
        component["bytes"] = json!(bytes.len());
        component["sha256"] = json!(sha(&bytes));
    }
    let source = f.directory();
    for component in f.manifest["components"].as_array().unwrap() {
        std::fs::write(
            source
                .path()
                .join("payload")
                .join(component["filename"].as_str().unwrap()),
            &bytes,
        )
        .unwrap();
    }
    let result = StagingArea::create(open(&parent), "full", 9 * 1024 * 1024)
        .unwrap()
        .stage(&open(source.path()), &f.trust, Platform::LinuxX86_64);
    assert!(
        matches!(result, Err(Error::Io(ref error)) if error.raw_os_error() == Some(libc::ENOSPC))
    );
    let stage = parent.join("full");
    let entries: Vec<_> = std::fs::read_dir(&stage)
        .unwrap()
        .collect::<std::io::Result<_>>()
        .unwrap();
    assert!(entries.len() <= 11);
    for entry in entries {
        // Reservation failed before copying any signed payload bytes. Existing
        // zero-filled bounded attempts remain for controlled recovery.
        let data = std::fs::read(entry.path()).unwrap();
        assert!(data.len() <= 1024 * 1024);
        assert!(data.iter().all(|b| *b == 0));
    }
    assert!(source.path().join("payload/base_rootfs.payload").is_file());
}

#[cfg(windows)]
#[test]
fn windows_pinned_originals_deny_writes_deletes_and_foreign_acl() {
    let f = Fixture::new();
    let source = f.directory();
    let parent = private_parent();
    let mut staged = StagingArea::create(open(parent.path()), "attempt", 36)
        .unwrap()
        .stage(&open(source.path()), &f.trust, Platform::LinuxX86_64)
        .unwrap();
    let path = parent.path().join("attempt");
    let file = path.join("component-0.bin");
    assert!(std::fs::write(&file, b"fail").is_err());
    assert!(std::fs::remove_file(&file).is_err());
    assert!(std::fs::rename(&file, path.join("replacement.bin")).is_err());
    staged.verify().unwrap();
    set_acl(
        &path,
        "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;WD)",
    );
    assert!(staged.verify().is_err());
    assert!(StagingArea::create(open(&path), "foreign-writable", 36).is_err());
    assert!(!path.join("foreign-writable").exists());
}

#[cfg(windows)]
fn set_acl(path: &std::path::Path, sddl: &str) {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        Storage::FileSystem::*,
    };
    let mut descriptor = std::ptr::null_mut();
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    assert_ne!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        },
        0
    );
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl = std::ptr::null_mut();
    assert_ne!(
        unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) },
        0
    );
    let handle = std::fs::OpenOptions::new()
        .access_mode(WRITE_DAC | READ_CONTROL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .unwrap();
    let status = unsafe {
        SetSecurityInfo(
            handle.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl,
            std::ptr::null(),
        )
    };
    unsafe {
        LocalFree(descriptor);
    }
    assert_eq!(status, 0);
}
