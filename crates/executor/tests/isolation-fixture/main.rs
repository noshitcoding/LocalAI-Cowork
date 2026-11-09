//! Synthetic headless child: no static imports of user32/GDI or the host supervisor.
#[cfg(windows)]
mod probe {
    use std::{
        fs,
        io::{Read, Write},
        path::Path,
        process::Command,
        ptr,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::*,
        System::{JobObjects::*, LibraryLoader::*, Memory::*, Threading::*},
    };
    pub fn main() {
        let args: Vec<_> = std::env::args().skip(1).collect();
        match args[0].as_str() {
            "echo" => {
                let mut input = Vec::new();
                std::io::stdin().read_to_end(&mut input).unwrap();
                let mut job = 0;
                assert_ne!(
                    unsafe { IsProcessInJob(GetCurrentProcess(), ptr::null_mut(), &mut job) },
                    0
                );
                println!(
                    "{}",
                    serde_json::json!({"supervised_before_first_action":job!=0,"path_inherited":std::env::var_os("PATH").is_some(),"stdin_bytes":input.len()})
                );
            }
            "exit" => {
                fs::write(&args[1], b"executed once").unwrap();
                std::process::exit(37);
            }
            "heartbeat" => {
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(Path::new(&args[1]).join("heartbeat"))
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
            "isolation-files" => {
                assert_eq!(fs::read("input.txt").unwrap(), b"approved input");
                fs::write("output.txt", b"isolated output").unwrap();
                for path in &args[1..] {
                    assert_eq!(
                        fs::read(path).unwrap_err().kind(),
                        std::io::ErrorKind::PermissionDenied,
                        "read {path}"
                    );
                    assert_eq!(
                        fs::OpenOptions::new()
                            .write(true)
                            .open(path)
                            .unwrap_err()
                            .kind(),
                        std::io::ErrorKind::PermissionDenied,
                        "write {path}"
                    );
                }
                let executable = std::env::current_exe().unwrap();
                assert!(!fs::read(&executable).unwrap().is_empty());
                assert_eq!(
                    fs::OpenOptions::new()
                        .write(true)
                        .open(executable)
                        .unwrap_err()
                        .kind(),
                    std::io::ErrorKind::PermissionDenied
                );
                println!(
                    "{}",
                    serde_json::json!({"denied":args.len()-1,"workspace_write":true,"bundle_write_denied":true})
                );
            }
            "isolation-desktop" => {
                let mut flags = 0u32;
                assert_ne!(
                    unsafe {
                        GetProcessMitigationPolicy(
                            GetCurrentProcess(),
                            ProcessSystemCallDisablePolicy,
                            (&mut flags as *mut u32).cast(),
                            4,
                        )
                    },
                    0
                );
                assert_eq!(flags & 1, 1);
                let name: Vec<u16> = "user32.dll".encode_utf16().chain([0]).collect();
                let library = unsafe {
                    LoadLibraryExW(name.as_ptr(), ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32)
                };
                let error = unsafe { GetLastError() };
                if !library.is_null() {
                    unsafe {
                        FreeLibrary(library);
                    }
                }
                assert!(
                    library.is_null(),
                    "GUI library unexpectedly initialized despite Win32k restriction"
                );
                println!(
                    "{}",
                    serde_json::json!({"win32k_disabled":true,"gui_library_blocked":true,"code":error})
                );
            }
            "isolation-host-process" => {
                let process = unsafe {
                    OpenProcess(
                        PROCESS_VM_READ
                            | PROCESS_VM_WRITE
                            | PROCESS_CREATE_THREAD
                            | PROCESS_DUP_HANDLE,
                        0,
                        args[1].parse().unwrap(),
                    )
                };
                let error = unsafe { GetLastError() };
                if !process.is_null() {
                    unsafe {
                        CloseHandle(process);
                    }
                }
                assert!(process.is_null(), "unexpected host process access");
                assert_eq!(error, ERROR_ACCESS_DENIED);
                println!(
                    "{}",
                    serde_json::json!({"host_process_denied":true,"code":error})
                );
            }
            "isolation-network" => network(&args[1]),
            "isolation-limits" => limits(),
            "isolation-cpu" => cpu(),
            "isolation-child-probe" => child(),
            _ => panic!("unknown headless fixture mode"),
        }
    }
    fn network(address: &str) {
        use windows_sys::Win32::Networking::WinSock::*;
        let mut data: WSADATA = unsafe { std::mem::zeroed() };
        let startup = unsafe { WSAStartup(0x202, &mut data) };
        if startup != 0 {
            assert_eq!(startup, WSASYSCALLFAILURE);
            println!(
                "{}",
                serde_json::json!({"tcp_connected":false,"stage":"winsock_startup","code":startup})
            );
            return;
        }
        match std::net::TcpStream::connect_timeout(
            &address.parse().unwrap(),
            Duration::from_secs(2),
        ) {
            Ok(_) => panic!("unexpected network access"),
            Err(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
                println!(
                    "{}",
                    serde_json::json!({"tcp_connected":false,"stage":"connect_denied","code":e.raw_os_error()})
                );
            }
        }
        unsafe {
            WSACleanup();
        }
    }
    fn limits() {
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        assert_ne!(
            unsafe {
                QueryInformationJobObject(
                    ptr::null_mut(),
                    JobObjectExtendedLimitInformation,
                    (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                    ptr::null_mut(),
                )
            },
            0
        );
        let mut cpu: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { std::mem::zeroed() };
        assert_ne!(
            unsafe {
                QueryInformationJobObject(
                    ptr::null_mut(),
                    JobObjectCpuRateControlInformation,
                    (&mut cpu as *mut JOBOBJECT_CPU_RATE_CONTROL_INFORMATION).cast(),
                    std::mem::size_of_val(&cpu) as u32,
                    ptr::null_mut(),
                )
            },
            0
        );
        let allocation = unsafe {
            VirtualAlloc(
                ptr::null(),
                512 * 1024 * 1024,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        let error = unsafe { GetLastError() };
        if !allocation.is_null() {
            unsafe {
                VirtualFree(allocation, 0, MEM_RELEASE);
            }
        }
        assert!(allocation.is_null(), "allocation exceeded the job cap");
        let child_error = match Command::new(std::env::current_exe().unwrap())
            .arg("echo")
            .spawn()
        {
            Ok(mut child) => {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("process limit exceeded")
            }
            Err(e) => e.raw_os_error(),
        };
        println!(
            "{}",
            serde_json::json!({"memory":limits.JobMemoryLimit,"processes":limits.BasicLimitInformation.ActiveProcessLimit,"flags":limits.BasicLimitInformation.LimitFlags,"cpu_flags":cpu.ControlFlags,"cpu_rate":unsafe{cpu.Anonymous.CpuRate},"allocation_error":error,"child_error":child_error})
        );
    }
    fn cpu() {
        let begin = Instant::now();
        let mut n = 1u64;
        while begin.elapsed() < Duration::from_secs(4) {
            n = std::hint::black_box(n.wrapping_mul(37).wrapping_add(1));
        }
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        assert_ne!(
            unsafe {
                GetProcessTimes(
                    GetCurrentProcess(),
                    &mut created,
                    &mut exited,
                    &mut kernel,
                    &mut user,
                )
            },
            0
        );
        let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        println!(
            "{}",
            serde_json::json!({"wall_ms":begin.elapsed().as_millis(),"cpu_ms":(ticks(kernel)+ticks(user))/10_000,"n":n})
        );
    }
    fn child() {
        use std::os::windows::ffi::OsStrExt;
        let exe = std::env::current_exe().unwrap();
        let exe_w: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
        let mut command: Vec<u16> = format!("\"{}\" exit child-marker", exe.display())
            .encode_utf16()
            .chain([0])
            .collect();
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = std::mem::size_of_val(&startup) as u32;
        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let okay = unsafe {
            CreateProcessW(
                exe_w.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                CREATE_NO_WINDOW,
                ptr::null(),
                ptr::null(),
                &startup,
                &mut info,
            )
        };
        let error = unsafe { GetLastError() };
        if okay != 0 {
            unsafe {
                WaitForSingleObject(info.hProcess, 3000);
                TerminateProcess(info.hProcess, 1);
                WaitForSingleObject(info.hProcess, 1000);
                CloseHandle(info.hThread);
                CloseHandle(info.hProcess);
            }
        }
        println!(
            "{}",
            serde_json::json!({"spawn":okay,"code":error,"marker":fs::read("child-marker").ok()})
        );
    }
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("confined-probe") => confined::probe(&args[0], &args[2..]),
        Some("noop") => {}
        #[cfg(windows)]
        _ => probe::main(),
        #[cfg(not(windows))]
        _ => {}
    }
}

/// Observations under `cowork_executor::supervise_confined` on Windows and Linux. It never
/// asserts; the host test judges the report.
mod confined {
    use std::{fs, net::TcpStream, path::Path, time::Duration};
    fn outcome<T>(result: std::io::Result<T>) -> serde_json::Value {
        match result {
            Ok(_) => "ok".into(),
            Err(e) => format!("{:?}", e.kind()).into(),
        }
    }
    /// args: outside file, own writable directory, live host TCP address, host PID.
    pub fn probe(program: &str, args: &[String]) {
        let outside = Path::new(&args[0]);
        let own = Path::new(&args[1]);
        let mut report = serde_json::json!({
            "read_outside": outcome(fs::read(outside)),
            "append_outside": outcome(fs::OpenOptions::new().append(true).open(outside)),
            "create_beside_outside": outcome(fs::write(outside.with_extension("planted"), b"x")),
            "write_own": outcome(fs::write(own.join("own.txt"), b"confined output")),
            "connect": outcome(TcpStream::connect_timeout(
                &args[2].parse().unwrap(),
                Duration::from_secs(2),
            )),
            "spawn": outcome(std::process::Command::new(program).arg("noop").status()),
            "host_process": host_process(args[3].parse().unwrap()),
        });
        #[cfg(unix)]
        {
            report["symlink"] = outcome(std::os::unix::fs::symlink(outside, own.join("link")));
            report["hardlink"] = outcome(fs::hard_link(outside, own.join("hard")));
        }
        #[cfg(not(unix))]
        let _ = &mut report;
        println!("{report}");
    }
    #[cfg(windows)]
    fn host_process(pid: u32) -> serde_json::Value {
        use windows_sys::Win32::{Foundation::*, System::Threading::*};
        let process = unsafe {
            OpenProcess(
                PROCESS_TERMINATE | PROCESS_VM_READ | PROCESS_DUP_HANDLE,
                0,
                pid,
            )
        };
        if process.is_null() {
            return format!("denied {}", unsafe { GetLastError() }).into();
        }
        unsafe { CloseHandle(process) };
        "opened".into()
    }
    #[cfg(unix)]
    fn host_process(pid: u32) -> serde_json::Value {
        // Signal 0 only checks permission to signal.
        if unsafe { libc::kill(pid as i32, 0) } == 0 {
            return "signalable".into();
        }
        format!("{:?}", std::io::Error::last_os_error().kind()).into()
    }
}
