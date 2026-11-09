//! Test binary only; production release-policy APIs have no fault switches.
use super::*;
use std::io::Write;
use std::os::fd::AsRawFd;

#[test]
#[ignore = "only the owned private ext4 supervisor may run this worker"]
fn owned_ext4_release_floor_crash_worker() {
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let parent =
        std::path::PathBuf::from(std::env::var_os("COWORK_JOURNAL_CRASH_DIRECTORY").unwrap());
    let mode = std::env::var("COWORK_JOURNAL_CRASH_MODE").unwrap();
    let mut f = Fixture::new();
    f.manifest["release_sequence"] = json!(7);
    let low = f.directory();
    let lower = verify_directory(&open(low.path()), &f.trust, Platform::LinuxX86_64).unwrap();
    f.manifest["release_sequence"] = json!(11);
    let high = f.directory();
    let higher = verify_directory(&open(high.path()), &f.trust, Platform::LinuxX86_64).unwrap();
    let mut key = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(parent.join("trusted.pub"))
        .unwrap();
    key.write_all(f.key.public_key().as_ref()).unwrap();
    key.sync_all().unwrap();
    drop(key);
    if mode == "enospc" {
        let directory = std::fs::File::open(&parent).unwrap();
        let mut fs = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
        assert_eq!(
            unsafe { libc::fstatvfs(directory.as_raw_fd(), fs.as_mut_ptr()) },
            0
        );
        let fs = unsafe { fs.assume_init() };
        let available = fs.f_bavail.checked_mul(fs.f_frsize).unwrap();
        assert!(available > 1024 * 1024);
        let filler = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(parent.join("own-filler"))
            .unwrap();
        assert_eq!(
            unsafe {
                libc::fallocate(
                    filler.as_raw_fd(),
                    0,
                    0,
                    (available - 384 * 1024) as libc::off_t,
                )
            },
            0
        );
        filler.sync_all().unwrap();
        let failed =
            ReleaseFloor::initialize(open(&parent), "floor", &f.trust, Platform::LinuxX86_64);
        assert!(
            matches!(failed, Err(cowork_managed_runtime::Error::Io(ref e)) if e.raw_os_error()==Some(libc::ENOSPC))
        );
        for name in ["owner.lock", "floor.a", "floor.b"] {
            assert!(parent.join("floor").join(name).is_file());
        }
        println!("RELEASE_FLOOR_ACTUAL_INITIAL_ENOSPC=true");
        ready_and_wait(&mode);
        return;
    }
    let mut policy =
        ReleaseFloor::initialize(open(&parent), "floor", &f.trust, Platform::LinuxX86_64).unwrap();
    if mode == "initialized" {
        ready_and_wait(&mode);
        return;
    }
    policy.advance(&lower).unwrap();
    match mode.as_str() {
        "advanced" => {}
        "before-write" => {
            policy.check_candidate(&higher).unwrap();
        }
        "complete" => {
            policy.advance(&higher).unwrap();
        }
        "torn-write" => {
            let mut previous = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            assert_eq!(
                unsafe { libc::getrlimit(libc::RLIMIT_FSIZE, &mut previous) },
                0
            );
            let old_signal = unsafe { libc::signal(libc::SIGXFSZ, libc::SIG_IGN) };
            assert_ne!(old_signal, libc::SIG_ERR);
            let limit = libc::rlimit {
                rlim_cur: 128,
                rlim_max: previous.rlim_max,
            };
            assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &limit) }, 0);
            let failed = policy.advance(&higher);
            let restored = unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &previous) };
            unsafe {
                libc::signal(libc::SIGXFSZ, old_signal);
            }
            assert_eq!(restored, 0);
            assert!(
                matches!(failed, Err(cowork_managed_runtime::Error::Io(ref e)) if e.raw_os_error()==Some(libc::EFBIG))
            );
            assert!(policy.check_candidate(&lower).is_err());
            assert!(policy.check_candidate(&higher).is_err());
            println!("RELEASE_FLOOR_ACTUAL_PARTIAL_WRITE_EFBIG=true");
        }
        "after-write" | "after-sync" => {
            ready(&mode);
            assert_eq!(
                unsafe {
                    libc::ptrace(
                        libc::PTRACE_TRACEME,
                        0,
                        std::ptr::null_mut::<libc::c_void>(),
                        std::ptr::null_mut::<libc::c_void>(),
                    )
                },
                0
            );
            assert_eq!(unsafe { libc::raise(libc::SIGSTOP) }, 0);
            policy.advance(&higher).unwrap();
            panic!("supervisor failed to stop the exact original floor write/fsync boundary");
        }
        _ => panic!("unknown owned release-floor crash mode"),
    }
    ready_and_wait(&mode);
}

fn ready(mode: &str) {
    println!(
        "FLOOR_WORKER_READY={}",
        json!({"mode":mode, "pid":std::process::id(),
        "tid":unsafe { libc::syscall(libc::SYS_gettid) }})
    );
    std::io::stdout().flush().unwrap();
}
fn ready_and_wait(mode: &str) {
    ready(mode);
    let mut byte = [0];
    std::io::stdin().read_exact(&mut byte).unwrap();
}
