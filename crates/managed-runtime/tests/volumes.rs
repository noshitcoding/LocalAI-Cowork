#![cfg(target_os = "linux")]
//! These privileged tests run only inside an explicitly created, bounded own
//! loop fixture. No test recursively removes a mounted tree on failure.
use cap_std::{
    ambient_authority,
    fs::{Dir, Permissions, PermissionsExt},
};
use cowork_managed_runtime::volumes::*;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, PermissionsExt as _},
    path::{Path, PathBuf},
};

struct Fixture {
    pool: VolumePool,
    path: PathBuf,
}
fn formatter() -> PinnedFormatter {
    let path = Path::new("/usr/sbin/mke2fs").canonicalize().unwrap();
    let hash = format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap()));
    PinnedFormatter::open(&path, &hash).unwrap()
}
impl Fixture {
    fn new(bytes: u64, slots: u16) -> Self {
        assert_eq!(unsafe { libc::geteuid() }, 0);
        let root = PathBuf::from(
            std::env::var("COWORK_RUNTIME_VOLUME_PROOF_DIR")
                .expect("explicit private bounded proof mount required"),
        );
        assert_eq!(root.file_name().unwrap(), "case-storage");
        assert!(
            root.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("cowork-runtime-volumes-")
        );
        assert_eq!(
            root.parent().unwrap().parent().unwrap(),
            Path::new("/var/tmp")
        );
        assert_eq!(root.canonicalize().unwrap(), root);
        let m = std::fs::metadata(&root).unwrap();
        assert_eq!(m.uid(), 0);
        assert_eq!(m.mode() & 0o077, 0);
        let path = root.join(format!("case-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let dir = Dir::open_ambient_dir(&path, ambient_authority()).unwrap();
        let pool = VolumePool::initialize(
            dir,
            PoolLimits {
                reserved_bytes: bytes,
                max_volumes: slots,
            },
            formatter(),
        )
        .unwrap();
        Self { pool, path }
    }
    fn open(&self) -> VolumePool {
        VolumePool::open(
            Dir::open_ambient_dir(&self.path, ambient_authority()).unwrap(),
            formatter(),
        )
        .unwrap()
    }
    fn request(label: &str, bytes: u64, inodes: u32) -> VolumeRequest {
        VolumeRequest {
            request_sha256: format!("{:x}", Sha256::digest(label.as_bytes())),
            bytes,
            inodes,
        }
    }
}
const MIB: u64 = 1024 * 1024;

fn latest_journal(f: &Fixture, id: &str) -> (PathBuf, serde_json::Value) {
    ["volume.a", "volume.b"]
        .into_iter()
        .filter_map(|name| {
            let path = f.path.join("volumes").join(id).join(name);
            let bytes = std::fs::read(&path).unwrap();
            let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
            if len == 0
                || len > 8192 - 36
                || Sha256::digest(&bytes[36..36 + len]).as_slice() != &bytes[4..36]
            {
                return None;
            }
            Some((
                path,
                serde_json::from_slice::<serde_json::Value>(&bytes[36..36 + len]).unwrap(),
            ))
        })
        .max_by_key(|(_, record)| record["sequence"].as_u64().unwrap())
        .unwrap()
}
fn corrupt_latest(f: &Fixture, id: &str) {
    let (path, _) = latest_journal(f, id);
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.seek(SeekFrom::Start(4)).unwrap();
    let mut byte = [0u8];
    file.read_exact(&mut byte).unwrap();
    byte[0] ^= 0x80;
    file.seek(SeekFrom::Start(4)).unwrap();
    file.write_all(&byte).unwrap();
    file.sync_all().unwrap();
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn reserved_ext4_enforces_disk_bound_and_keeps_pool_journal_writable() {
    let f = Fixture::new(96 * MIB, 2);
    let volume = f
        .pool
        .reserve(Fixture::request("disk", 64 * MIB, 2048))
        .unwrap();
    let id = volume.id().to_owned();
    assert_eq!(f.pool.usage().unwrap().reserved_bytes, 64 * MIB);
    assert!(volume.usage().unwrap().capacity_bytes <= 64 * MIB);
    let mut file = volume.directory().create("full.data").unwrap();
    let mut count = 0;
    let block = [0u8; 65536];
    loop {
        match file.write_all(&block) {
            Ok(()) => count += block.len(),
            Err(error) => {
                assert_eq!(error.raw_os_error(), Some(libc::ENOSPC));
                break;
            }
        }
        assert!(count < 64 * 1024 * 1024);
    }
    drop(file);
    assert_eq!(volume.usage().unwrap().available_bytes, 0);
    assert!(f.path.join("volumes").join(&id).join("volume.a").is_file());
    volume.retire_unlaunched().unwrap();
    assert_eq!(f.pool.usage().unwrap().reserved_bytes, 0);
    f.pool.abort_unlaunched(&id).unwrap();
    println!(
        "physical_disk_bound=true; original_reserved_bytes={}; written_bytes={count}; retired=true",
        64 * MIB
    );
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn ext4_inode_limit_and_shared_pool_admission_are_physical() {
    let f = Fixture::new(64 * MIB, 1);
    let volume = f
        .pool
        .reserve(Fixture::request("inodes", 64 * MIB, 128))
        .unwrap();
    assert!(matches!(
        f.pool.reserve(Fixture::request("other", 16 * MIB, 128)),
        Err(VolumeError::Capacity)
    ));
    let capacity = volume.usage().unwrap().inode_capacity;
    assert!(capacity <= 128);
    let mut files = 0;
    for i in 0..512 {
        match volume.directory().create(format!("file-{i}")) {
            Ok(_) => files += 1,
            Err(error) => {
                assert_eq!(error.raw_os_error(), Some(libc::ENOSPC));
                break;
            }
        }
    }
    assert!(files < 128);
    assert_eq!(volume.usage().unwrap().available_inodes, 0);
    volume.retire_unlaunched().unwrap();
    let next = f
        .pool
        .reserve(Fixture::request("after-retirement", 16 * MIB, 128))
        .unwrap();
    next.retire_unlaunched().unwrap();
    println!("physical_inode_bound={capacity}; created_files={files}; shared_capacity_gate=true");
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn reopened_reservation_preserves_actual_files_and_the_original_receipt() {
    let f = Fixture::new(64 * MIB, 2);
    let request = Fixture::request("reopen", 32 * MIB, 128);
    let volume = f.pool.reserve(request.clone()).unwrap();
    let id = volume.id().to_owned();
    volume
        .directory()
        .write("edited.txt", b"retained edit")
        .unwrap();
    drop(volume);
    let reopened = f.open();
    let volume = reopened.reserve(request).unwrap();
    assert_eq!(volume.id(), id);
    let mut text = String::new();
    volume
        .directory()
        .open("edited.txt")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "retained edit");
    assert_eq!(reopened.usage().unwrap().reserved_volumes, 1);
    volume.retire_unlaunched().unwrap();
    assert_eq!(reopened.usage().unwrap().reserved_volumes, 0);
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn busy_unmount_and_replaced_native_image_never_free_the_reservation() {
    let f = Fixture::new(64 * MIB, 2);
    let volume = f
        .pool
        .reserve(Fixture::request("busy", 32 * MIB, 128))
        .unwrap();
    let id = volume.id().to_owned();
    let borrowed = volume.directory().try_clone().unwrap();
    assert!(volume.retire_unlaunched().is_err());
    assert_eq!(f.pool.usage().unwrap().reserved_bytes, 32 * MIB);
    drop(borrowed);
    let image = f.path.join("volumes").join(&id).join("image.ext4");
    let retained = image.with_extension("retained");
    std::fs::rename(&image, &retained).unwrap();
    std::fs::write(&image, b"replacement").unwrap();
    std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(f.pool.reopen_unlaunched(&id).is_err());
    assert!(f.pool.abort_unlaunched(&id).is_err());
    std::fs::remove_file(&image).unwrap();
    std::fs::rename(&retained, &image).unwrap();
    assert_eq!(f.pool.usage().unwrap().reserved_bytes, 32 * MIB);
    f.pool.abort_unlaunched(&id).unwrap();
    assert_eq!(f.pool.usage().unwrap().reserved_bytes, 0);
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn insufficient_backing_filesystem_fails_before_a_mounted_receipt_without_sparse_fallback() {
    let f = Fixture::new(32 * MIB, 1);
    let parent = f
        .pool
        .reserve(Fixture::request("small-parent", 16 * MIB, 128))
        .unwrap();
    {
        parent.directory().create_dir("child-pool").unwrap();
        parent
            .directory()
            .set_permissions("child-pool", Permissions::from_mode(0o700))
            .unwrap();
        let child = VolumePool::initialize(
            parent.directory().open_dir("child-pool").unwrap(),
            PoolLimits {
                reserved_bytes: 64 * MIB,
                max_volumes: 1,
            },
            formatter(),
        )
        .unwrap();
        assert!(
            child
                .reserve(Fixture::request("cannot-fit", 32 * MIB, 128))
                .is_err()
        );
        assert_eq!(child.usage().unwrap().reserved_bytes, 32 * MIB);
        let dir = parent.directory().open_dir("child-pool/volumes").unwrap();
        let id = dir
            .entries()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name()
            .to_string_lossy()
            .into_owned();
        drop(dir);
        assert!(child.reopen_unlaunched(&id).is_err());
        child.abort_unlaunched(&id).unwrap();
        assert_eq!(child.usage().unwrap().reserved_bytes, 0);
    }
    parent.retire_unlaunched().unwrap();
    println!(
        "preallocation_failure_before_receipt=true; failed_reservation_charged_until_explicit_abort=true"
    );
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn stale_pool_copy_and_unrecorded_entries_are_not_adopted() {
    let f = Fixture::new(64 * MIB, 2);
    let copy = f
        .path
        .with_file_name(format!("case-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&copy).unwrap();
    std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::copy(f.path.join("pool.json"), copy.join("pool.json")).unwrap();
    std::fs::copy(f.path.join("pool.lock"), copy.join("pool.lock")).unwrap();
    std::fs::create_dir(copy.join("volumes")).unwrap();
    std::fs::set_permissions(
        copy.join("pool.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    std::fs::set_permissions(
        copy.join("pool.lock"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    std::fs::set_permissions(copy.join("volumes"), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        VolumePool::open(
            Dir::open_ambient_dir(copy, ambient_authority()).unwrap(),
            formatter()
        )
        .is_err()
    );
    let orphan = f
        .path
        .join("volumes")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&orphan).unwrap();
    std::fs::set_permissions(&orphan, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(f.pool.usage().is_err());
    assert!(
        f.pool
            .reserve(Fixture::request("unrecorded", 32 * MIB, 128))
            .is_err()
    );
    std::fs::remove_dir(orphan).unwrap();
    assert_eq!(f.pool.usage().unwrap().reserved_bytes, 0);
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn published_workspaces_stay_booked_and_cannot_be_recreated_or_unlaunched() {
    let f = Fixture::new(32 * MIB, 1);
    let request = Fixture::request("published", 32 * MIB, 128);
    let volume = f.pool.reserve(request.clone()).unwrap();
    let id = volume.id().to_owned();
    let published = volume.publish_for_dispatch().unwrap();
    published
        .directory()
        .write("visible.txt", b"published copy")
        .unwrap();
    drop(published);
    assert!(f.pool.reopen_unlaunched(&id).is_err());
    assert!(f.pool.reserve(request).is_err());
    assert!(f.pool.abort_unlaunched(&id).is_err());
    assert_eq!(f.pool.usage().unwrap().reserved_bytes, 32 * MIB);
    // No task is launched by this library-only test. The external proof owner
    // cleans up its exact mount/loop after this process exits; production has
    // no published-volume release API until platform stop evidence is bound.
    println!(
        "published_volume_recovery_required=true; reservation_retained=true; no_task_launched=true; fixture_volume={id}"
    );
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn a_successful_loop_clear_request_is_not_a_physical_detachment_receipt() {
    let f = Fixture::new(32 * MIB, 1);
    let volume = f
        .pool
        .reserve(Fixture::request("held-loop", 32 * MIB, 128))
        .unwrap();
    let id = volume.id().to_owned();
    let (_, record) = latest_journal(&f, &id);
    let number = record["device"]["number"].as_u64().unwrap();
    let observer = std::fs::File::open(format!("/dev/loop{number}")).unwrap();
    assert!(matches!(
        volume.retire_unlaunched(),
        Err(VolumeError::StillAttached)
    ));
    assert_eq!(f.pool.usage().unwrap().reserved_bytes, 32 * MIB);
    drop(observer);
    f.pool.abort_unlaunched(&id).unwrap();
    assert_eq!(f.pool.usage().unwrap().reserved_bytes, 0);
    println!("clear_ioctl_alone_does_not_release=true; positive_detachment_required=true");
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn a_torn_prelaunch_journal_is_cleanup_only_and_never_reformats_a_workspace() {
    let f = Fixture::new(32 * MIB, 1);
    let request = Fixture::request("torn-prelaunch", 32 * MIB, 128);
    let volume = f.pool.reserve(request.clone()).unwrap();
    let id = volume.id().to_owned();
    drop(volume);
    corrupt_latest(&f, &id);
    assert!(f.pool.reserve(request).is_err());
    assert!(f.pool.reopen_unlaunched(&id).is_err());
    assert_eq!(f.pool.usage().unwrap().reserved_bytes, 32 * MIB);
    f.pool.abort_unlaunched(&id).unwrap();
    assert_eq!(f.pool.usage().unwrap().reserved_bytes, 0);
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn a_torn_published_journal_cannot_downgrade_the_one_way_dispatch_fence() {
    let f = Fixture::new(32 * MIB, 1);
    let request = Fixture::request("torn-published", 32 * MIB, 128);
    let volume = f.pool.reserve(request.clone()).unwrap();
    let id = volume.id().to_owned();
    drop(volume.publish_for_dispatch().unwrap());
    corrupt_latest(&f, &id);
    assert!(f.pool.reserve(request).is_err());
    assert!(f.pool.reopen_unlaunched(&id).is_err());
    assert!(f.pool.abort_unlaunched(&id).is_err());
    assert!(
        f.pool
            .reserve(Fixture::request("cannot-reuse-slot", 16 * MIB, 128))
            .is_err()
    );
    assert!(f.path.join("volumes").join(id).join("image.ext4").is_file());
    println!("corrupt_latest_record_preserves_dispatch_fence=true; no_task_launched=true");
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn independent_pool_instances_serialize_competing_physical_reservations() {
    let f = Fixture::new(32 * MIB, 1);
    let first = f.open();
    let second = f.open();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let one = barrier.clone();
    let a = std::thread::spawn(move || {
        one.wait();
        first.reserve(Fixture::request("concurrent-a", 32 * MIB, 128))
    });
    let b = std::thread::spawn(move || {
        barrier.wait();
        second.reserve(Fixture::request("concurrent-b", 32 * MIB, 128))
    });
    let results = [a.join().unwrap(), b.join().unwrap()];
    let mut accepted = 0;
    for result in results {
        match result {
            Ok(volume) => {
                accepted += 1;
                volume.retire_unlaunched().unwrap();
            }
            Err(VolumeError::Busy | VolumeError::Capacity) => {}
            Err(error) => panic!("{error:?}"),
        }
    }
    assert_eq!(accepted, 1);
    assert_eq!(f.pool.usage().unwrap().reserved_bytes, 0);
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn the_unprivileged_data_root_is_distinct_from_private_control_storage() {
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    let f = Fixture::new(32 * MIB, 1);
    let volume = f
        .pool
        .reserve(Fixture::request("uid-data-root", 32 * MIB, 128))
        .unwrap();
    let data = volume.data_directory().unwrap();
    let fd = data.as_raw_fd();
    let mut child = std::process::Command::new("/usr/bin/python3");
    child.env_clear().args(["-c", "import os; assert os.geteuid()==1000; open('uid-proof.txt','w').write('uid 1000 data');\ntry: os.listdir('..')\nexcept PermissionError: pass\nelse: raise RuntimeError('private control root is readable')"]);
    unsafe {
        child.pre_exec(move || {
            if libc::fchdir(fd) < 0
                || libc::setgroups(0, std::ptr::null()) < 0
                || libc::setgid(1000) < 0
                || libc::setuid(1000) < 0
                || libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(data.read("uid-proof.txt").unwrap(), b"uid 1000 data");
    assert_eq!(
        data.open("uid-proof.txt")
            .unwrap()
            .into_std()
            .metadata()
            .unwrap()
            .uid(),
        1000
    );
    drop(data);
    volume.retire_unlaunched().unwrap();
    println!(
        "trusted_uid_probe=true; workload_data_uid=1000; private_control_root_not_exposed=true; no_sandbox_claim=true"
    );
}

#[test]
#[ignore = "Requires the root-only bounded runtime-volume fixture"]
fn unsupported_backing_filesystems_are_rejected_before_pool_initialization() {
    // This creates only an empty own directory on existing tmpfs; it neither
    // mounts nor reconfigures /dev/shm and allocates no volume outside the proof.
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let directory = tempfile::Builder::new()
        .prefix("cowork-volume-unsupported-")
        .tempdir_in("/dev/shm")
        .unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let capability = Dir::open_ambient_dir(directory.path(), ambient_authority()).unwrap();
    assert!(matches!(
        VolumePool::initialize(
            capability,
            PoolLimits {
                reserved_bytes: 32 * MIB,
                max_volumes: 1
            },
            formatter()
        ),
        Err(VolumeError::Limits)
    ));
    assert!(
        std::fs::read_dir(directory.path())
            .unwrap()
            .next()
            .is_none()
    );
    directory.close().unwrap();
    println!(
        "unsupported_tmpfs_backing_rejected=true; no_weaker_fallback=true; own_empty_directory_removed=true"
    );
}
