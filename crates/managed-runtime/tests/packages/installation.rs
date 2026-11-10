use super::*;
use cowork_managed_runtime::staging::archive::InstallationLimits;
#[cfg(target_os = "linux")]
#[path = "installation/publication.rs"]
mod publication;

fn role_vectors() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("../fixtures/python-ustar.json")).unwrap()["installation_roles"].as_array().unwrap().clone()
}
fn package_data(data: &[Vec<u8>]) -> (Fixture, tempfile::TempDir) {
    assert_eq!(data.len(), 9);
    let mut f = Fixture::new();
    for (item, data) in f.manifest["components"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(data)
    {
        item["bytes"] = json!(data.len());
        item["sha256"] = json!(sha(data));
    }
    let source = f.directory();
    for (item, data) in f.manifest["components"]
        .as_array()
        .unwrap()
        .iter()
        .zip(data)
    {
        std::fs::write(
            source
                .path()
                .join("payload")
                .join(item["filename"].as_str().unwrap()),
            data,
        )
        .unwrap();
    }
    (f, source)
}
fn staged(
    data: &[Vec<u8>],
    parent: &std::path::Path,
    name: &str,
) -> cowork_managed_runtime::staging::StagedPackage {
    let (f, source) = package_data(data);
    StagingArea::create(
        open(parent),
        name,
        data.iter().map(|b| b.len() as u64).sum(),
    )
    .unwrap()
    .stage(&open(source.path()), &f.trust, Platform::LinuxX86_64)
    .unwrap()
}
fn data() -> Vec<Vec<u8>> {
    role_vectors().iter().map(bytes).collect()
}

#[test]
fn all_nine_roles_share_one_actual_signed_manifest_and_aggregate_receipt() {
    let parent = private_parent();
    let bytes = data();
    let mut inspected = staged(&bytes, parent.path(), "all-nine")
        .inspect_installation(InstallationLimits::new(27, 1024, 8192).unwrap())
        .unwrap();
    let receipt = serde_json::to_value(inspected.receipt().unwrap()).unwrap();
    assert_eq!(receipt["roles"].as_array().unwrap().len(), 9);
    assert_eq!(receipt["entries"], 27);
    assert_eq!(receipt["maximum_depth"], 2);
    for (index, vector) in role_vectors().iter().enumerate() {
        assert_eq!(receipt["roles"][index]["role"], vector["role"]);
        assert_eq!(
            receipt["roles"][index]["component_sha256"],
            sha(&bytes[index])
        );
        assert_eq!(
            receipt["roles"][index]["expanded_bytes"],
            vector["body"].as_str().unwrap().len()
        );
    }
    assert_eq!(receipt["all_roles_inspected"], true);
    assert_eq!(receipt["all_roles_prepared"], false);
    assert_eq!(receipt["installed"], false);
    assert_eq!(receipt["execution_authority"], false);
}

#[test]
fn per_role_valid_archives_cannot_bypass_aggregate_limits_or_hide_the_last_bad_role() {
    let parent = private_parent();
    for (name, limits) in [
        ("entries", InstallationLimits::new(26, 1024, 8192).unwrap()),
        ("bytes", InstallationLimits::new(27, 1, 8192).unwrap()),
        ("paths", InstallationLimits::new(27, 1024, 1).unwrap()),
    ] {
        assert!(
            staged(&data(), parent.path(), name)
                .inspect_installation(limits)
                .is_err()
        );
    }
    let mut broken = data();
    broken[8] = bytes(
        &vectors()
            .into_iter()
            .find(|v| v["name"] == "checksum")
            .unwrap(),
    );
    assert!(matches!(
        staged(&broken, parent.path(), "last-bad")
            .inspect_installation(InstallationLimits::new(100, 1024, 8192).unwrap()),
        Err(Error::Archive("header_checksum"))
    ));
}

#[test]
fn actual_cli_inspects_the_complete_nine_role_package_without_installing() {
    let (f, source) = package_data(&data());
    let parent = private_parent();
    let config = tempfile::tempdir().unwrap();
    let key = config.path().join("trusted.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_cowork-runtime-package"))
        .arg(source.path())
        .arg(&key)
        .arg("fixture-only")
        .arg("linux_x86_64")
        .arg("--stage")
        .arg(parent.path())
        .arg("cli-nine")
        .arg("1048576")
        .args(["--inspect-installation", "27", "1024", "8192"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let receipt: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        receipt["installation"]["roles"].as_array().unwrap().len(),
        9
    );
    assert_eq!(receipt["installation"]["all_roles_inspected"], true);
    assert_eq!(receipt["installation"]["all_roles_prepared"], false);
    assert_eq!(receipt["installed"], false);
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires private UID-0 nine-role preparation fixture"]
fn actual_nine_role_session_prepares_every_role_and_cli_and_rejects_replacement() {
    use cowork_managed_runtime::staging::archive::TargetDescriptorPolicy;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let parent = std::path::PathBuf::from(
        std::env::var_os("COWORK_STAGING_ENOSPC_PARENT").expect("own fixture"),
    );
    let inspected = staged(&data(), &parent, "source")
        .inspect_installation(InstallationLimits::new(27, 1024, 8192).unwrap())
        .unwrap();
    let target = StagingArea::create(open(&parent), "prepared", 1024).unwrap();
    let mut prepared = inspected
        .prepare(target, TargetDescriptorPolicy::new(27).unwrap())
        .unwrap();
    let receipt = serde_json::to_value(prepared.receipt().unwrap()).unwrap();
    assert_eq!(receipt["all_roles_prepared"], true);
    assert_eq!(receipt["roles"].as_array().unwrap().len(), 9);
    assert_eq!(receipt["installed"], false);
    for (index, vector) in role_vectors().iter().enumerate() {
        let root = parent.join("prepared").join(format!("component-{index}"));
        let file = root.join(vector["file"].as_str().unwrap());
        assert_eq!(
            std::fs::read(&file).unwrap(),
            vector["body"].as_str().unwrap().as_bytes()
        );
        assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o7777, 0o444);
        assert_eq!(
            std::fs::read_link(root.join("entry")).unwrap(),
            std::path::Path::new(vector["file"].as_str().unwrap())
        );
    }
    let (f, source) = package_data(&data());
    let config = tempfile::tempdir().unwrap();
    let key = config.path().join("trusted.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let result = std::process::Command::new(
        std::env::var_os("COWORK_STAGING_PACKAGE_BINARY").expect("exact CLI"),
    )
    .arg(source.path())
    .arg(&key)
    .arg("fixture-only")
    .arg("linux_x86_64")
    .arg("--stage")
    .arg(&parent)
    .arg("cli-source")
    .arg("1048576")
    .arg("--prepare-installation")
    .arg(&parent)
    .arg("cli-prepared")
    .args(["27", "1024", "8192"])
    .output()
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let cli: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(cli["installation"]["all_roles_prepared"], true);
    assert_eq!(cli["installation"]["roles"].as_array().unwrap().len(), 9);
    assert_eq!(cli["installation"]["installed"], false);
    for (index, vector) in role_vectors().iter().enumerate() {
        let root = parent
            .join("cli-prepared")
            .join(format!("component-{index}"));
        let file = root.join(vector["file"].as_str().unwrap());
        let meta = std::fs::symlink_metadata(&file).unwrap();
        assert!(meta.is_file());
        assert_eq!(meta.uid(), 0);
        assert_eq!(meta.nlink(), 1);
        assert_eq!(meta.mode() & 0o7777, 0o444);
        assert_eq!(
            std::fs::read(&file).unwrap(),
            vector["body"].as_str().unwrap().as_bytes()
        );
        assert_eq!(
            std::fs::read_link(root.join("entry")).unwrap(),
            std::path::Path::new(vector["file"].as_str().unwrap())
        );
    }
    // Total target descriptors plus all nine role-root capabilities must be
    // admitted together, before even the first component root is created.
    use std::os::unix::process::CommandExt;
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) },
        0
    );
    let hard = limit.rlim_max;
    assert!(hard >= 100);
    let mut command =
        std::process::Command::new(std::env::var_os("COWORK_STAGING_PACKAGE_BINARY").unwrap());
    command
        .arg(source.path())
        .arg(&key)
        .arg("fixture-only")
        .arg("linux_x86_64")
        .arg("--stage")
        .arg(&parent)
        .arg("limited-source")
        .arg("1048576")
        .arg("--prepare-installation")
        .arg(&parent)
        .arg("limited-empty")
        .args(["27", "1024", "8192"]);
    unsafe {
        command.pre_exec(move || {
            let child = libc::rlimit {
                rlim_cur: 100,
                rlim_max: hard,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &child) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let failed = command.output().unwrap();
    assert!(!failed.status.success());
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("target_descriptor_budget"),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
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
        (after.rlim_cur, after.rlim_max),
        (limit.rlim_cur, limit.rlim_max)
    );
    println!("NINE_ROLE_FD_BUDGET_BEFORE_ROLE_ROOTS=true");
    let replaced = parent.join("prepared/component-8/bin/runtime_service");
    let original = std::fs::metadata(&replaced).unwrap();
    std::fs::remove_file(&replaced).unwrap();
    std::fs::write(&replaced, role_vectors()[8]["body"].as_str().unwrap()).unwrap();
    std::fs::set_permissions(&replaced, std::fs::Permissions::from_mode(0o444)).unwrap();
    assert_ne!(std::fs::metadata(&replaced).unwrap().ino(), original.ino());
    assert!(prepared.verify().is_err());
    println!("NINE_ROLE_API_AND_CLI_PREPARED_WITH_ORIGINAL_CUSTODY=true");
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires private bounded filesystem for actual whole-package ENOSPC"]
fn actual_last_role_storage_failure_happens_before_any_role_payload_copy() {
    use cowork_managed_runtime::staging::archive::TargetDescriptorPolicy;
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let parent = std::path::PathBuf::from(
        std::env::var_os("COWORK_STAGING_ENOSPC_PARENT").expect("own fixture"),
    );
    let mut data = data();
    let vectors: Value =
        serde_json::from_str(include_str!("../fixtures/python-ustar.json")).unwrap();
    let header = bytes(&json!({"hex": vectors["large_file_header_hex"]}));
    let mut large = data[8][..512].to_vec();
    large.extend_from_slice(&header);
    large.resize(large.len() + 9 * 1024 * 1024, b'x');
    large.resize(large.len() + 1024, 0);
    data[8] = large;
    let outside = private_parent();
    let inspected = staged(&data, outside.path(), "large-source")
        .inspect_installation(InstallationLimits::new(27, 16 * 1024 * 1024, 8192).unwrap())
        .unwrap();
    let target = StagingArea::create(open(&parent), "unqualified", 16 * 1024 * 1024).unwrap();
    let result = inspected.prepare(target, TargetDescriptorPolicy::new(27).unwrap());
    assert!(
        matches!(result, Err(Error::Io(ref error)) if error.raw_os_error() == Some(libc::ENOSPC))
    );
    let mut pending = vec![parent.join("unqualified")];
    let mut files = 0;
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                assert!(meta.is_file());
                let bytes = std::fs::read(entry.path()).unwrap();
                assert!(bytes.iter().all(|b| *b == 0));
                files += 1;
            }
        }
    }
    assert!(files >= 8);
    println!("NINE_ROLE_ENOSPC_BEFORE_ANY_PAYLOAD=true");
}
