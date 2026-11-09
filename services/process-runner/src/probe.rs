//! Compiled trusted qualification probe; never accepts model commands.
use std::{
    io::Write,
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    time::Duration,
};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--supervise") {
        let result = (|| -> anyhow::Result<()> {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Command {
                program: PathBuf,
                arguments: Vec<String>,
                environment: std::collections::BTreeMap<String, String>,
                timeout_seconds: u64,
            }
            anyhow::ensure!(
                args.len() == 3 && args[2].len() <= 128 * 1024,
                "Invalid supervised command envelope"
            );
            let command: Command = serde_json::from_str(&args[2])?;
            let launch = cowork_executor::TrustedLaunch {
                program: command.program,
                arguments: command.arguments,
                environment: command.environment,
                directory: PathBuf::from("/workspace"),
                timeout: Duration::from_secs(command.timeout_seconds),
                capture_bytes_per_stream: 65536,
            };
            let result =
                cowork_executor::supervise(&launch, &std::sync::atomic::AtomicBool::new(false))?;
            println!("{}", serde_json::to_string(&result)?);
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("supervisor failed: {error}");
            std::process::exit(3);
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--boundary") {
        match boundary(&args[2..]) {
            Ok(value) => println!("{value}"),
            Err(error) => {
                eprintln!("boundary unavailable: {error}");
                std::process::exit(3);
            }
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--manifest") {
        match manifest(&args[2..]) {
            Ok(value) => println!("{value}"),
            Err(error) => {
                eprintln!("manifest unavailable: {error}");
                std::process::exit(3);
            }
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--reset")
        || args.get(1).map(String::as_str) == Some("--directories")
    {
        if let Err(error) = prepare(&args[1..]) {
            eprintln!("workspace prepare failed: {error}");
            std::process::exit(3);
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--proxy-probe") {
        match proxy_probe(&args[2..]) {
            Ok(report) => println!("{report}"),
            Err(error) => {
                eprintln!("network probe failed: {error}");
                std::process::exit(3);
            }
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--egress-denied") {
        let address = args.get(2).and_then(|s| s.parse::<SocketAddr>().ok());
        let denied = address.is_some_and(|a| {
            !a.ip().is_loopback() && TcpStream::connect_timeout(&a, Duration::from_secs(3)).is_err()
        });
        if !denied {
            std::process::exit(3);
        }
        println!("{}", serde_json::json!({"direct_denied":true}));
        return;
    }
    if args.get(1).map(String::as_str) == Some("--start-preview") {
        let result = (|| -> anyhow::Result<()> {
            anyhow::ensure!(
                args.get(2).map(String::as_str) == Some("8081") && args.len() == 3,
                "Only approved static preview port supported"
            );
            let mut child = std::process::Command::new(std::env::current_exe()?)
                .args(["--serve-workspace", "8081"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            for _ in 0..40 {
                if child.try_wait()?.is_some() {
                    anyhow::bail!("Preview child exited before readiness");
                }
                if TcpStream::connect_timeout(
                    &"127.0.0.1:8081".parse()?,
                    Duration::from_millis(100),
                )
                .is_ok()
                {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            anyhow::bail!("Preview readiness unconfirmed")
        })();
        if let Err(error) = result {
            eprintln!("preview start failed: {error}");
            std::process::exit(3);
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--serve-workspace") {
        if args.get(2).map(String::as_str) != Some("8081") || args.len() != 3 {
            std::process::exit(3);
        }
        if let Err(error) = serve_workspace() {
            eprintln!("preview server failed: {error}");
            std::process::exit(3);
        }
        return;
    }
    if args.len() == 2 && args[1] == "--version" {
        println!(
            "{}",
            serde_json::json!({"application_version":env!("CARGO_PKG_VERSION"),"runner_protocol_version":1,"binary":"cowork-runner-probe","os":std::env::consts::OS,"arch":std::env::consts::ARCH})
        );
        return;
    }
    if args.get(1).map(String::as_str) == Some("--child") {
        std::thread::sleep(Duration::from_secs(120));
        return;
    }
    if args.len() != 4 && args.len() != 7 {
        std::process::exit(2);
    }
    let host = PathBuf::from(&args[1]);
    let network: SocketAddr = args[2].parse().unwrap();
    let workspace = PathBuf::from(&args[3]);
    let resource_limits_enforced = if args.len() == 7 {
        let limits = vec![
            args[4].clone(),
            args[5].clone(),
            args[6].clone(),
            u64::MAX.to_string(),
        ];
        Some(boundary(&limits).is_ok())
    } else {
        None
    };
    let read_denied = std::fs::read(&host).is_err();
    let write_denied = std::fs::OpenOptions::new().write(true).open(&host).is_err();
    let network_denied = TcpStream::connect_timeout(&network, Duration::from_millis(500)).is_err();
    let credentials_isolated = std::env::var_os("COWORK_RUNNER_SECRET_PROBE").is_none();
    let writable = std::fs::File::create(workspace.join("qualification-output.txt"))
        .and_then(|mut f| f.write_all(b"isolated output\n"))
        .is_ok();
    let child = std::env::current_exe().ok().and_then(|p| {
        std::process::Command::new(p)
            .arg("--child")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()
    });
    let child_started = child.is_some();
    // The parent exits while its own child remains alive. Qualification accepts
    // only after the adapter proves the entire owned Job/container quiescent.
    println!(
        "{}",
        serde_json::json!({"read_denied":read_denied,"write_denied":write_denied,"network_denied":network_denied,"credentials_isolated":credentials_isolated,"workspace_writable":writable,"child_started":child_started,"resource_limits_enforced":resource_limits_enforced})
    );
    if !(read_denied
        && write_denied
        && network_denied
        && credentials_isolated
        && writable
        && child_started)
        || resource_limits_enforced == Some(false)
    {
        std::process::exit(3);
    }
}
fn serve_workspace() -> anyhow::Result<()> {
    let listener = std::net::TcpListener::bind("0.0.0.0:8081")?;
    serve_workspace_at(listener, PathBuf::from("/workspace"), None)
}
fn serve_workspace_at(
    listener: std::net::TcpListener,
    root: PathBuf,
    requests: Option<usize>,
) -> anyhow::Result<()> {
    use std::io::Read;
    let root = root.canonicalize()?;
    for stream in listener.incoming().take(requests.unwrap_or(usize::MAX)) {
        let mut stream = stream?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let mut bytes = Vec::with_capacity(8192);
        let mut chunk = [0u8; 1024];
        while bytes.len() < 8192 && !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
            let remaining = (8192 - bytes.len()).min(chunk.len());
            match stream.read(&mut chunk[..remaining]) {
                Ok(0) | Err(_) => break,
                Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            }
        }
        let response = (|| -> anyhow::Result<(Vec<u8>, &'static str)> {
            anyhow::ensure!(
                bytes.ends_with(b"\r\n\r\n"),
                "Complete bounded HTTP headers required"
            );
            let line = std::str::from_utf8(&bytes)?
                .lines()
                .next()
                .ok_or_else(|| anyhow::anyhow!("Empty HTTP request"))?;
            let mut words = line.split_whitespace();
            anyhow::ensure!(words.next() == Some("GET"), "Only static GET supported");
            let path = words
                .next()
                .ok_or_else(|| anyhow::anyhow!("Missing path"))?
                .split('?')
                .next()
                .unwrap_or("");
            let decoded = percent_encoding::percent_decode_str(path).decode_utf8()?;
            let relative = decoded
                .strip_prefix('/')
                .ok_or_else(|| anyhow::anyhow!("Absolute HTTP path required"))?;
            let relative = if relative.is_empty() {
                "index.html"
            } else {
                relative
            };
            anyhow::ensure!(
                cowork_agent_sandbox::runtime::path_valid(relative),
                "Traversal denied"
            );
            let mut current = root.clone();
            for part in relative.split('/') {
                current.push(part);
                anyhow::ensure!(
                    !current.symlink_metadata()?.file_type().is_symlink(),
                    "Symlinks denied"
                );
            }
            let canonical = current.canonicalize()?;
            anyhow::ensure!(
                canonical.starts_with(&root)
                    && canonical.is_file()
                    && canonical.metadata()?.len() <= 64 * 1024 * 1024,
                "Unsupported static file"
            );
            let content = std::fs::read(canonical)?;
            let kind = match std::path::Path::new(relative)
                .extension()
                .and_then(|s| s.to_str())
            {
                Some("html" | "htm") => "text/html; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("js" | "mjs") => "text/javascript; charset=utf-8",
                Some("json") => "application/json",
                Some("png") => "image/png",
                Some("svg") => "image/svg+xml",
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("wasm") => "application/wasm",
                _ => "application/octet-stream",
            };
            Ok((content, kind))
        })();
        let (status, body, kind) = match response {
            Ok((body, kind)) => ("200 OK", body, kind),
            Err(_) => (
                "404 Not Found",
                b"Preview file unavailable".to_vec(),
                "text/plain",
            ),
        };
        let _=write!(stream,"HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",body.len()).and_then(|_|stream.write_all(&body));
    }
    Ok(())
}
fn prepare(args: &[String]) -> anyhow::Result<()> {
    use anyhow::ensure;
    let root = std::path::Path::new("/workspace");
    if args.first().map(String::as_str) == Some("--reset") {
        ensure!(
            args == ["--reset", "/workspace"],
            "Only sandbox workspace reset permitted"
        );
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            let m = entry.path().symlink_metadata()?;
            if m.is_dir() && !m.file_type().is_symlink() {
                std::fs::remove_dir_all(entry.path())?;
            } else {
                std::fs::remove_file(entry.path())?;
            }
        }
    } else {
        for path in &args[1..] {
            ensure!(
                cowork_agent_sandbox::runtime::path_valid(path) && !path.is_empty(),
                "Invalid directory path"
            );
            std::fs::create_dir_all(root.join(path))?;
        }
    }
    Ok(())
}
fn proxy_probe(args: &[String]) -> anyhow::Result<serde_json::Value> {
    use anyhow::{Context, ensure};
    use std::io::Read;
    ensure!(args.len() == 3, "Expected proxy/target/direct endpoint");
    let direct: SocketAddr = args[2].parse()?;
    let direct_denied = TcpStream::connect_timeout(&direct, Duration::from_secs(2)).is_err();
    // The operator's probe uses HTTP through the forward proxy, with an absolute
    // request target. No library proxy environment or ambient credentials involved.
    let proxy = reqwest::Url::parse(&args[0])?;
    let target = reqwest::Url::parse(&args[1])?;
    ensure!(
        proxy.scheme() == "http" && target.scheme() == "http",
        "HTTP probe origins required"
    );
    let host = proxy.host_str().context("Proxy host absent")?;
    use std::net::ToSocketAddrs;
    let address = (
        host,
        proxy.port_or_known_default().context("Proxy port absent")?,
    )
        .to_socket_addrs()?
        .next()
        .context("Proxy DNS absent")?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    write!(
        stream,
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        target.as_str(),
        target.host_str().context("Target host absent")?
    )?;
    let mut response = [0u8; 1024];
    let n = stream.read(&mut response)?;
    let line = std::str::from_utf8(&response[..n])?
        .lines()
        .next()
        .context("Proxy response absent")?;
    let status: u16 = line
        .split_whitespace()
        .nth(1)
        .context("Proxy status absent")?
        .parse()?;
    let proxy_reachable = (200..400).contains(&status);
    ensure!(
        direct_denied && proxy_reachable,
        "Proxy/direct egress policy failed"
    );
    Ok(
        serde_json::json!({"direct_denied":direct_denied,"proxy_reachable":proxy_reachable,"executable_sha256":own_digest()?}),
    )
}

fn own_digest() -> anyhow::Result<String> {
    let bytes = std::fs::read(std::env::current_exe()?)?;
    Ok(hex_digest(&bytes))
}
fn hex_digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[cfg(target_os = "linux")]
fn boundary(args: &[String]) -> anyhow::Result<serde_json::Value> {
    use anyhow::{Context, ensure};
    ensure!(
        args.len() == 4,
        "Expected memory/cpu/process/storage limits"
    );
    let expected: Vec<u64> = args
        .iter()
        .map(|a| a.parse::<u64>())
        .collect::<Result<_, _>>()?;
    let number = |name: &str| -> anyhow::Result<u64> {
        Ok(std::fs::read_to_string(format!("/sys/fs/cgroup/{name}"))?
            .trim()
            .parse()?)
    };
    let memory = number("memory.max")?;
    let pids = number("pids.max")?;
    let cpu = std::fs::read_to_string("/sys/fs/cgroup/cpu.max")?;
    let mut parts = cpu.split_whitespace();
    let quota: u64 = parts.next().context("CPU quota missing")?.parse()?;
    let period: u64 = parts.next().context("CPU period missing")?.parse()?;
    ensure!(
        period > 0 && memory > 0 && pids > 0,
        "Invalid cgroup limits"
    );
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let path = c"/workspace";
    ensure!(
        unsafe { libc::statvfs(path.as_ptr(), &mut stat) } == 0,
        "Workspace statvfs failed"
    );
    let capacity = u128::from(stat.f_blocks)
        .checked_mul(u128::from(stat.f_frsize))
        .and_then(|bytes| u64::try_from(bytes).ok())
        .context("Capacity overflow")?;
    ensure!(
        memory <= expected[0]
            && u128::from(quota) * 1000 <= u128::from(expected[1]) * u128::from(period)
            && pids <= expected[2]
            && capacity > 0
            && capacity <= expected[3],
        "Actual hard resource limits exceed definition"
    );
    ensure!(
        std::fs::metadata("/sys/fs/cgroup")?.is_dir(),
        "Unified cgroup unavailable"
    );
    Ok(
        serde_json::json!({"executable_sha256":own_digest()?,"memory_max":memory,"pids_max":pids,"cpu_quota":quota,"cpu_period":period,"workspace_capacity":capacity,"limits_enforced":true}),
    )
}
#[cfg(not(target_os = "linux"))]
fn boundary(_: &[String]) -> anyhow::Result<serde_json::Value> {
    anyhow::bail!("Linux cgroup v2 required")
}

fn manifest(args: &[String]) -> anyhow::Result<serde_json::Value> {
    use anyhow::{Context, ensure};
    use std::collections::BTreeMap;
    ensure!(
        args == ["/workspace"],
        "Only the sandbox workspace may be exported"
    );
    let root = std::path::Path::new("/workspace");
    let mut entries = BTreeMap::new();
    let mut pending = vec![String::new()];
    let mut total = 0u64;
    while let Some(parent) = pending.pop() {
        for entry in std::fs::read_dir(root.join(&parent))? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Non-UTF8 name"))?;
            ensure!(
                !name.is_empty()
                    && !matches!(name.as_str(), "." | "..")
                    && !name.contains(['\\', ':', '\0', '/']),
                "Unsupported name"
            );
            let path = if parent.is_empty() {
                name
            } else {
                format!("{parent}/{name}")
            };
            ensure!(
                path.len() <= 1024 && entries.len() < 10000,
                "Manifest limit exceeded"
            );
            let m = entry.path().symlink_metadata()?;
            ensure!(!m.file_type().is_symlink(), "Workspace symlink denied");
            if m.is_dir() {
                entries.insert(
                    path.clone(),
                    serde_json::json!({"directory":true,"bytes":0,"sha256":hex_digest(b"")}),
                );
                pending.push(path);
                continue;
            }
            ensure!(
                m.is_file() && m.len() <= 64 * 1024 * 1024,
                "Unsupported file"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                ensure!(m.nlink() == 1, "Hardlinked file denied");
            }
            total = total
                .checked_add(m.len())
                .context("Manifest length overflow")?;
            ensure!(total <= 512 * 1024 * 1024, "Workspace exceeds export limit");
            let bytes = std::fs::read(entry.path())?;
            ensure!(bytes.len() as u64 == m.len(), "File changed during export");
            entries.insert(
                path,
                serde_json::json!({"directory":false,"bytes":m.len(),"sha256":hex_digest(&bytes)}),
            );
        }
    }
    Ok(serde_json::json!({"executable_sha256":own_digest()?,"entries":entries}))
}

#[cfg(test)]
mod preview_tests {
    use super::*;
    use std::io::Read;
    #[test]
    fn static_preview_serves_real_workspace_and_denies_encoded_traversal() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.html"), "<h1>workspace preview</h1>").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let path = root.path().to_path_buf();
        let server =
            std::thread::spawn(move || serve_workspace_at(listener, path, Some(2)).unwrap());
        for (url, expected) in [("/", "200 OK"), ("/%2e%2e/private", "404 Not Found")] {
            let mut client = TcpStream::connect(address).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            write!(
                client,
                "GET {url} HTTP/1.1\r\nHost: preview.invalid\r\n\r\n"
            )
            .unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            assert!(response.contains(expected));
            if url == "/" {
                assert!(response.ends_with("<h1>workspace preview</h1>"));
                assert!(response.contains("X-Content-Type-Options: nosniff"));
            }
        }
        server.join().unwrap();
    }
}
