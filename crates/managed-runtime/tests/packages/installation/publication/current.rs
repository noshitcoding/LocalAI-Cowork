use super::*;
use cowork_managed_runtime::staging::archive::{InstalledPackage, InstalledPackagePolicy};
use cowork_managed_runtime::staging::release_floor::ReleaseFloor;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn load_policy() -> InstalledPackagePolicy {
    InstalledPackagePolicy::new(
        2 * 1024 * 1024,
        InstallationLimits::new(27, 1024, 8192).unwrap(),
        TargetDescriptorPolicy::new(27).unwrap(),
    )
    .unwrap()
}
fn floor(parent: &Path, f: &Fixture) -> ReleaseFloor {
    ReleaseFloor::open(open(parent), "load-policy", &f.trust, Platform::LinuxX86_64).unwrap()
}
fn load(parent: &Path, versions: &Path, f: &Fixture, identity: &str) -> Result<InstalledPackage> {
    InstalledPackage::open(
        open(versions),
        identity,
        &f.trust,
        Platform::LinuxX86_64,
        floor(parent, f),
        load_policy(),
    )
}

#[test]
#[ignore = "owned root-only bounded fresh installed-byte fixture; no runtime activation"]
fn actual_current_package_proves_all_roles_and_rejects_wrong_or_mixed_bytes() {
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let parent =
        std::path::PathBuf::from(std::env::var_os("COWORK_STAGING_ENOSPC_PARENT").unwrap());
    let versions = parent.join("versions");
    std::fs::create_dir(&versions).unwrap();
    std::fs::set_permissions(&versions, std::fs::Permissions::from_mode(0o700)).unwrap();
    let raw = data();
    let (f, source) = package_data(&raw);
    let exact = cowork_managed_runtime::verify_directory(
        &open(source.path()),
        &f.trust,
        Platform::LinuxX86_64,
    )
    .unwrap();
    let identity = exact.manifest_sha256().to_owned();
    let mut publication = attempt(&parent, "load-input", &f, source.path());
    publication.publish(open(&versions)).unwrap();
    drop(publication);
    let mut policy = ReleaseFloor::initialize(
        open(&parent),
        "load-policy",
        &f.trust,
        Platform::LinuxX86_64,
    )
    .unwrap();
    policy.advance(&exact).unwrap();
    drop(policy);
    let original = load(&parent, &versions, &f, &identity).unwrap();
    assert_eq!(original.manifest_sha256(), identity);
    assert_eq!(original.manifest().components.len(), 9);
    original.verify().unwrap();
    // An admission capability retains native identity, not an exclusive policy
    // writer lock that would prevent subsequent update decisions forever.
    let admission = original.admit_new_start().unwrap();
    assert!(
        ReleaseFloor::open(
            open(&parent),
            "load-policy",
            &f.trust,
            Platform::LinuxX86_64
        )
        .is_err()
    );
    drop(admission);
    // A new independently valid policy at the same path cannot replace the
    // original live anchor. Matching bytes/paths never recover old custody.
    let policy_path = parent.join("load-policy");
    let saved_policy = parent.join("original-load-policy");
    let replacement_policy = parent.join("replacement-load-policy");
    std::fs::rename(&policy_path, &saved_policy).unwrap();
    let mut substitute = ReleaseFloor::initialize(
        open(&parent),
        "load-policy",
        &f.trust,
        Platform::LinuxX86_64,
    )
    .unwrap();
    substitute.advance(&exact).unwrap();
    drop(substitute);
    load(&parent, &versions, &f, &identity).unwrap();
    assert!(original.admit_new_start().is_err());
    std::fs::rename(&policy_path, &replacement_policy).unwrap();
    std::fs::rename(&saved_policy, &policy_path).unwrap();
    drop(original.admit_new_start().unwrap());
    println!("INSTALLED_ANCHORED_POLICY_REPLACEMENT_REJECTED=true");
    let mut current_policy = floor(&parent, &f);
    let mut newer = f.manifest.clone();
    newer["release_sequence"] = json!(11);
    let newer = f.verify(&newer).unwrap();
    current_policy.advance(&newer).unwrap();
    drop(current_policy);
    assert!(load(&parent, &versions, &f, &identity).is_err());
    assert!(original.admit_new_start().is_err());
    // Already retained bytes remain themselves: raising policy does not rebind
    // an existing profile or turn byte verification into a new-start grant.
    original.verify().unwrap();
    println!("INSTALLED_CURRENT_NINE_ROLES_AND_NONBLOCKING_FLOOR=true");

    // Use another original policy for the remaining immutable-byte checks.
    let secondary = parent.join("secondary");
    std::fs::create_dir(&secondary).unwrap();
    std::fs::set_permissions(&secondary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut policy = ReleaseFloor::initialize(
        open(&secondary),
        "load-policy",
        &f.trust,
        Platform::LinuxX86_64,
    )
    .unwrap();
    policy.advance(&exact).unwrap();
    drop(policy);
    let package_root = versions.join(format!("version-{identity}"));
    let wrong_identity = "b".repeat(64);
    let wrong_root = versions.join(format!("version-{wrong_identity}"));
    std::fs::rename(&package_root, &wrong_root).unwrap();
    assert!(matches!(
        load(&secondary, &versions, &f, &wrong_identity),
        Err(Error::Archive("installed_package_identity"))
    ));
    std::fs::rename(&wrong_root, &package_root).unwrap();
    original.verify().unwrap();
    println!("INSTALLED_WRONG_REAL_PACKAGE_IDENTITY_REJECTED=true");

    let mut changed = raw.clone();
    let vectors = role_vectors();
    let body = vectors[3]["body"].as_str().unwrap().as_bytes();
    let offset = changed[3]
        .windows(body.len())
        .position(|b| b == body)
        .unwrap();
    changed[3][offset] ^= 1;
    let (other_f, other_source) = package_data(&changed);
    let other_exact = cowork_managed_runtime::verify_directory(
        &open(other_source.path()),
        &other_f.trust,
        Platform::LinuxX86_64,
    )
    .unwrap();
    let other_root = versions.join(format!("version-{}", other_exact.manifest_sha256()));
    let mut other_publication = attempt(&parent, "other-load", &other_f, other_source.path());
    other_publication.publish(open(&versions)).unwrap();
    drop(other_publication);
    let a_role = package_root.join("runtime/component-3");
    let b_role = other_root.join("runtime/component-3");
    let saved = parent.join("original-role-three");
    std::fs::rename(&a_role, &saved).unwrap();
    std::fs::rename(&b_role, &a_role).unwrap();
    assert!(load(&secondary, &versions, &f, &identity).is_err());
    assert!(original.verify().is_err());
    std::fs::rename(&a_role, &b_role).unwrap();
    std::fs::rename(&saved, &a_role).unwrap();
    original.verify().unwrap();
    println!("INSTALLED_MIXED_SIGNED_ROLE_TREES_REJECTED=true");

    let regular = package_root
        .join("runtime/component-8")
        .join(vectors[8]["file"].as_str().unwrap());
    let replacement = regular.with_file_name("replacement-fixture");
    std::fs::write(&replacement, std::fs::read(&regular).unwrap()).unwrap();
    std::fs::set_permissions(&replacement, regular.metadata().unwrap().permissions()).unwrap();
    std::fs::rename(&replacement, &regular).unwrap();
    assert!(original.verify().is_err());
    // A separately authorized complete fresh load may acquire the new current
    // originals; the earlier live capability never adopts replacement inodes.
    load(&secondary, &versions, &f, &identity)
        .unwrap()
        .verify()
        .unwrap();
    println!("INSTALLED_ORIGINAL_REPLACEMENT_NEVER_ADOPTED=true");
    println!("INSTALLED_FIXTURE_NO_BACKEND_OR_WORKLOAD_STARTED=true");
}
