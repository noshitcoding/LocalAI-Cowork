//! Read-only OS listener ownership, bound to the supervisor's original child.
use anyhow::{Result, bail};
use std::net::{IpAddr, SocketAddr};

pub struct BackendOwner {
    pid: u32,
    #[cfg(windows)]
    process: std::os::windows::io::OwnedHandle,
    #[cfg(target_os = "linux")]
    pidfd: std::os::fd::OwnedFd,
}
impl BackendOwner {
    /// Called synchronously from the original supervisor spawn observation;
    /// the original child cannot have been reaped/reused while this runs.
    pub fn capture(pid: u32) -> Result<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::io::FromRawHandle;
            use windows_sys::Win32::System::Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            };
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                )
            };
            if handle.is_null() {
                bail!("Cannot pin original owned process")
            }
            Ok(Self {
                pid,
                process: unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle) },
            })
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::FromRawFd;
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
            if fd < 0 {
                bail!("Kernel cannot pin original owned process")
            }
            Ok(Self {
                pid,
                pidfd: unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) },
            })
        }
    }
    fn alive(&self) -> Result<()> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::{
                Foundation::WAIT_TIMEOUT, System::Threading::WaitForSingleObject,
            };
            if unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } != WAIT_TIMEOUT {
                bail!("Original backend process is no longer alive")
            }
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let mut fd = libc::pollfd {
                fd: self.pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut fd, 1, 0) } != 0 {
                bail!("Original backend process is no longer alive")
            }
        }
        Ok(())
    }
    pub fn assert_listener(&self, address: SocketAddr) -> Result<()> {
        self.alive()?;
        let IpAddr::V4(ip) = address.ip() else {
            bail!("Owned backend uses an IPv4 loopback listener")
        };
        if !ip.is_loopback() {
            bail!("Owned backend loopback required")
        }
        #[cfg(windows)]
        let owned = {
            use windows_sys::Win32::NetworkManagement::IpHelper::{
                GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
            };
            let mut size = 0u32;
            unsafe {
                GetExtendedTcpTable(
                    std::ptr::null_mut(),
                    &mut size,
                    0,
                    2,
                    TCP_TABLE_OWNER_PID_LISTENER,
                    0,
                )
            };
            if !(4..=8 * 1024 * 1024).contains(&size) {
                bail!("Listener inventory bound")
            }
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            if unsafe {
                GetExtendedTcpTable(
                    buffer.as_mut_ptr().cast(),
                    &mut size,
                    0,
                    2,
                    TCP_TABLE_OWNER_PID_LISTENER,
                    0,
                )
            } != 0
            {
                bail!("Cannot inspect original listener owner")
            }
            let count = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<u32>()) } as usize;
            let stride = std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
            if size as usize > buffer.len() * 8
                || count > (size as usize).saturating_sub(4) / stride
            {
                bail!("Listener inventory malformed")
            }
            let mut matches = vec![];
            for n in 0..count {
                let row = unsafe {
                    std::ptr::read_unaligned(
                        buffer
                            .as_ptr()
                            .cast::<u8>()
                            .add(4 + n * stride)
                            .cast::<MIB_TCPROW_OWNER_PID>(),
                    )
                };
                if row.dwState == 2
                    && row.dwLocalAddr.to_ne_bytes() == ip.octets()
                    && u16::from_be(row.dwLocalPort as u16) == address.port()
                {
                    matches.push(row.dwOwningPid)
                }
            }
            matches == [self.pid]
        };
        #[cfg(target_os = "linux")]
        let owned = {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open("/proc/net/tcp")?
                .take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 8 * 1024 * 1024 {
                bail!("Listener inventory bound")
            }
            let table = std::str::from_utf8(&bytes)?;
            let local = format!(
                "{:08X}:{:04X}",
                u32::from_le_bytes(ip.octets()),
                address.port()
            );
            let mut inodes = vec![];
            for line in table.lines().skip(1) {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.len() >= 10 && fields[1] == local && fields[3] == "0A" {
                    inodes.push(fields[9].to_string())
                }
            }
            if inodes.len() != 1 {
                bail!("Exact owned listener absent or ambiguous")
            }
            let target = format!("socket:[{}]", inodes[0]);
            let mut found = false;
            for (number, entry) in std::fs::read_dir(format!("/proc/{}/fd", self.pid))?.enumerate()
            {
                if number > 16384 {
                    bail!("Original process descriptor inventory bound")
                }
                let path = entry?.path();
                if std::fs::read_link(path)
                    .is_ok_and(|v| v.as_os_str() == std::ffi::OsStr::new(&target))
                {
                    found = true;
                    break;
                }
            }
            found
        };
        self.alive()?;
        if !owned {
            bail!("Listener belongs to a different original process")
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Read, Write};
    struct Original(std::process::Child);
    impl Drop for Original {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    #[test]
    fn listener_owner_refuses_foreign_live_process_and_exited_handle() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "owner::tests::socket_child",
                "--nocapture",
            ])
            .env("COWORK_GATEWAY_SOCKET_PROBE", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut original = Original(command.spawn().unwrap());
        let child_owner = BackendOwner::capture(original.0.id()).unwrap();
        let mut input = std::io::BufReader::new(original.0.stdout.take().unwrap());
        let address = loop {
            let mut line = String::new();
            assert!(input.read_line(&mut line).unwrap() > 0);
            if let Some(value) = line.strip_prefix("owned_socket=") {
                break value.trim().parse().unwrap();
            }
        };
        child_owner.assert_listener(address).unwrap();
        let parent_owner = BackendOwner::capture(std::process::id()).unwrap();
        assert!(parent_owner.assert_listener(address).is_err());
        original.0.stdin.take().unwrap().write_all(b"x").unwrap();
        assert!(original.0.wait().unwrap().success());
        assert!(child_owner.assert_listener(address).is_err());
    }
    #[test]
    #[ignore = "Owned helper process selected only by the narrow listener probe"]
    fn socket_child() {
        if std::env::var_os("COWORK_GATEWAY_SOCKET_PROBE").is_none() {
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        println!("owned_socket={}", listener.local_addr().unwrap());
        std::io::stdout().flush().unwrap();
        let mut byte = [0u8; 1];
        std::io::stdin().read_exact(&mut byte).unwrap();
    }
}
