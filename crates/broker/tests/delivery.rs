use cowork_broker::delivery::{PreparedDelivery, validate_delivery};
use cowork_contracts::DownloadMetadata;
use cowork_domain::{Error, hash};
use std::{fs, path::Path};

fn metadata(bytes: &[u8]) -> DownloadMetadata {
    DownloadMetadata {
        file_name: "Prüfung 📄.bin".into(),
        size: bytes.len() as u64,
        sha256: hash(bytes),
    }
}
fn stages(root: &Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".cowork-save-")
        })
        .collect()
}

#[test]
fn backup_delivery_streams_above_normal_cap_and_preserves_normal_delivery_limit() {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("large.sqlite3");
    let size = cowork_contracts::MAX_DOWNLOAD_BYTES + 1;
    let block = [0x5au8; 65536];
    let mut hash = Sha256::new();
    let mut left = size;
    while left > 0 {
        let n = left.min(block.len() as u64) as usize;
        hash.update(&block[..n]);
        left -= n as u64;
    }
    let backup_metadata = DownloadMetadata {
        file_name: "backup.sqlite3".into(),
        size,
        sha256: format!("{:x}", hash.finalize()),
    };
    let mut source = std::io::repeat(0x5a).take(size);
    let pending =
        PreparedDelivery::prepare_backup(&target, &mut source, &backup_metadata, &[]).unwrap();
    assert!(!target.exists());
    let saved = pending.commit().unwrap();
    assert_eq!(saved.size, size);
    assert_eq!(fs::metadata(&target).unwrap().len(), size);
    assert!(
        PreparedDelivery::prepare(
            &target,
            b"ordinary small file",
            &metadata(b"ordinary small file"),
            &[]
        )
        .is_err()
    );
    let original = fs::metadata(&target).unwrap().len();
    let mut bad = std::io::Cursor::new(b"too short".as_slice());
    assert!(PreparedDelivery::prepare_backup(&target, &mut bad, &backup_metadata, &[]).is_err());
    assert_eq!(fs::metadata(&target).unwrap().len(), original);
    assert!(stages(temp.path()).is_empty());
}

#[test]
fn binary_empty_replacement_and_drop_preserve_confirmed_contents() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("Vom Nutzer gewählt 📄.bin");
    let bytes: Vec<u8> = (0..=255).cycle().take(2 * 1024 * 1024).collect();
    let pending = PreparedDelivery::prepare(&target, &bytes, &metadata(&bytes), &[]).unwrap();
    assert!(!target.exists());
    let saved = pending.commit().unwrap();
    assert_eq!(saved.file_name, "Vom Nutzer gewählt 📄.bin");
    assert_eq!(saved.sha256, hash(&fs::read(&target).unwrap()));
    let abandoned =
        PreparedDelivery::prepare(&target, b"discarded", &metadata(b"discarded"), &[]).unwrap();
    assert_eq!(fs::read(&target).unwrap(), bytes);
    drop(abandoned);
    assert!(stages(temp.path()).is_empty());
    PreparedDelivery::prepare(&target, b"", &metadata(b""), &[])
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"");
    assert!(stages(temp.path()).is_empty());
}

#[test]
fn invalid_integrity_paths_protected_data_and_hardlinks_never_stage_data() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("file.bin");
    let valid = metadata(b"new");
    for invalid in [
        DownloadMetadata {
            size: 4,
            ..valid.clone()
        },
        DownloadMetadata {
            sha256: hash(b"wrong"),
            ..valid.clone()
        },
    ] {
        assert!(PreparedDelivery::prepare(&target, b"new", &invalid, &[]).is_err());
    }
    for name in [
        "../outside",
        "nested/file.bin",
        "C:\\secret",
        ".cowork-save-private",
        "NUL",
        "file:stream",
    ] {
        assert!(
            validate_delivery(
                b"new",
                &DownloadMetadata {
                    file_name: name.into(),
                    ..valid.clone()
                }
            )
            .is_err(),
            "accepted {name}"
        );
    }
    assert!(matches!(
        PreparedDelivery::prepare(&target, b"new", &valid, &[temp.path().to_owned()]),
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        PreparedDelivery::prepare(Path::new("relative.bin"), b"new", &valid, &[]),
        Err(Error::Forbidden)
    ));
    fs::write(&target, b"original").unwrap();
    let alias = temp.path().join("alias.bin");
    fs::hard_link(&target, &alias).unwrap();
    assert!(matches!(
        PreparedDelivery::prepare(&target, b"new", &valid, &[]),
        Err(Error::Forbidden)
    ));
    assert_eq!(fs::read(&alias).unwrap(), b"original");
    assert!(stages(temp.path()).is_empty());
}

#[test]
fn changed_and_late_destinations_and_tampered_staging_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("output.bin");
    for existing in [false, true] {
        if existing {
            fs::write(&target, b"original").unwrap();
        }
        let pending = PreparedDelivery::prepare(&target, b"new", &metadata(b"new"), &[]).unwrap();
        fs::write(&target, b"external edit").unwrap();
        assert!(matches!(pending.commit(), Err(Error::FileConflict)));
        assert_eq!(fs::read(&target).unwrap(), b"external edit");
        assert!(stages(temp.path()).is_empty());
        fs::remove_file(&target).unwrap();
    }
    fs::write(&target, b"original").unwrap();
    let pending = PreparedDelivery::prepare(&target, b"new", &metadata(b"new"), &[]).unwrap();
    // Same content at a different native identity must also invalidate the selection.
    fs::rename(&target, temp.path().join("old.bin")).unwrap();
    fs::write(&target, b"original").unwrap();
    assert!(matches!(pending.commit(), Err(Error::FileConflict)));
    let pending = PreparedDelivery::prepare(&target, b"new", &metadata(b"new"), &[]).unwrap();
    fs::write(&stages(temp.path())[0], b"corrupt").unwrap();
    assert!(matches!(pending.commit(), Err(Error::FileConflict)));
    assert_eq!(fs::read(&target).unwrap(), b"original");
    assert!(stages(temp.path()).is_empty());
}

#[test]
fn readonly_target_is_preserved_and_foreign_stage_is_not_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("private.bin");
    fs::write(&target, b"original").unwrap();
    let original_permissions = fs::metadata(&target).unwrap().permissions();
    let mut restricted = original_permissions.clone();
    restricted.set_readonly(true);
    fs::set_permissions(&target, restricted).unwrap();
    assert!(matches!(
        PreparedDelivery::prepare(&target, b"new", &metadata(b"new"), &[]),
        Err(Error::Forbidden)
    ));
    fs::set_permissions(&target, original_permissions).unwrap();
    let pending = PreparedDelivery::prepare(&target, b"new", &metadata(b"new"), &[]).unwrap();
    let stage = stages(temp.path()).remove(0);
    fs::rename(&stage, temp.path().join("moved-stage.bin")).unwrap();
    fs::write(&stage, b"foreign replacement").unwrap();
    drop(pending);
    assert_eq!(fs::read(&stage).unwrap(), b"foreign replacement");
    assert_eq!(fs::read(&target).unwrap(), b"original");
}

#[cfg(unix)]
#[test]
fn unix_mode_and_symlink_boundaries_are_preserved() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("private.bin");
    fs::write(&target, b"original").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    PreparedDelivery::prepare(&target, b"new", &metadata(b"new"), &[])
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let alias = temp.path().join("alias.bin");
    symlink(&target, &alias).unwrap();
    assert!(matches!(
        PreparedDelivery::prepare(&alias, b"bad", &metadata(b"bad"), &[]),
        Err(Error::Forbidden)
    ));
    assert_eq!(fs::read(&target).unwrap(), b"new");
}

#[test]
#[ignore = "Child fixture, exercised by process_termination_preserves_old_or_confirmed_file"]
fn delivery_process_fixture() {
    use std::io::{BufRead, Write};
    let root = std::env::var_os("COWORK_DELIVERY_TEST_ROOT").expect("isolated test root required");
    let target = Path::new(&root).join("output.bin");
    let pending =
        PreparedDelivery::prepare(&target, b"confirmed", &metadata(b"confirmed"), &[]).unwrap();
    println!("DELIVERY_STAGED");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "commit");
    pending.commit().unwrap();
    println!("DELIVERY_CONFIRMED");
    std::io::stdout().flush().unwrap();
    std::io::stdin()
        .lock()
        .read_line(&mut String::new())
        .unwrap();
}

#[test]
fn process_termination_preserves_old_or_confirmed_file() {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
        sync::mpsc,
        time::Duration,
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("output.bin");
    fs::write(&target, b"original").unwrap();
    for commit in [false, true] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "delivery_process_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("COWORK_DELIVERY_TEST_ROOT", temp.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = Child(command.spawn().unwrap());
        let output = child.0.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if sender.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let wait_for = |marker| loop {
            if receiver
                .recv_timeout(Duration::from_secs(20))
                .expect("child fixture did not reach checkpoint")
                .contains(marker)
            {
                break;
            }
        };
        wait_for("DELIVERY_STAGED");
        assert_eq!(fs::read(&target).unwrap(), b"original");
        if commit {
            child
                .0
                .stdin
                .as_mut()
                .unwrap()
                .write_all(b"commit\n")
                .unwrap();
            child.0.stdin.as_mut().unwrap().flush().unwrap();
            wait_for("DELIVERY_CONFIRMED");
        }
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        reader.join().unwrap();
        assert_eq!(
            fs::read(&target).unwrap(),
            if commit {
                b"confirmed".as_slice()
            } else {
                b"original".as_slice()
            }
        );
    }
    // The killed unpublished stage is retained; no restart guesses or replays its intent.
    assert_eq!(stages(temp.path()).len(), 1);
    assert_eq!(fs::read(&stages(temp.path())[0]).unwrap(), b"confirmed");
}
