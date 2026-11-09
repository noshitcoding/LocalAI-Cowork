//! Synthetic trusted child for executor integration tests. Never staged as an app tool.
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
#[cfg(windows)]
mod isolation;
fn heartbeat(path: &Path) {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .unwrap();
    writeln!(file, "{}", std::process::id()).unwrap();
    file.flush().unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        file.write_all(b".").unwrap();
        file.flush().unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn launch(directory: PathBuf, mode: &str) -> cowork_executor::TrustedLaunch {
    cowork_executor::TrustedLaunch {
        program: std::env::current_exe().unwrap(),
        arguments: vec![mode.into(), directory.to_string_lossy().into_owned()],
        directory,
        environment: Default::default(),
        timeout: Duration::from_secs(15),
        capture_bytes_per_stream: 4096,
    }
}
fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if arguments[0].starts_with("isolation-") {
        isolation::run(&arguments);
        return;
    }
    match arguments[0].as_str() {
        "echo" => {
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input).unwrap();
            #[cfg(windows)]
            let supervised = {
                let mut yes = 0;
                assert_ne!(
                    unsafe {
                        windows_sys::Win32::System::JobObjects::IsProcessInJob(
                            windows_sys::Win32::System::Threading::GetCurrentProcess(),
                            std::ptr::null_mut(),
                            &mut yes,
                        )
                    },
                    0
                );
                yes != 0
            };
            #[cfg(target_os = "linux")]
            let supervised = unsafe {
                libc::getpgrp() == libc::getpid()
                    && libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1
            };
            println!(
                "{}",
                serde_json::json!({"arguments":arguments[1..],"directory":std::env::current_dir().unwrap(),"path_inherited":std::env::var_os("PATH").is_some(),"private_environment_inherited":std::env::var_os("COWORK_EXECUTOR_PRIVATE_FIXTURE").is_some(),"marker":std::env::var("COWORK_EXECUTOR_MARKER").ok(),"stdin_bytes":input.len(),"supervised_before_first_action":supervised})
            );
        }
        "exit" => {
            std::fs::write(&arguments[1], b"executed once").unwrap();
            std::process::exit(37);
        }
        "flood" => {
            let out = std::thread::spawn(|| {
                let mut stream = std::io::stdout().lock();
                stream.write_all(&[0xff]).unwrap();
                for _ in 0..100 {
                    stream.write_all(&[b'o'; 65536]).unwrap();
                }
            });
            let err = std::thread::spawn(|| {
                let mut stream = std::io::stderr().lock();
                stream.write_all(&[0xfe]).unwrap();
                for _ in 0..100 {
                    stream.write_all(&[b'e'; 65536]).unwrap();
                }
            });
            out.join().unwrap();
            err.join().unwrap();
        }
        "endless" => {
            let mut stream = std::io::stdout().lock();
            loop {
                if stream.write_all(&[b'x'; 65536]).is_err() {
                    break;
                }
            }
        }
        "heartbeat" => heartbeat(&Path::new(&arguments[1]).join("heartbeat")),
        "tree" | "root-exit" => {
            let directory = Path::new(&arguments[1]);
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .arg("heartbeat")
                .arg(directory)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
            }
            #[expect(
                clippy::zombie_processes,
                reason = "This fixture deliberately exits before its child to exercise supervisor cleanup; production supervisor waits and reaps."
            )]
            let mut child = command.spawn().unwrap();
            let until = Instant::now() + Duration::from_secs(3);
            while !directory.join("heartbeat").exists() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(directory.join("heartbeat").exists());
            if arguments[0] == "tree" {
                child.wait().unwrap();
            } else {
                // Deliberately orphan this fixture child; the supervisor must terminate it.
                std::process::exit(0);
            }
        }
        "supervisor-tree" | "supervisor-single" => {
            let mode = if arguments[0] == "supervisor-tree" {
                "tree"
            } else {
                "heartbeat"
            };
            let result = cowork_executor::supervise(
                &launch(PathBuf::from(&arguments[1]), mode),
                &AtomicBool::new(false),
            )
            .unwrap();
            println!("{}", serde_json::to_string(&result).unwrap());
        }
        "supervisor-echo" => {
            let result = cowork_executor::supervise(
                &launch(PathBuf::from(&arguments[1]), "echo"),
                &AtomicBool::new(false),
            )
            .unwrap();
            std::io::stdout().write_all(&result.stdout.bytes).unwrap();
        }
        #[cfg(windows)]
        "foreign-handle" => {
            let foreign =
                arguments[1].parse::<usize>().unwrap() as windows_sys::Win32::Foundation::HANDLE;
            unsafe {
                windows_sys::Win32::System::Threading::SetEvent(foreign);
            }
        }
        #[cfg(windows)]
        "breakaway" => {
            use std::os::windows::process::CommandExt;
            match Command::new(std::env::current_exe().unwrap())
                .arg("heartbeat")
                .arg(&arguments[1])
                .creation_flags(
                    windows_sys::Win32::System::Threading::CREATE_BREAKAWAY_FROM_JOB
                        | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
                )
                .spawn()
            {
                Ok(mut child) => {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("unexpected job breakaway")
                }
                Err(error) => {
                    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
                    println!("breakaway denied");
                }
            }
        }
        _ => panic!("unknown synthetic fixture mode"),
    }
}
