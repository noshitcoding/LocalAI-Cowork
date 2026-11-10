use super::*;
#[path = "journal/crash.rs"]
mod crash;

#[test]
#[ignore = "requires own root-only bounded journal fixture"]
fn actual_cli_publication_requires_its_durable_original_journal() {
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let parent =
        std::path::PathBuf::from(std::env::var_os("COWORK_STAGING_ENOSPC_PARENT").unwrap());
    let (f, source) = package_data(&data());
    let keys = tempfile::tempdir().unwrap();
    let key = keys.path().join("trusted.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let output =
        std::process::Command::new(std::env::var_os("COWORK_STAGING_PACKAGE_BINARY").unwrap())
            .arg(source.path())
            .arg(&key)
            .args(["fixture-only", "linux_x86_64", "--publish-installation"])
            .arg(&parent)
            .arg("journal-case")
            .arg(&parent)
            .args(["2097152", "1048576", "27", "1024", "8192"])
            .output()
            .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["published"], true);
    let version = receipt["version_directory"].as_str().unwrap();
    let root = parent.join(version);
    assert!(root.join("package/manifest.json").is_file());
    for (i, vector) in role_vectors().iter().enumerate() {
        let file = root
            .join("runtime")
            .join(format!("component-{i}"))
            .join(vector["file"].as_str().unwrap());
        assert_eq!(
            std::fs::read(file).unwrap(),
            vector["body"].as_str().unwrap().as_bytes()
        );
    }
    let journal = parent.join("journal-case-journal");
    if !journal.exists() {
        println!("ACTUAL_CLI_PUBLISHED_WITHOUT_DURABLE_JOURNAL=true");
    }
    assert!(
        journal.is_dir(),
        "actual publication succeeded without the required durable original journal"
    );
    for name in ["owner.lock", "state.a", "state.b"] {
        assert!(journal.join(name).is_file());
    }
    println!("ACTUAL_CLI_DURABLE_ORIGINAL_JOURNAL=true");
}

#[test]
#[ignore = "requires own root-only bounded status fixture"]
fn actual_history_reads_never_dispatch_or_initialize_and_require_independent_trust() {
    use cowork_managed_runtime::staging::archive::read_publication_history;
    use std::io::{Read, Seek, SeekFrom, Write};
    let parent =
        std::path::PathBuf::from(std::env::var_os("COWORK_STAGING_ENOSPC_PARENT").unwrap());
    let (f, source) = package_data(&data());
    let mut active = attempt(&parent, "history", &f, source.path());
    let journal = parent.join("history-journal");
    assert!(read_publication_history(open(&journal), &f.trust, Platform::LinuxX86_64).is_err());
    active.arm_publication(open(&parent)).unwrap();
    assert_eq!(active.phase(), PublicationPhase::IntentConfirmed);
    let version = serde_json::to_value(active.receipt().unwrap()).unwrap()["version_directory"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!parent.join(&version).exists());
    drop(active);
    let view = serde_json::to_value(
        read_publication_history(open(&journal), &f.trust, Platform::LinuxX86_64).unwrap(),
    )
    .unwrap();
    assert_eq!(view["last_valid_phase"], "intent");
    assert_eq!(view["status_only"], true);
    assert_eq!(
        view["physical_publication_unconfirmed_after_process_loss"],
        true
    );
    for name in [
        "can_dispatch",
        "can_activate",
        "can_reclaim",
        "execution_authority",
        "native_custody_reconstructible",
    ] {
        assert_eq!(view[name], false);
    }
    assert!(!parent.join(&version).exists());
    assert!(parent.join("history").is_dir());
    let other = Fixture::new();
    assert!(read_publication_history(open(&journal), &other.trust, Platform::LinuxX86_64).is_err());
    let keys = tempfile::tempdir().unwrap();
    let key = keys.path().join("trusted.pub");
    std::fs::write(&key, f.key.public_key().as_ref()).unwrap();
    let run = |dir: &std::path::Path, key: &std::path::Path| {
        std::process::Command::new(std::env::var_os("COWORK_STAGING_PACKAGE_BINARY").unwrap())
            .arg("--publication-status")
            .arg(dir)
            .arg(key)
            .args(["fixture-only", "linux_x86_64"])
            .output()
            .unwrap()
    };
    let result = run(&journal, &key);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let cli: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(cli, view);
    let missing = parent.join("missing-journal");
    assert!(!run(&missing, &key).status.success());
    assert!(!missing.exists());
    assert!(
        !run(&journal, &keys.path().join("missing-key"))
            .status
            .success()
    );
    assert!(!keys.path().join("missing-key").exists());
    assert!(!parent.join(&version).exists());
    println!("PUBLICATION_HISTORY_STATUS_ONLY_AND_NO_INIT=true");
    println!("PUBLICATION_HISTORY_INDEPENDENT_TRUST_REQUIRED=true");

    let mut complete = attempt(&parent, "complete-history", &f, source.path());
    complete.publish(open(&parent)).unwrap();
    drop(complete);
    let completed_journal = parent.join("complete-history-journal");
    let view = serde_json::to_value(
        read_publication_history(open(&completed_journal), &f.trust, Platform::LinuxX86_64)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(view["last_valid_phase"], "published");
    assert_eq!(view["can_dispatch"], false);
    assert_eq!(view["can_reclaim"], false);
    // Exact-size torn/corrupted last frame: prior intent is diagnostic history
    // only. It may not authorize another physical operation.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(completed_journal.join("state.b"))
        .unwrap();
    file.seek(SeekFrom::Start(36)).unwrap();
    let mut byte = [0];
    file.read_exact(&mut byte).unwrap();
    byte[0] ^= 1;
    file.seek(SeekFrom::Start(36)).unwrap();
    file.write_all(&byte).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let degraded = serde_json::to_value(
        read_publication_history(open(&completed_journal), &f.trust, Platform::LinuxX86_64)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(degraded["last_valid_phase"], "intent");
    assert_eq!(degraded["frames_degraded"], true);
    assert_eq!(degraded["can_dispatch"], false);
    std::fs::remove_file(completed_journal.join("state.a")).unwrap();
    assert!(
        read_publication_history(open(&completed_journal), &f.trust, Platform::LinuxX86_64)
            .is_err()
    );
    assert!(!completed_journal.join("state.a").exists());
    println!("PUBLICATION_HISTORY_CORRUPT_OR_MISSING_STATE_FAILS_CLOSED=true");
}
