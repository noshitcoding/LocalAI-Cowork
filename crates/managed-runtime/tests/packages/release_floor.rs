#[path = "release_floor/crash.rs"]
mod crash;
use super::*;
use cowork_managed_runtime::staging::release_floor::ReleaseFloor;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

fn open(path: &Path) -> Dir {
    Dir::open_ambient_dir(path, ambient_authority()).unwrap()
}
fn inspect(parent: &Path, name: &str, f: &Fixture) -> Result<ReleaseFloor> {
    ReleaseFloor::open(open(parent), name, &f.trust, Platform::LinuxX86_64)
}
fn snapshot(path: &Path) -> Vec<(String, Vec<u8>, u64, u64, u32)> {
    ["owner.lock", "floor.a", "floor.b"]
        .into_iter()
        .map(|n| {
            let p = path.join(n);
            let m = p.metadata().unwrap();
            (
                n.into(),
                std::fs::read(p).unwrap(),
                m.dev(),
                m.ino(),
                m.mode(),
            )
        })
        .collect()
}

#[test]
#[ignore = "requires own root-only bounded release-policy filesystem"]
fn actual_release_floor_cli_and_original_policy_fail_closed() {
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let parent =
        std::path::PathBuf::from(std::env::var_os("COWORK_STAGING_ENOSPC_PARENT").unwrap());
    let binary = std::env::var_os("COWORK_STAGING_PACKAGE_BINARY").unwrap();
    let mut f = Fixture::new();
    let low = f.directory();
    let low_package = verify_directory(&open(low.path()), &f.trust, Platform::LinuxX86_64).unwrap();
    f.manifest["release_sequence"] = json!(7);
    let high = f.directory();
    let high_package =
        verify_directory(&open(high.path()), &f.trust, Platform::LinuxX86_64).unwrap();
    f.manifest["version"] = json!("other-same-sequence");
    let reused = f.directory();
    let reused_package =
        verify_directory(&open(reused.path()), &f.trust, Platform::LinuxX86_64).unwrap();
    let keys = tempfile::tempdir().unwrap();
    let key = keys.path().join("trusted.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let mgmt = |flag: &str, name: &str| {
        std::process::Command::new(&binary)
            .arg(flag)
            .arg(&parent)
            .arg(name)
            .arg(&key)
            .args(["fixture-only", "linux_x86_64"])
            .output()
            .unwrap()
    };
    let candidate = |source: &Path, flag: &str, name: &str| {
        std::process::Command::new(&binary)
            .arg(source)
            .arg(&key)
            .args(["fixture-only", "linux_x86_64", flag])
            .arg(&parent)
            .arg(name)
            .output()
            .unwrap()
    };
    let missing = mgmt("--release-floor-status", "missing");
    assert!(!missing.status.success());
    assert!(!parent.join("missing").exists());
    assert!(
        mgmt("--initialize-release-floor", "policy")
            .status
            .success()
    );
    let before = snapshot(&parent.join("policy"));
    assert!(
        !mgmt("--initialize-release-floor", "policy")
            .status
            .success()
    );
    assert_eq!(before, snapshot(&parent.join("policy")));
    assert!(
        candidate(high.path(), "--check-release-floor", "policy")
            .status
            .success()
    );
    assert_eq!(before, snapshot(&parent.join("policy")));
    let advanced = candidate(high.path(), "--advance-release-floor", "policy");
    assert!(
        advanced.status.success(),
        "{}",
        String::from_utf8_lossy(&advanced.stderr)
    );
    let receipt: Value = serde_json::from_slice(&advanced.stdout).unwrap();
    assert_eq!(receipt["floor"]["policy_format_version"], 2);
    assert_eq!(receipt["floor"]["release_sequence"], 7);
    assert_eq!(receipt["floor"]["revision"], 1);
    assert_eq!(receipt["floor"]["restrictive_policy_only"], true);
    for n in [
        "installed",
        "activated",
        "qualified",
        "execution_authority",
        "recovered_native_authority",
    ] {
        assert_eq!(receipt["floor"][n], false);
    }
    let committed = snapshot(&parent.join("policy"));
    for source in [low.path(), reused.path()] {
        for operation in ["--check-release-floor", "--advance-release-floor"] {
            let rejected = candidate(source, operation, "policy");
            assert!(!rejected.status.success());
            assert!(
                String::from_utf8_lossy(&rejected.stderr)
                    .contains("release_floor_downgrade_or_sequence_reuse")
            );
            assert_eq!(committed, snapshot(&parent.join("policy")));
        }
    }
    assert!(
        candidate(high.path(), "--advance-release-floor", "policy")
            .status
            .success()
    );
    assert_eq!(committed, snapshot(&parent.join("policy")));
    for n in ["floor.a", "floor.b"] {
        let m = parent.join("policy").join(n).metadata().unwrap();
        assert_eq!(m.len(), 256 * 1024);
        assert!(m.blocks() * 512 >= m.len());
        assert_eq!(m.mode() & 0o777, 0o600);
    }
    let active = inspect(&parent, "policy", &f).unwrap();
    assert!(inspect(&parent, "policy", &f).is_err());
    assert!(active.check_candidate(&low_package).is_err());
    assert!(active.check_candidate(&reused_package).is_err());
    assert_eq!(
        receipt["floor"]["manifest_sha256"].as_str().unwrap(),
        high_package.manifest_sha256()
    );
    active.check_candidate(&high_package).unwrap();
    println!("ACTUAL_RELEASE_FLOOR_SEQUENTIAL_SAME_MANIFEST=true");
    let concurrent_failures = std::thread::scope(|scope| {
        let readers: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    (0..32)
                        .filter(|_| active.check_candidate(&high_package).is_err())
                        .count()
                })
            })
            .collect();
        readers
            .into_iter()
            .map(|r| r.join().unwrap())
            .sum::<usize>()
    });
    assert_eq!(
        concurrent_failures, 0,
        "valid concurrent policy reads shared a seek cursor"
    );
    println!("ACTUAL_RELEASE_FLOOR_CONCURRENT_READS=256");

    let other = Fixture::new();
    drop(active);
    assert!(
        ReleaseFloor::open(open(&parent), "policy", &other.trust, Platform::LinuxX86_64).is_err()
    );
    assert!(
        ReleaseFloor::open(
            open(&parent),
            "policy",
            &f.trust,
            Platform::WindowsWsl2X86_64
        )
        .is_err()
    );
    assert_eq!(committed, snapshot(&parent.join("policy")));

    // A copied policy has equal signed bytes but different current local files.
    let copied = parent.join("copied");
    std::fs::create_dir(&copied).unwrap();
    std::fs::set_permissions(&copied, std::fs::Permissions::from_mode(0o700)).unwrap();
    for n in ["owner.lock", "floor.a", "floor.b"] {
        std::fs::copy(parent.join("policy").join(n), copied.join(n)).unwrap();
        std::fs::set_permissions(copied.join(n), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(inspect(&parent, "copied", &f).is_err());
    assert_eq!(committed, snapshot(&parent.join("policy")));

    // A recomputed frame checksum is not a signature for a larger floor.
    let mut forged =
        ReleaseFloor::initialize(open(&parent), "forged", &f.trust, Platform::LinuxX86_64).unwrap();
    forged.advance(&high_package).unwrap();
    drop(forged);
    let path = parent.join("forged/floor.b");
    let bytes = std::fs::read(&path).unwrap();
    let length = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    let mut record: Value = serde_json::from_slice(&bytes[36..36 + length]).unwrap();
    record["release_sequence"] = json!(99);
    let payload = serde_json::to_vec(&record).unwrap();
    let mut bytes = vec![0; 256 * 1024];
    bytes[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes[4..36].copy_from_slice(&Sha256::digest(&payload));
    bytes[36..36 + payload.len()].copy_from_slice(&payload);
    std::fs::write(&path, bytes).unwrap();
    assert!(inspect(&parent, "forged", &f).is_err());

    // Neither an invalid latest slot nor an invalid older slot can lower floor.
    for (name, damaged) in [("torn-new", "floor.b"), ("torn-old", "floor.a")] {
        let mut policy =
            ReleaseFloor::initialize(open(&parent), name, &f.trust, Platform::LinuxX86_64).unwrap();
        policy.advance(&high_package).unwrap();
        drop(policy);
        let path = parent.join(name).join(damaged);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[40] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        let damaged_before = snapshot(&parent.join(name));
        assert!(inspect(&parent, name, &f).is_err());
        assert_eq!(damaged_before, snapshot(&parent.join(name)));
    }

    // Actual EFBIG on this test process, restored before assertions/stdio. A
    // failed owner only retries its retained exact frame; new policy use needs
    // an independently checked fresh Open, never a latch reset.
    let mut policy =
        ReleaseFloor::initialize(open(&parent), "pending", &f.trust, Platform::LinuxX86_64)
            .unwrap();
    let mut previous = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_FSIZE, &mut previous) },
        0
    );
    let previous_signal = unsafe { libc::signal(libc::SIGXFSZ, libc::SIG_IGN) };
    assert_ne!(previous_signal, libc::SIG_ERR);
    let zero = libc::rlimit {
        rlim_cur: 0,
        rlim_max: previous.rlim_max,
    };
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &zero) }, 0);
    let result = policy.advance(&high_package);
    let restore = unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &previous) };
    unsafe {
        libc::signal(libc::SIGXFSZ, previous_signal);
    }
    assert_eq!(restore, 0);
    assert!(
        matches!(result, Err(cowork_managed_runtime::Error::Io(ref e)) if e.raw_os_error() == Some(libc::EFBIG))
    );
    assert!(policy.check_candidate(&high_package).is_err());
    assert!(policy.advance(&reused_package).is_err());
    policy.advance(&high_package).unwrap();
    assert!(policy.receipt().is_err());
    drop(policy);
    let policy = inspect(&parent, "pending", &f).unwrap();
    assert!(policy.check_candidate(&low_package).is_err());
    assert!(policy.check_candidate(&high_package).is_ok());
    drop(policy);

    let mut policy =
        ReleaseFloor::initialize(open(&parent), "replaced", &f.trust, Platform::LinuxX86_64)
            .unwrap();
    std::fs::rename(parent.join("replaced"), parent.join("original-replaced")).unwrap();
    std::fs::create_dir(parent.join("replaced")).unwrap();
    std::fs::set_permissions(
        parent.join("replaced"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(policy.advance(&high_package).is_err());
    assert!(policy.receipt().is_err());
    let relocated = parent.join("relocated-parent");
    std::fs::create_dir(&relocated).unwrap();
    std::fs::set_permissions(&relocated, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::rename(parent.join("policy"), relocated.join("policy")).unwrap();
    let relocated_before = snapshot(&relocated.join("policy"));
    assert!(inspect(&relocated, "policy", &f).is_err());
    assert_eq!(relocated_before, snapshot(&relocated.join("policy")));
    println!("ACTUAL_RELEASE_FLOOR_POLICY_ONLY=true");
    println!("ACTUAL_RELEASE_FLOOR_EFBIG_AND_FAIL_CLOSED=true");
}
