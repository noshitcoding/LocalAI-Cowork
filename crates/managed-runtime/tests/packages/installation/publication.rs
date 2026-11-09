use super::*;
#[path = "publication/current.rs"]
mod current;
#[path = "publication/journal.rs"]
mod journal;
use cowork_managed_runtime::staging::archive::{
    PublicationAttempt, PublicationPhase, PublicationPolicy, TargetDescriptorPolicy,
};

fn policy() -> PublicationPolicy {
    PublicationPolicy::new(
        1024 * 1024,
        InstallationLimits::new(27, 1024, 8192).unwrap(),
        TargetDescriptorPolicy::new(27).unwrap(),
    )
    .unwrap()
}
fn attempt(
    parent: &std::path::Path,
    name: &str,
    f: &Fixture,
    source: &std::path::Path,
) -> PublicationAttempt {
    PublicationAttempt::prepare(
        StagingArea::create(open(parent), name, 2 * 1024 * 1024).unwrap(),
        &open(source),
        &f.trust,
        Platform::LinuxX86_64,
        policy(),
    )
    .unwrap()
}

#[test]
fn publication_requires_root_before_creating_any_package_or_runtime_child() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let parent = private_parent();
    let (f, source) = package_data(&data());
    let area = StagingArea::create(open(parent.path()), "unprivileged", 2 * 1024 * 1024).unwrap();
    assert!(matches!(
        PublicationAttempt::prepare(
            area,
            &open(source.path()),
            &f.trust,
            Platform::LinuxX86_64,
            policy()
        ),
        Err(Error::Archive("root_manager_required"))
    ));
    assert_eq!(
        std::fs::read_dir(parent.path().join("unprivileged"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
#[ignore = "requires own UID-0 bounded atomic publication fixture"]
fn actual_atomic_publication_retains_every_original_and_never_overwrites_or_replays() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let parent =
        std::path::PathBuf::from(std::env::var_os("COWORK_STAGING_ENOSPC_PARENT").unwrap());
    let (f, source) = package_data(&data());
    let versions = parent.join("versions");
    std::fs::create_dir(&versions).unwrap();
    std::fs::set_permissions(&versions, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut first = attempt(&parent, "first", &f, source.path());
    let old = std::fs::metadata(parent.join("first")).unwrap();
    let mut originals = Vec::new();
    for (i, vector) in role_vectors().iter().enumerate() {
        let path = parent
            .join("first/runtime")
            .join(format!("component-{i}"))
            .join(vector["file"].as_str().unwrap());
        let meta = std::fs::metadata(path).unwrap();
        originals.push((meta.dev(), meta.ino()));
    }
    assert_eq!(first.phase(), PublicationPhase::Prepared);
    assert!(first.observe_publication().is_err());
    // Real successful rename with its acknowledgement deliberately left
    // unobserved. No receipt may declare publication before the next step.
    first.begin_publication(open(&versions)).unwrap();
    assert_eq!(first.phase(), PublicationPhase::DispatchedUnconfirmed);
    assert!(!parent.join("first").exists());
    assert_eq!(std::fs::read_dir(&versions).unwrap().count(), 1);
    assert!(first.receipt().is_err());
    assert!(matches!(
        first.publish(open(&versions)),
        Err(Error::Archive("publication_already_dispatched"))
    ));
    first.observe_publication().unwrap();
    assert_eq!(first.phase(), PublicationPhase::Published);
    first.observe_publication().unwrap();
    let receipt = serde_json::to_value(first.receipt().unwrap()).unwrap();
    assert_eq!(receipt["published"], true);
    assert_eq!(receipt["directory_sync_confirmed"], true);
    for key in [
        "installed",
        "activated",
        "qualified",
        "execution_authority",
        "anti_rollback_applied",
        "native_custody_reconstructible",
    ] {
        assert_eq!(receipt[key], false);
    }
    let version = receipt["version_directory"].as_str().unwrap();
    let published = versions.join(version);
    assert!(!parent.join("first").exists());
    let new = std::fs::metadata(&published).unwrap();
    assert_eq!((old.dev(), old.ino()), (new.dev(), new.ino()));
    assert_eq!(std::fs::read_dir(&published).unwrap().count(), 2);
    assert_eq!(
        std::fs::read_dir(published.join("package"))
            .unwrap()
            .count(),
        11
    );
    for (i, vector) in role_vectors().iter().enumerate() {
        let role = published.join("runtime").join(format!("component-{i}"));
        let path = role.join(vector["file"].as_str().unwrap());
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!((meta.dev(), meta.ino()), originals[i]);
        assert_eq!(meta.mode() & 0o7777, 0o444);
        assert_eq!(
            std::fs::read(path).unwrap(),
            vector["body"].as_str().unwrap().as_bytes()
        );
        assert_eq!(
            std::fs::read_link(role.join("entry")).unwrap(),
            std::path::Path::new(vector["file"].as_str().unwrap())
        );
    }
    // An independently prepared but byte/signature-identical second attempt
    // reaches the real kernel NOREPLACE collision; no existing version changes.
    let mut second = attempt(&parent, "second", &f, source.path());
    assert!(
        matches!(second.publish(open(&versions)), Err(Error::Io(ref error)) if error.raw_os_error() == Some(libc::EEXIST))
    );
    assert_eq!(second.phase(), PublicationPhase::DispatchedUnconfirmed);
    assert!(parent.join("second").is_dir());
    assert!(second.receipt().is_err());
    assert!(second.observe_publication().is_err());
    first.verify().unwrap();
    assert_eq!(std::fs::metadata(&published).unwrap().ino(), old.ino());
    // Even after the target name becomes free, the original second attempt
    // cannot issue rename again. Only read-only original-state observation.
    std::fs::rename(&published, versions.join("held-first")).unwrap();
    assert!(matches!(
        second.publish(open(&versions)),
        Err(Error::Archive("publication_already_dispatched"))
    ));
    assert!(second.observe_publication().is_err());
    assert!(!published.exists());
    assert!(parent.join("second").is_dir());
    std::fs::rename(versions.join("held-first"), &published).unwrap();
    first.verify().unwrap();
    println!("ATOMIC_PUBLICATION_ORIGINAL_ROOT_AND_NINE_ROLES=true");
    println!("ATOMIC_PUBLICATION_NOREPLACE_AND_NO_REPLAY=true");
    println!("ATOMIC_PUBLICATION_REOBSERVE_ORIGINAL_AFTER_DISPATCH=true");

    // A real EXDEV must not trigger a copy fallback or a second rename.
    let outside = private_parent();
    assert_ne!(std::fs::metadata(outside.path()).unwrap().dev(), old.dev());
    let mut cross = attempt(&parent, "cross-device", &f, source.path());
    assert!(
        matches!(cross.publish(open(outside.path())), Err(Error::Io(ref error)) if error.raw_os_error() == Some(libc::EXDEV))
    );
    assert_eq!(cross.phase(), PublicationPhase::DispatchedUnconfirmed);
    assert!(parent.join("cross-device").is_dir());
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    assert!(cross.publish(open(&versions)).is_err());
    assert!(cross.observe_publication().is_err());
    println!("ATOMIC_PUBLICATION_CROSS_FILESYSTEM_REJECTED_WITHOUT_COPY=true");

    // Aggregate publication payload includes both original component archives
    // and expanded role files, rather than granting each a full root budget.
    let input_bytes: u64 = data().iter().map(|b| b.len() as u64).sum();
    let expanded: u64 = role_vectors()
        .iter()
        .map(|v| v["body"].as_str().unwrap().len() as u64)
        .sum();
    let limited =
        StagingArea::create(open(&parent), "combined-budget", input_bytes + expanded - 1).unwrap();
    let limited_policy = PublicationPolicy::new(
        input_bytes,
        InstallationLimits::new(27, 1024, 8192).unwrap(),
        TargetDescriptorPolicy::new(27).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        PublicationAttempt::prepare(
            limited,
            &open(source.path()),
            &f.trust,
            Platform::LinuxX86_64,
            limited_policy
        ),
        Err(Error::Archive("publication_payload_budget"))
    ));
    assert!(parent.join("combined-budget/package").is_dir());
    assert!(!parent.join("combined-budget/runtime").exists());
    assert_eq!(std::fs::read_dir(&versions).unwrap().count(), 1);
    println!("ATOMIC_PUBLICATION_COMBINED_INPUT_AND_EXPANDED_BUDGET=true");

    let key_parent = tempfile::tempdir().unwrap();
    let key = key_parent.path().join("trusted.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let cli_versions = parent.join("cli-versions");
    std::fs::create_dir(&cli_versions).unwrap();
    std::fs::set_permissions(&cli_versions, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output =
        std::process::Command::new(std::env::var_os("COWORK_STAGING_PACKAGE_BINARY").unwrap())
            .arg(source.path())
            .arg(&key)
            .args(["fixture-only", "linux_x86_64", "--publish-installation"])
            .arg(&parent)
            .arg("cli-attempt")
            .arg(&cli_versions)
            .args(["2097152", "1048576", "27", "1024", "8192"])
            .output()
            .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(cli["published"], true);
    assert_eq!(cli["installed"], false);
    assert_eq!(cli["version_directory"], version);
    assert!(!parent.join("cli-attempt").exists());
    let cli_root = cli_versions.join(version);
    for (i, vector) in role_vectors().iter().enumerate() {
        let role = cli_root.join("runtime").join(format!("component-{i}"));
        let file = role.join(vector["file"].as_str().unwrap());
        let meta = std::fs::symlink_metadata(&file).unwrap();
        assert!(meta.is_file());
        assert_eq!(meta.uid(), 0);
        assert_eq!(meta.nlink(), 1);
        assert_eq!(meta.mode() & 0o7777, 0o444);
        assert_eq!(
            std::fs::read(file).unwrap(),
            vector["body"].as_str().unwrap().as_bytes()
        );
        assert_eq!(
            std::fs::read_link(role.join("entry")).unwrap(),
            std::path::Path::new(vector["file"].as_str().unwrap())
        );
    }
    println!("ATOMIC_PUBLICATION_ACTUAL_CLI=true");
    // Retain original expanded descriptors through a real unlink/replacement.
    let leaf = published.join("runtime/component-8/bin/runtime_service");
    let before = std::fs::metadata(&leaf).unwrap();
    std::fs::remove_file(&leaf).unwrap();
    std::fs::write(&leaf, role_vectors()[8]["body"].as_str().unwrap()).unwrap();
    std::fs::set_permissions(&leaf, std::fs::Permissions::from_mode(0o444)).unwrap();
    assert_ne!(std::fs::metadata(&leaf).unwrap().ino(), before.ino());
    assert!(first.verify().is_err());
    assert!(first.receipt().is_err());
    println!("ATOMIC_PUBLICATION_ORIGINAL_CUSTODY_AFTER_RENAME=true");
}
