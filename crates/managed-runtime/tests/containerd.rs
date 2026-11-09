#![cfg(all(target_os = "linux", feature = "containerd"))]
//! Real private daemon and official gRPC binding. No container/task mutation or
//! workload qualification; only our newly spawned children are stopped.
use cowork_managed_runtime::containerd::{
    ContainerdConnection, DaemonExecutable, ExpectedPeer, PrivateDaemon,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

struct Daemon {
    peer: ExpectedPeer,
    pid: u32,
    socket: PathBuf,
    cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<cowork_executor::Result<cowork_executor::Outcome>>>,
    dir: tempfile::TempDir,
}
impl Daemon {
    fn start(binary: &Path, expected: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("cowork-containerd-rpc-")
            .tempdir()
            .unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let executable = dir.path().join("containerd");
        std::fs::copy(binary, &executable).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(std::fs::read(&executable).unwrap())),
            expected
        );
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o500)).unwrap();
        let socket = dir.path().join("c.sock");
        let config = dir.path().join("containerd.toml");
        // containerd also creates /run/containerd even with a private --state.
        // The privileged fixture hides /run in its own mount namespace. Every
        // configurable path stays here; no host config imports or opt plugins.
        assert_eq!(unsafe { libc::geteuid() }, 0);
        assert_eq!(
            std::env::var("COWORK_TEST_PRIVATE_CONTAINERD_NS").as_deref(),
            Ok("1")
        );
        std::fs::write(&config,format!("version = 3\ntemp = {}\nimports = [{}]\ndisabled_plugins = [\"io.containerd.cri.v1.images\", \"io.containerd.cri.v1.runtime\", \"io.containerd.grpc.v1.cri\", \"io.containerd.nri.v1.nri\", \"io.containerd.internal.v1.opt\", \"io.containerd.image-verifier.v1.bindir\"]\n[ttrpc]\naddress = {}\n[metrics]\naddress = \"\"\n[debug]\nlevel = \"warn\"\n",
            serde_json::to_string(&dir.path().join("tmp")).unwrap(),
            serde_json::to_string(&dir.path().join("no-imports/*.toml")).unwrap(),
            serde_json::to_string(&dir.path().join("t.sock")).unwrap())).unwrap();
        let launch = cowork_executor::TrustedLaunch {
            program: executable,
            directory: dir.path().into(),
            arguments: vec![
                "--config".into(),
                config.to_string_lossy().into_owned(),
                "--address".into(),
                socket.to_string_lossy().into_owned(),
                "--root".into(),
                dir.path().join("root").to_string_lossy().into_owned(),
                "--state".into(),
                dir.path().join("state").to_string_lossy().into_owned(),
            ],
            environment: BTreeMap::from([("PATH".into(), "/usr/sbin:/usr/bin:/sbin:/bin".into())]),
            timeout: Duration::from_secs(60),
            capture_bytes_per_stream: 65536,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let expected = expected.to_owned();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            cowork_executor::supervise_started(&launch, &stop, |pid| {
                let peer =
                    ExpectedPeer::capture_new_child(pid, unsafe { libc::geteuid() }, &expected)
                        .map_err(|_| {
                            cowork_executor::Error::Invalid("private child binding failed")
                        })?;
                tx.send((peer, pid)).map_err(|_| {
                    cowork_executor::Error::Invalid("fixture startup receiver closed")
                })?;
                Ok(())
            })
        });
        let (peer, pid) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        Self {
            peer,
            pid,
            socket,
            cancel,
            thread: Some(thread),
            dir,
        }
    }
    async fn ready(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if self
                    .thread
                    .as_ref()
                    .is_some_and(|thread| thread.is_finished())
                {
                    panic!("own private daemon exited before its socket became ready");
                }
                if self.socket.exists() {
                    std::fs::set_permissions(&self.socket, std::fs::Permissions::from_mode(0o600))
                        .unwrap();
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
    fn stop(mut self) {
        self.cancel.store(true, Ordering::Release);
        let outcome = self.thread.take().unwrap().join().unwrap().unwrap();
        assert_eq!(outcome.termination, cowork_executor::Termination::Canceled);
        assert!(outcome.output_complete);
        assert!(!Path::new(&format!("/proc/{}", self.pid)).exists());
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && let Ok(Ok(outcome)) = thread.join()
            && outcome.termination != cowork_executor::Termination::Canceled
        {
            eprintln!(
                "own fixture daemon exit={:?}, stderr={}, stdout={}",
                outcome.exit_code,
                String::from_utf8_lossy(&outcome.stderr.bytes),
                String::from_utf8_lossy(&outcome.stdout.bytes)
            );
        }
        // No test method can create a task; daemon exit is sufficient to retire
        // this fixture. This is not a workload physical-termination witness.
        let _ = &self.dir;
    }
}

#[tokio::test]
#[ignore = "Requires explicit upstream binary/hash; starts only two fresh private fixture daemons"]
async fn real_private_containerd_version_and_endpoint_replacement_guards() {
    let binary = PathBuf::from(
        std::env::var("COWORK_TEST_CONTAINERD_BINARY").expect("explicit fixture binary"),
    );
    let expected = std::env::var("COWORK_TEST_CONTAINERD_SHA256").expect("exact fixture hash");
    assert!(binary.is_absolute());
    let a = Daemon::start(&binary, &expected);
    a.ready().await;
    let b = Daemon::start(&binary, &expected);
    b.ready().await;
    let binding = "1".repeat(64);
    let connection = ContainerdConnection::connect(&a.socket, a.peer.clone(), &binding)
        .await
        .unwrap();
    let version = connection.version().await.unwrap();
    assert_eq!(version.version, "v2.2.1");
    assert_eq!(connection.namespace(), format!("cowork-{}", "1".repeat(32)));
    assert!(
        ContainerdConnection::connect(&b.socket, a.peer.clone(), &binding)
            .await
            .is_err(),
        "Same executable/UID on another live PID must not be adopted"
    );
    std::fs::set_permissions(&a.socket, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(connection.version().await.is_err());
    std::fs::set_permissions(&a.socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let retained = a.socket.with_extension("original");
    std::fs::rename(&a.socket, &retained).unwrap();
    let substitute = tokio::net::UnixListener::bind(&a.socket).unwrap();
    std::fs::set_permissions(&a.socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        connection.version().await.is_err(),
        "An existing HTTP2 channel must not hide socket replacement"
    );
    drop(substitute);
    std::fs::remove_file(&a.socket).unwrap();
    std::fs::rename(&retained, &a.socket).unwrap();
    assert_eq!(connection.version().await.unwrap().version, "v2.2.1");
    let owned_elf = a.dir.path().join("containerd");
    std::fs::set_permissions(&owned_elf, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        connection.version().await.is_err(),
        "A cached hash cannot hide newly writable executable bytes"
    );
    assert!(ExpectedPeer::capture_new_child(a.pid, 0, &expected).is_err());
    std::fs::set_permissions(&owned_elf, std::fs::Permissions::from_mode(0o500)).unwrap();
    assert!(
        connection.version().await.is_err(),
        "Restoring mode cannot erase the observed native ctime change"
    );
    let alias = a.dir.path().join("unexpected-hardlink");
    std::fs::hard_link(&owned_elf, &alias).unwrap();
    assert!(
        ExpectedPeer::capture_new_child(a.pid, 0, &expected).is_err(),
        "A second link invalidates immutable executable qualification"
    );
    std::fs::remove_file(alias).unwrap();
    drop(connection);
    a.stop();
    b.stop();
    println!(
        "actual_upstream_containerd_version_verified=true; wrong_peer_rejected=true; socket_replacement_rejected=true; permissions_rechecked=true; immutable_peer_elf_changes_rejected=true; own_daemons_reaped=true; no_workload_tasks_launched=true"
    );
}

#[tokio::test]
#[ignore = "Requires the explicit root/private namespace fixture and exact upstream binary/hash"]
async fn real_owned_daemon_supervision_keeps_parent_mounts_and_restart_fence() {
    let binary = PathBuf::from(std::env::var("COWORK_TEST_CONTAINERD_BINARY").unwrap());
    let expected = std::env::var("COWORK_TEST_CONTAINERD_SHA256").unwrap();
    assert_eq!(
        std::env::var("COWORK_TEST_PRIVATE_CONTAINERD_NS").as_deref(),
        Ok("1")
    );
    let run_before = std::fs::read("/proc/thread-self/mountinfo").unwrap();
    let dir = tempfile::Builder::new()
        .prefix("cowork-owned-containerd-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let daemon = PrivateDaemon::start(
        DaemonExecutable::open(&binary, &expected).unwrap(),
        dir.path(),
        &"a".repeat(64),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert_eq!(
        daemon.connection().version().await.unwrap().version,
        "v2.2.1"
    );
    assert_eq!(
        std::fs::read("/proc/thread-self/mountinfo").unwrap(),
        run_before,
        "The managed supervisor must isolate only its own new OS thread/child"
    );
    let fence = std::fs::read(dir.path().join("daemon-start.fence")).unwrap();
    let pid = daemon.process_id();
    let exit = daemon.stop().await.unwrap();
    assert!(exit.daemon_process_reaped);
    assert_eq!(
        exit.outcome.termination,
        cowork_executor::Termination::Canceled
    );
    assert!(exit.outcome.output_complete);
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert!(
        PrivateDaemon::start(
            DaemonExecutable::open(&binary, &expected).unwrap(),
            dir.path(),
            &"a".repeat(64),
            Duration::from_secs(60)
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read(dir.path().join("daemon-start.fence")).unwrap(),
        fence
    );
    let dropped_dir = tempfile::Builder::new()
        .prefix("cowork-dropped-containerd-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(dropped_dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let dropped = PrivateDaemon::start(
        DaemonExecutable::open(&binary, &expected).unwrap(),
        dropped_dir.path(),
        &"b".repeat(64),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    let dropped_pid = dropped.process_id();
    drop(dropped);
    assert!(!Path::new(&format!("/proc/{dropped_pid}")).exists());
    assert!(dropped_dir.path().join("daemon-start.fence").is_file());
    println!(
        "owned_daemon_supervisor_verified=true; private_control_namespace_verified=true; parent_mounts_unchanged=true; stopped_generation_not_adopted=true; dropped_owner_reaped=true; no_task_stop_witness_claimed=true"
    );
}

#[tokio::test]
#[ignore = "Requires the exact upstream ELF and the explicit private root fixture"]
async fn real_daemon_uses_verified_open_elf_after_source_path_replacement() {
    let source = PathBuf::from(std::env::var("COWORK_TEST_CONTAINERD_BINARY").unwrap());
    let expected = std::env::var("COWORK_TEST_CONTAINERD_SHA256").unwrap();
    let directory = tempfile::Builder::new()
        .prefix("cowork-pinned-daemon-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("containerd");
    std::fs::copy(source, &path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        DaemonExecutable::open(&path, &expected).is_err(),
        "Writable executable must be rejected"
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).unwrap();
    assert!(DaemonExecutable::open(&path, &"0".repeat(64)).is_err());
    let executable = DaemonExecutable::open(&path, &expected).unwrap();
    std::fs::rename(&path, directory.path().join("original-elf")).unwrap();
    std::fs::write(&path, b"#!/bin/sh\nexit 97\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).unwrap();
    assert!(DaemonExecutable::open(&path, &expected).is_err());
    let state = directory.path().join("control");
    std::fs::create_dir(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let daemon = PrivateDaemon::start(executable, &state, &"c".repeat(64), Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(
        daemon.connection().version().await.unwrap().version,
        "v2.2.1"
    );
    assert!(daemon.stop().await.unwrap().daemon_process_reaped);
    println!(
        "pinned_elf_after_path_replacement_verified=true; wrong_hash_rejected=true; writable_elf_rejected=true; script_not_executed=true"
    );
}
