//! Own test executable only. No fault knobs are compiled into production APIs.
use super::*;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;

#[test]
#[ignore = "only the owned private ext4 supervisor may run this worker"]
fn owned_ext4_publication_crash_worker() {
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let parent =
        std::path::PathBuf::from(std::env::var_os("COWORK_JOURNAL_CRASH_DIRECTORY").unwrap());
    let mode = std::env::var("COWORK_JOURNAL_CRASH_MODE").unwrap();
    assert!(parent.is_absolute());
    let (f, source) = package_data(&data());
    let key = parent.join("trusted.pub");
    let mut keyfile = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&key)
        .unwrap();
    keyfile.write_all(f.key.public_key().as_ref()).unwrap();
    keyfile.sync_all().unwrap();
    drop(keyfile);
    let versions = parent.join("versions");
    std::fs::create_dir(&versions).unwrap();
    std::fs::set_permissions(&versions, std::fs::Permissions::from_mode(0o700)).unwrap();
    if mode == "enospc" {
        use std::os::fd::AsRawFd;
        let mut fs = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
        let directory = std::fs::File::open(&parent).unwrap();
        assert_eq!(
            unsafe { libc::fstatvfs(directory.as_raw_fd(), fs.as_mut_ptr()) },
            0
        );
        let fs = unsafe { fs.assume_init() };
        let available = fs.f_bavail.checked_mul(fs.f_frsize).unwrap();
        assert!(available > 1024 * 1024);
        let filler = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(parent.join("own-filler"))
            .unwrap();
        assert_eq!(
            unsafe {
                libc::fallocate(
                    filler.as_raw_fd(),
                    0,
                    0,
                    (available - 512 * 1024) as libc::off_t,
                )
            },
            0
        );
        filler.sync_all().unwrap();
        let area = StagingArea::create(open(&parent), "attempt", 2 * 1024 * 1024).unwrap();
        let result = PublicationAttempt::prepare(
            area,
            &open(source.path()),
            &f.trust,
            Platform::LinuxX86_64,
            policy(),
        );
        assert!(
            matches!(result, Err(Error::Io(ref error)) if error.raw_os_error() == Some(libc::ENOSPC))
        );
        assert!(parent.join("attempt-journal/owner.lock").is_file());
        assert!(parent.join("attempt-journal/state.a").is_file());
        assert!(parent.join("attempt-journal/state.b").is_file());
        assert_eq!(std::fs::read_dir(&versions).unwrap().count(), 0);
        println!("JOURNAL_ACTUAL_ENOSPC_BEFORE_DISPATCH=true");
    } else {
        let mut publication = attempt(&parent, "attempt", &f, source.path());
        match mode.as_str() {
            "prepared" => {}
            "intent" => publication.arm_publication(open(&versions)).unwrap(),
            "renamed" | "first-parent-sync" | "second-parent-sync" => {
                publication.begin_publication(open(&versions)).unwrap()
            }
            "complete" => publication.publish(open(&versions)).unwrap(),
            "intent-io-error" => {
                let mut limit = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                assert_eq!(
                    unsafe { libc::getrlimit(libc::RLIMIT_FSIZE, &mut limit) },
                    0
                );
                let restricted = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: limit.rlim_max,
                };
                let old_signal = unsafe { libc::signal(libc::SIGXFSZ, libc::SIG_IGN) };
                assert_ne!(old_signal, libc::SIG_ERR);
                assert_eq!(
                    unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &restricted) },
                    0
                );
                let failed = publication.arm_publication(open(&versions));
                assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &limit) }, 0);
                unsafe {
                    libc::signal(libc::SIGXFSZ, old_signal);
                }
                assert!(
                    matches!(failed, Err(Error::Io(ref error)) if error.raw_os_error() == Some(libc::EFBIG))
                );
                assert!(publication.dispatch_publication().is_err());
                assert!(publication.publish(open(&versions)).is_err());
                assert!(parent.join("attempt").is_dir());
                assert_eq!(std::fs::read_dir(&versions).unwrap().count(), 0);
                println!("JOURNAL_ACTUAL_INTENT_IO_ERROR_NO_DISPATCH=true");
            }
            _ => panic!("unknown owned crash mode"),
        }
        let tid = unsafe { libc::syscall(libc::SYS_gettid) };
        println!(
            "JOURNAL_WORKER_READY={}",
            json!({"mode":mode,"pid":std::process::id(),"tid":tid})
        );
        std::io::stdout().flush().unwrap();
        if mode.ends_with("parent-sync") {
            // Parent traces only this exact owned test thread. It stops after
            // a real successful parent fsync, without production fault hooks.
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
            publication.observe_publication().unwrap();
            panic!("supervisor failed to terminate the selected original fsync boundary");
        }
        let mut byte = [0];
        std::io::stdin().read_exact(&mut byte).unwrap();
        return;
    }
    println!(
        "JOURNAL_WORKER_READY={}",
        json!({"mode":mode,"pid":std::process::id(),"tid":unsafe { libc::syscall(libc::SYS_gettid) }})
    );
    std::io::stdout().flush().unwrap();
    let mut byte = [0];
    std::io::stdin().read_exact(&mut byte).unwrap();
}
