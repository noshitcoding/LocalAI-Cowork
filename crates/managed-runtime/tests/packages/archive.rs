use super::*;
use cowork_managed_runtime::staging::archive::{ArchiveEntry, ArchiveLimits, InspectedArchive};

#[path = "installation.rs"]
mod installation;

fn vectors() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("../fixtures/python-ustar.json")).unwrap()["cases"]
        .as_array()
        .unwrap()
        .clone()
}
fn bytes(vector: &Value) -> Vec<u8> {
    vector["hex"]
        .as_str()
        .unwrap()
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn stage(
    data: &[u8],
    parent: &std::path::Path,
    attempt: &str,
) -> cowork_managed_runtime::staging::StagedPackage {
    let mut f = Fixture::new();
    f.manifest["components"][0]["bytes"] = json!(data.len());
    f.manifest["components"][0]["sha256"] = json!(sha(data));
    let source = f.directory();
    std::fs::write(source.path().join("payload/base_rootfs.payload"), data).unwrap();
    StagingArea::create(open(parent), attempt, data.len() as u64 + 32)
        .unwrap()
        .stage(&open(source.path()), &f.trust, Platform::LinuxX86_64)
        .unwrap()
}
fn inspect(data: &[u8], parent: &std::path::Path, attempt: &str) -> Result<InspectedArchive> {
    stage(data, parent, attempt).inspect_archive(
        ComponentRole::BaseRootfs,
        ArchiveLimits::new(16, 1024, 4096).unwrap(),
    )
}

#[test]
fn signed_independent_python_tar_is_inspected_and_file_bytes_come_from_original_custody() {
    let parent = private_parent();
    let data = bytes(&vectors()[0]);
    let mut archive = inspect(&data, parent.path(), "valid").unwrap();
    assert_eq!(archive.role(), ComponentRole::BaseRootfs);
    assert_eq!(archive.entries().count(), 3);
    assert_eq!(
        archive.expanded_bytes(),
        b"signed archive fixture\n".len() as u64
    );
    assert!(archive.entries().any(|(name, item)| name == "hello"
        && matches!(item, ArchiveEntry::Symlink { target } if target == "bin/hello")));
    let mut copied = Vec::new();
    archive.copy_file("bin/hello", &mut copied).unwrap();
    assert_eq!(copied, b"signed archive fixture\n");
    assert!(archive.copy_file("hello", &mut Vec::new()).is_err());
    assert!(archive.copy_file("../outside", &mut Vec::new()).is_err());
    archive.verify().unwrap();
}

#[test]
fn even_valid_signed_archives_reject_unsafe_or_unsupported_members_before_extraction() {
    let parent = private_parent();
    for vector in vectors().into_iter().skip(1) {
        let name = vector["name"].as_str().unwrap();
        let error = vector["error"].as_str().unwrap();
        let result = inspect(&bytes(&vector), parent.path(), name);
        assert!(
            matches!(result, Err(Error::Archive(reason)) if reason == error),
            "{name}: {error}"
        );
    }
}

#[test]
fn signed_archive_limits_bound_entries_expansion_and_path_memory() {
    let parent = private_parent();
    let data = bytes(&vectors()[0]);
    for (attempt, limits, reason) in [
        (
            "entries",
            ArchiveLimits::new(1, 1024, 4096).unwrap(),
            "entry_limit",
        ),
        (
            "expanded",
            ArchiveLimits::new(16, 1, 4096).unwrap(),
            "expanded_limit",
        ),
        (
            "paths",
            ArchiveLimits::new(16, 1024, 1).unwrap(),
            "path_limit",
        ),
    ] {
        let result =
            stage(&data, parent.path(), attempt).inspect_archive(ComponentRole::BaseRootfs, limits);
        assert!(matches!(result, Err(Error::Archive(found)) if found == reason));
    }
}

#[test]
fn cli_inspects_signed_original_staging_without_extraction_or_execution() {
    let data = bytes(&vectors()[0]);
    let mut f = Fixture::new();
    f.manifest["components"][0]["bytes"] = json!(data.len());
    f.manifest["components"][0]["sha256"] = json!(sha(&data));
    let source = f.directory();
    std::fs::write(source.path().join("payload/base_rootfs.payload"), &data).unwrap();
    let parent = private_parent();
    let config = tempfile::tempdir().unwrap();
    let key = config.path().join("trusted.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cowork-runtime-package"))
        .arg(source.path())
        .arg(&key)
        .arg("fixture-only")
        .arg("linux_x86_64")
        .arg("--stage")
        .arg(parent.path())
        .arg("cli-archive")
        .arg("65536")
        .args(["--inspect-archive", "base_rootfs", "16", "1024", "4096"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["archive"]["entries"], 3);
    assert_eq!(receipt["archive"]["expanded_bytes"], 23);
    assert_eq!(receipt["archive"]["inspected"], true);
    assert_eq!(receipt["archive"]["extracted"], false);
    assert_eq!(receipt["installed"], false);
    assert_eq!(receipt["qualified"], false);
    assert_eq!(receipt["execution_authority"], false);
}

#[cfg(target_os = "linux")]
#[test]
fn changed_original_archive_bytes_cannot_be_reported_as_a_successful_copy() {
    use std::io::{Seek, Write};
    let parent = private_parent();
    let mut archive = inspect(&bytes(&vectors()[0]), parent.path(), "changed").unwrap();
    let path = parent.path().join("changed/component-0.bin");
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(std::io::SeekFrom::Start(1024)).unwrap();
    file.write_all(b"bad").unwrap();
    file.sync_all().unwrap();
    assert!(archive.copy_file("bin/hello", &mut Vec::new()).is_err());
    assert!(archive.verify().is_err());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the private root-owned bounded filesystem fixture"]
fn actual_root_archive_extraction_keeps_original_objects_and_never_follows_links() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let parent = std::path::PathBuf::from(
        std::env::var_os("COWORK_STAGING_ENOSPC_PARENT").expect("private fixture required"),
    );
    assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
    let archive = inspect(&bytes(&vectors()[0]), &parent, "source").unwrap();
    let target = StagingArea::create(open(&parent), "expanded", 1024).unwrap();
    let mut extracted = archive.extract(target).unwrap();
    let root = parent.join("expanded");
    assert_eq!(
        extracted.expanded_bytes(),
        b"signed archive fixture\n".len() as u64
    );
    assert_eq!(
        std::fs::read(root.join("bin/hello")).unwrap(),
        b"signed archive fixture\n"
    );
    assert_eq!(
        std::fs::read_link(root.join("hello")).unwrap(),
        std::path::Path::new("bin/hello")
    );
    let meta = std::fs::symlink_metadata(root.join("bin/hello")).unwrap();
    assert_eq!(meta.uid(), 0);
    assert_eq!(meta.nlink(), 1);
    assert_eq!(meta.mode() & 0o7777, 0o444);
    assert_eq!(
        std::fs::metadata(root.join("bin")).unwrap().mode() & 0o7777,
        0o555
    );
    extracted.verify().unwrap();
    let mut f = Fixture::new();
    let data = bytes(&vectors()[0]);
    f.manifest["components"][0]["bytes"] = json!(data.len());
    f.manifest["components"][0]["sha256"] = json!(sha(&data));
    let cli_source = f.directory();
    std::fs::write(cli_source.path().join("payload/base_rootfs.payload"), &data).unwrap();
    let config = tempfile::tempdir().unwrap();
    let key = config.path().join("trusted.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let program = std::env::var_os("COWORK_STAGING_PACKAGE_BINARY")
        .expect("exact private CLI binary required");
    let output = std::process::Command::new(program)
        .arg(cli_source.path())
        .arg(&key)
        .arg("fixture-only")
        .arg("linux_x86_64")
        .arg("--stage")
        .arg(&parent)
        .arg("cli-source")
        .arg("65536")
        .arg("--extract-archive")
        .arg(&parent)
        .arg("cli-expanded")
        .args(["base_rootfs", "16", "1024", "4096"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["archive"]["extracted"], true);
    assert_eq!(
        receipt["manifest_sha256"],
        sha(&std::fs::read(cli_source.path().join("manifest.json")).unwrap())
    );
    assert_eq!(receipt["installed"], false);
    assert_eq!(receipt["qualified"], false);
    assert_eq!(receipt["execution_authority"], false);
    assert_eq!(
        std::fs::read(parent.join("cli-expanded/bin/hello")).unwrap(),
        b"signed archive fixture\n"
    );
    assert_eq!(
        std::fs::read_link(parent.join("cli-expanded/hello")).unwrap(),
        std::path::Path::new("bin/hello")
    );
    println!("ARCHIVE_NATIVE_CLI_EXTRACTED=true");
    std::fs::write(root.join("unexpected"), b"untrusted").unwrap();
    assert!(extracted.verify().is_err());
    std::fs::remove_file(root.join("unexpected")).unwrap();
    extracted.verify().unwrap();
    let file = root.join("bin/hello");
    let original = root.join("bin/held-original");
    std::fs::rename(&file, &original).unwrap();
    std::fs::write(&file, b"signed archive fixture\n").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    assert!(extracted.verify().is_err());
    assert_eq!(
        std::fs::read(&original).unwrap(),
        b"signed archive fixture\n"
    );
    std::fs::remove_file(&file).unwrap();
    std::fs::rename(&original, &file).unwrap();
    extracted.verify().unwrap();
    std::fs::write(&file, b"untrusted archive copy\n").unwrap();
    assert!(extracted.verify().is_err());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires its own private UID-0 custody fixture, including a scoped child FD limit"]
fn actual_root_target_custody_survives_unlink_and_rejects_descriptor_exhaustion() {
    use cowork_managed_runtime::staging::archive::TargetDescriptorPolicy;
    use std::os::unix::{
        fs::{MetadataExt, PermissionsExt, symlink},
        process::CommandExt,
    };
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let parent = std::path::PathBuf::from(
        std::env::var_os("COWORK_STAGING_ENOSPC_PARENT").expect("own fixture required"),
    );
    assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
    let data = bytes(&vectors()[0]);
    let archive = inspect(&data, &parent, "source").unwrap();
    let target = StagingArea::create(open(&parent), "expanded", 1024).unwrap();
    let mut extracted = archive.extract(target).unwrap();
    assert_eq!(extracted.retained_target_descriptors(), 3);
    let root = parent.join("expanded");
    let file = root.join("bin/hello");
    let dir = root.join("bin");
    let link = root.join("hello");
    let originals = [
        std::fs::symlink_metadata(&file).unwrap(),
        std::fs::symlink_metadata(&dir).unwrap(),
        std::fs::symlink_metadata(&link).unwrap(),
    ];
    for meta in &originals {
        assert!(original_descriptors(meta.dev(), meta.ino()) > 0);
    }
    // Delete each original name. No held-original rename can prevent inode
    // reuse on behalf of the implementation under test.
    std::fs::remove_file(&file).unwrap();
    std::fs::write(&file, b"signed archive fixture\n").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    assert_ne!(std::fs::metadata(&file).unwrap().ino(), originals[0].ino());
    assert!(extracted.verify().is_err());
    std::fs::remove_file(&file).unwrap();
    std::fs::remove_dir(&dir).unwrap();
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    assert_ne!(std::fs::metadata(&dir).unwrap().ino(), originals[1].ino());
    std::fs::write(&file, b"signed archive fixture\n").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    std::fs::remove_file(&link).unwrap();
    symlink("bin/hello", &link).unwrap();
    assert_ne!(
        std::fs::symlink_metadata(&link).unwrap().ino(),
        originals[2].ino()
    );
    assert!(extracted.verify().is_err());
    for _ in 0..64 {
        let probe = root.join("reuse-probe");
        std::fs::write(&probe, b"same filesystem inode allocation").unwrap();
        let meta = std::fs::metadata(&probe).unwrap();
        assert!(
            originals
                .iter()
                .all(|old| (old.dev(), old.ino()) != (meta.dev(), meta.ino()))
        );
        std::fs::remove_file(&probe).unwrap();
    }
    for meta in &originals {
        assert!(original_descriptors(meta.dev(), meta.ino()) > 0);
    }
    println!("ARCHIVE_TARGET_ORIGINALS_PINNED=true");
    drop(extracted);
    for meta in &originals {
        assert_eq!(original_descriptors(meta.dev(), meta.ino()), 0);
    }
    println!("ARCHIVE_TARGET_DESCRIPTORS_DROPPED=true");

    // The configured object budget must fail before creating target members.
    let outside = private_parent();
    let archive = inspect(&data, outside.path(), "policy-source").unwrap();
    let target = StagingArea::create(open(&parent), "policy-empty", 1024).unwrap();
    assert!(matches!(
        archive.extract_with_policy(target, TargetDescriptorPolicy::new(2).unwrap()),
        Err(Error::Archive("target_descriptor_budget"))
    ));
    assert_eq!(
        std::fs::read_dir(parent.join("policy-empty"))
            .unwrap()
            .count(),
        0
    );

    // Exercise actual EMFILE in a dedicated child only. The parent process and
    // every foreign process/global limit remain unchanged.
    let mut f = Fixture::new();
    f.manifest["components"][0]["bytes"] = json!(data.len());
    f.manifest["components"][0]["sha256"] = json!(sha(&data));
    let source = f.directory();
    std::fs::write(source.path().join("payload/base_rootfs.payload"), &data).unwrap();
    let config = tempfile::tempdir().unwrap();
    let key = config.path().join("trusted.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let program =
        std::env::var_os("COWORK_STAGING_PACKAGE_BINARY").expect("exact private CLI required");
    let mut command = std::process::Command::new(program);
    command
        .arg(source.path())
        .arg(&key)
        .arg("fixture-only")
        .arg("linux_x86_64")
        .arg("--stage")
        .arg(&parent)
        .arg("limited-source")
        .arg("65536")
        .arg("--extract-archive")
        .arg(&parent)
        .arg("limited-empty")
        .args(["base_rootfs", "16", "1024", "4096"]);
    let mut original_limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut original_limit) },
        0
    );
    let hard = original_limit.rlim_max;
    assert!(hard >= 48);
    unsafe {
        command.pre_exec(move || {
            let limit = libc::rlimit {
                rlim_cur: 48,
                rlim_max: hard,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("target_descriptor_budget"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_dir(parent.join("limited-empty"))
            .unwrap()
            .count(),
        0
    );
    let mut after = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut after) },
        0
    );
    assert_eq!(
        (original_limit.rlim_cur, original_limit.rlim_max),
        (after.rlim_cur, after.rlim_max)
    );
    println!("ARCHIVE_TARGET_FD_LIMIT_REJECTED=true");
}

#[cfg(target_os = "linux")]
fn original_descriptors(device: u64, inode: u64) -> usize {
    let descriptors: Vec<i32> = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_str()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    assert!(descriptors.len() < 2048);
    descriptors
        .into_iter()
        .filter(|fd| {
            let mut meta = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(*fd, meta.as_mut_ptr()) } != 0 {
                return false;
            }
            let meta = unsafe { meta.assume_init() };
            if meta.st_dev != device || meta.st_ino != inode {
                return false;
            }
            assert_ne!(
                unsafe { libc::fcntl(*fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
                0
            );
            true
        })
        .count()
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires its own private fixture and a scoped low-FD CLI child"]
fn actual_deep_valid_tree_descriptor_budget_rejects_before_target_mutation() {
    use std::os::unix::process::CommandExt;
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let parent = std::path::PathBuf::from(
        std::env::var_os("COWORK_STAGING_ENOSPC_PARENT").expect("own fixture required"),
    );
    assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/python-ustar.json")).unwrap();
    let data = bytes(&fixture["deep_valid"]);
    let mut f = Fixture::new();
    f.manifest["components"][0]["bytes"] = json!(data.len());
    f.manifest["components"][0]["sha256"] = json!(sha(&data));
    let source = f.directory();
    std::fs::write(source.path().join("payload/base_rootfs.payload"), &data).unwrap();
    // Confirm valid signed content and the actual forty-level directory tree
    // independently of descriptor admission in the child.
    let outside = private_parent();
    let checked = stage(&data, outside.path(), "valid-deep")
        .inspect_archive(
            ComponentRole::BaseRootfs,
            ArchiveLimits::new(128, 1024, 8192).unwrap(),
        )
        .unwrap();
    assert_eq!(checked.entries().count(), 41);
    assert_eq!(
        checked
            .entries()
            .filter(|(_, item)| matches!(item, ArchiveEntry::Directory))
            .count(),
        40
    );
    drop(checked);
    let config = tempfile::tempdir().unwrap();
    let key = config.path().join("trusted.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let program =
        std::env::var_os("COWORK_STAGING_PACKAGE_BINARY").expect("exact private CLI required");
    let mut command = std::process::Command::new(program);
    command
        .arg(source.path())
        .arg(&key)
        .arg("fixture-only")
        .arg("linux_x86_64")
        .arg("--stage")
        .arg(&parent)
        .arg("deep-source")
        .arg("65536")
        .arg("--extract-archive")
        .arg(&parent)
        .arg("deep-output")
        .args(["base_rootfs", "128", "1024", "8192"]);
    let mut original = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut original) },
        0
    );
    let hard = original.rlim_max;
    assert!(hard >= 140);
    unsafe {
        command.pre_exec(move || {
            let limit = libc::rlimit {
                rlim_cur: 140,
                rlim_max: hard,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let result = command.output().unwrap();
    let root = parent.join("deep-output");
    let mut pending = vec![root.clone()];
    let mut members = 0;
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            members += 1;
            assert!(members <= 41);
            if kind.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    println!("DEEP_CLI_TARGET_MEMBERS={members}");
    println!(
        "DEEP_CLI_ERROR={}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("target_descriptor_budget"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        members, 0,
        "descriptor admission must precede every target member"
    );
    let mut after = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut after) },
        0
    );
    assert_eq!(
        (after.rlim_cur, after.rlim_max),
        (original.rlim_cur, original.rlim_max)
    );
    println!("ARCHIVE_DEEP_FD_BUDGET_REJECTED_BEFORE_MUTATION=true");
}
