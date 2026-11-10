//! Reviewed owner host PTYs. Deliberately separate from actor sandbox terminals.
//! Input/output are memory-only; a durable launch receipt prevents lost-ACK duplication.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path as FsPath, PathBuf},
    sync::Mutex,
};

const OUTPUT_LIMIT: usize = 1024 * 1024;
#[derive(Serialize, Deserialize)]
struct Receipt {
    key: String,
    digest: String,
    session: OwnerTerminalSession,
}
struct Session {
    record: OwnerTerminalSession,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    output: Arc<Mutex<OutputBuffer>>,
    // Only the last ACK is retained. Earlier sequence replays are never written twice.
    last_input: Option<(u64, Vec<u8>, bool)>,
    _lease: Option<Arc<cowork_toolbox::WorkspaceLease>>,
}
#[derive(Default)]
struct OutputBuffer {
    bytes: VecDeque<u8>,
    first: u64,
    next: u64,
}
pub struct TerminalService {
    toolbox: Arc<cowork_toolbox::Toolbox>,
    root: PathBuf,
    sessions: Mutex<BTreeMap<String, Arc<Mutex<Session>>>>,
    gate: Mutex<()>,
}
fn invalid(message: &str) -> Error {
    Error::Validation(message.into())
}
fn lock<T>(value: &Mutex<T>) -> cowork_domain::Result<std::sync::MutexGuard<'_, T>> {
    value.lock().map_err(|_| Error::Conflict)
}
fn size(rows: u16, cols: u16) -> cowork_domain::Result<PtySize> {
    if !(2..=200).contains(&rows) || !(10..=400).contains(&cols) {
        return Err(invalid("Terminal size is outside supported bounds"));
    }
    Ok(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })
}
fn private_path(path: &FsPath) -> cowork_domain::Result<()> {
    for parent in path.ancestors() {
        let Ok(meta) = fs::symlink_metadata(parent) else {
            continue;
        };
        if meta.is_symlink() {
            return Err(Error::Forbidden);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(Error::Forbidden);
            }
        }
    }
    Ok(())
}
fn io_error(_: impl std::fmt::Display) -> Error {
    invalid("Terminal resource is unavailable")
}
impl TerminalService {
    pub fn active_count(&self) -> usize {
        let Ok(sessions) = self.sessions.lock() else {
            return usize::MAX;
        };
        let mut count = 0;
        for session in sessions.values() {
            let Ok(mut s) = session.lock() else {
                return usize::MAX;
            };
            poll(&mut s);
            if s.child.is_some() {
                count += 1;
            }
        }
        count
    }
    pub fn open(toolbox: Arc<cowork_toolbox::Toolbox>) -> cowork_domain::Result<Arc<Self>> {
        let root = toolbox
            .setup_data_dir()
            .map_err(io_error)?
            .join("owner-terminals");
        private_path(&root)?;
        fs::create_dir_all(&root).map_err(io_error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
        }
        Ok(Arc::new(Self {
            toolbox,
            root,
            sessions: Mutex::new(BTreeMap::new()),
            gate: Mutex::new(()),
        }))
    }
    fn preview_inner(
        &self,
        order: &str,
        input: &OwnerTerminalLaunch,
    ) -> cowork_domain::Result<(OwnerTerminalPreview, PathBuf, BTreeMap<String, String>)> {
        if input.idempotency_key.is_empty() || input.idempotency_key.len() > 128 {
            return Err(invalid("Invalid terminal request key"));
        }
        size(input.rows, input.cols)?;
        // Shell arguments are fixed here. User commands enter only through the PTY.
        if !["powershell", "bash", "sh"].contains(&input.program.as_str()) {
            return Err(invalid("Select a configured terminal shell"));
        }
        let (program, environment) = self
            .toolbox
            .terminal_launch(&input.program)
            .map_err(io_error)?;
        let cwd = self.toolbox.workspace(order).map_err(io_error)?;
        let executable = program.to_string_lossy().into_owned();
        let cwd = cwd.to_string_lossy().into_owned();
        let digest = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    order,
                    &input.idempotency_key,
                    &input.program,
                    input.rows,
                    input.cols,
                    &executable,
                    &cwd
                ))
                .map_err(io_error)?
            )
        );
        Ok((OwnerTerminalPreview{approval_sha256:digest,executable,cwd,boundary:"owner_host: commands run with the daemon user's host permissions; the working directory is not a filesystem sandbox".into()},program,environment))
    }
    pub fn preview(
        &self,
        order: &str,
        input: &OwnerTerminalLaunch,
    ) -> cowork_domain::Result<OwnerTerminalPreview> {
        Ok(self.preview_inner(order, input)?.0)
    }
    fn receipt_path(&self, order: &str, key: &str) -> PathBuf {
        self.root.join(format!(
            "{:x}.json",
            Sha256::digest(format!("{order}\0{key}").as_bytes())
        ))
    }
    fn read_receipt(&self, path: &FsPath) -> cowork_domain::Result<Option<Receipt>> {
        private_path(path)?;
        let meta = match fs::metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_error(e)),
        };
        if !meta.is_file() || meta.len() > 16384 {
            return Err(Error::Forbidden);
        }
        Ok(Some(
            serde_json::from_slice(&fs::read(path).map_err(io_error)?).map_err(io_error)?,
        ))
    }
    pub fn launch(
        self: &Arc<Self>,
        order: &str,
        input: &OwnerTerminalLaunch,
    ) -> cowork_domain::Result<OwnerTerminalSession> {
        self.launch_checked(order, input, || Ok(()))
    }
    /// The live-chat check runs while the workspace lease is held, before any effect.
    pub fn launch_checked(
        self: &Arc<Self>,
        order: &str,
        input: &OwnerTerminalLaunch,
        check: impl FnOnce() -> cowork_domain::Result<()>,
    ) -> cowork_domain::Result<OwnerTerminalSession> {
        let _gate = lock(&self.gate)?;
        let path = self.receipt_path(order, &input.idempotency_key);
        // Resolve a durable original before consulting mutable executable configuration.
        if let Some(receipt) = self.read_receipt(&path)? {
            if receipt.key != input.idempotency_key
                || receipt.session.order_id != order
                || receipt.session.program != input.program
                || receipt.session.rows != input.rows
                || receipt.session.cols != input.cols
                || input.approval_sha256.as_deref() != Some(&receipt.digest)
            {
                return Err(Error::Conflict);
            }
            if let Some(session) = lock(&self.sessions)?.get(&receipt.session.id) {
                return Ok(lock(session)?.record.clone());
            }
            let mut record = receipt.session;
            record.state = "interrupted".into();
            record.pid = None;
            return Ok(record);
        }
        let (preview, program, environment) = self.preview_inner(order, input)?;
        if input.approval_sha256.as_deref() != Some(&preview.approval_sha256) {
            return Err(invalid(
                "Review and approve this exact terminal launch first",
            ));
        }
        let mut sessions = lock(&self.sessions)?;
        if sessions.len() >= 128 {
            sessions.retain(|_, session| session.lock().is_ok_and(|s| s.child.is_some()));
        }
        if sessions.len() >= 128
            || sessions
                .values()
                .filter(|v| v.lock().is_ok_and(|s| s.child.is_some()))
                .count()
                >= 16
        {
            return Err(invalid("Close unused terminals first"));
        }
        let lease = if let Some(existing) = sessions.values().find_map(|s| {
            s.lock()
                .ok()
                .filter(|s| s.record.order_id == order && s.record.state == "running")
                .and_then(|s| s._lease.clone())
        }) {
            existing
        } else {
            self.toolbox.workspace_lease(order).map_err(io_error)?
        };
        check()?;
        let record = OwnerTerminalSession {
            id: id(),
            order_id: order.into(),
            program: input.program.clone(),
            state: "starting".into(),
            pid: None,
            rows: input.rows,
            cols: input.cols,
            created_at: cowork_storage::now(),
            exit_code: None,
            input_sequence: 0,
        };
        // Persist intent before creating a physical process. A crash is never auto-replayed.
        let receipt = Receipt {
            key: input.idempotency_key.clone(),
            digest: preview.approval_sha256,
            session: record.clone(),
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io_error)?;
        serde_json::to_writer(&mut file, &receipt).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        let pair = portable_pty::native_pty_system()
            .openpty(size(input.rows, input.cols)?)
            .map_err(io_error)?;
        let mut command = CommandBuilder::new(program);
        command.cwd(&preview.cwd);
        command.env_clear();
        for (key, value) in environment {
            command.env(key, value);
        }
        for key in [
            "HOME",
            "USER",
            "LOGNAME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "PATHEXT",
            "ComSpec",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command.env("TERM", "xterm-256color");
        if input.program == "powershell" {
            command.args(["-NoLogo", "-NoProfile"]);
        } else {
            command.arg("-i");
        }
        let mut child = pair.slave.spawn_command(command).map_err(io_error)?;
        let reader = match pair.master.try_clone_reader() {
            Ok(v) => v,
            Err(e) => {
                let _ = child.kill();
                return Err(io_error(e));
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(v) => v,
            Err(e) => {
                let _ = child.kill();
                return Err(io_error(e));
            }
        };
        drop(pair.slave);
        let mut record = record;
        record.state = "running".into();
        record.pid = child.process_id();
        let session = Arc::new(Mutex::new(Session {
            record: record.clone(),
            master: Some(pair.master),
            writer: Some(Arc::new(Mutex::new(writer))),
            child: Some(child),
            output: Arc::new(Mutex::new(OutputBuffer::default())),
            last_input: None,
            _lease: Some(lease),
        }));
        sessions.insert(record.id.clone(), session.clone());
        drop(sessions);
        let output = lock(&session)?.output.clone();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut bytes = [0u8; 8192];
            loop {
                match reader.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut s) = output.lock() {
                            s.bytes.extend(&bytes[..n]);
                            s.next += n as u64;
                            while s.bytes.len() > OUTPUT_LIMIT {
                                s.bytes.pop_front();
                                s.first += 1;
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
        });
        Ok(record)
    }
    fn find(&self, order: &str, id: &str) -> cowork_domain::Result<Arc<Mutex<Session>>> {
        let sessions = lock(&self.sessions)?;
        let s = sessions.get(id).ok_or(Error::NotFound)?;
        if lock(s)?.record.order_id != order {
            return Err(Error::NotFound);
        }
        Ok(s.clone())
    }
    pub fn list(&self, order: &str) -> cowork_domain::Result<Vec<OwnerTerminalSession>> {
        let _gate = lock(&self.gate)?;
        let mut result = Vec::new();
        let sessions = lock(&self.sessions)?;
        for session in sessions.values() {
            let mut s = lock(session)?;
            if s.record.order_id != order {
                continue;
            }
            poll(&mut s);
            result.push(s.record.clone());
        }
        let current_ids = result
            .iter()
            .map(|v| v.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let active_ids = sessions
            .values()
            .filter_map(|s| {
                s.lock()
                    .ok()
                    .filter(|s| s.record.order_id == order && s.child.is_some())
                    .map(|s| s.record.id.clone())
            })
            .collect::<std::collections::BTreeSet<_>>();
        drop(sessions);
        for entry in fs::read_dir(&self.root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            if entry.path().extension().is_none_or(|v| v != "json") {
                continue;
            }
            if let Some(receipt) = self.read_receipt(&entry.path())?
                && receipt.session.order_id == order
                && !current_ids.contains(&receipt.session.id)
            {
                let mut record = receipt.session;
                record.state = "interrupted".into();
                record.pid = None;
                result.push(record);
                result.sort_by(|a, b| {
                    active_ids
                        .contains(&b.id)
                        .cmp(&active_ids.contains(&a.id))
                        .then_with(|| b.created_at.cmp(&a.created_at))
                        .then_with(|| a.id.cmp(&b.id))
                });
                result.truncate(128);
            }
        }
        result.sort_by_key(|v| v.created_at);
        Ok(result)
    }
    pub fn output(
        &self,
        order: &str,
        id: &str,
        after: u64,
    ) -> cowork_domain::Result<OwnerTerminalOutput> {
        let session = self.find(order, id)?;
        let mut s = lock(&session)?;
        poll(&mut s);
        let output = lock(&s.output)?;
        if after > output.next {
            return Err(Error::Conflict);
        }
        let start = after.max(output.first);
        let bytes: Vec<u8> = output
            .bytes
            .iter()
            .skip((start - output.first) as usize)
            .copied()
            .collect();
        Ok(OwnerTerminalOutput {
            session: s.record.clone(),
            first_sequence: start,
            next_sequence: output.next,
            bytes_base64: STANDARD.encode(bytes),
            truncated: after < output.first,
        })
    }
    pub fn input(
        &self,
        order: &str,
        id: &str,
        input: OwnerTerminalInput,
    ) -> cowork_domain::Result<OwnerTerminalSession> {
        if input.bytes_base64.len() > 24000 {
            return Err(invalid("Terminal input exceeds limit"));
        }
        let bytes = STANDARD.decode(input.bytes_base64).map_err(io_error)?;
        if bytes.is_empty() || bytes.len() > 16384 {
            return Err(invalid("Terminal input exceeds limit"));
        }
        let session = self.find(order, id)?;
        let mut s = lock(&session)?;
        poll(&mut s);
        if let Some((sequence, previous, confirmed)) = &s.last_input
            && input.sequence == *sequence
        {
            return if previous == &bytes {
                if *confirmed {
                    Ok(s.record.clone())
                } else {
                    Err(Error::UncertainEffect)
                }
            } else {
                Err(Error::Conflict)
            };
        }
        if s.record.state != "running"
            || input.sequence != s.record.input_sequence + 1
            || s.last_input.as_ref().is_some_and(|v| !v.2)
        {
            return Err(Error::Conflict);
        }
        // Latch before the write. Partial OS writes are uncertain and never resent automatically.
        s.record.input_sequence = input.sequence;
        s.last_input = Some((input.sequence, bytes.clone(), false));
        let writer = s.writer.as_ref().ok_or(Error::Conflict)?.clone();
        // Never hold the lifecycle lock during a potentially blocked PTY write.
        // Stop can close the PTY/process group and release a full input pipe.
        drop(s);
        let written = {
            let mut writer = lock(&writer)?;
            writer.write_all(&bytes).and_then(|_| writer.flush())
        };
        let mut s = lock(&session)?;
        if written.is_err() {
            if s.record.state == "running" {
                s.record.state = "input_uncertain".into();
            }
            return Err(Error::UncertainEffect);
        }
        if let Some(attempt) = s.last_input.as_mut() {
            attempt.2 = true;
        }
        Ok(s.record.clone())
    }
    pub fn resize(
        &self,
        order: &str,
        id: &str,
        input: OwnerTerminalResize,
    ) -> cowork_domain::Result<OwnerTerminalSession> {
        let session = self.find(order, id)?;
        let mut s = lock(&session)?;
        s.master
            .as_ref()
            .ok_or(Error::Conflict)?
            .resize(size(input.rows, input.cols)?)
            .map_err(io_error)?;
        s.record.rows = input.rows;
        s.record.cols = input.cols;
        Ok(s.record.clone())
    }
    pub fn stop(&self, order: &str, id: &str) -> cowork_domain::Result<OwnerTerminalSession> {
        let session = self.find(order, id)?;
        let mut s = lock(&session)?;
        poll(&mut s);
        if s.child.is_some() {
            #[cfg(target_os = "linux")]
            if let Some(pid) = s.child.as_ref().and_then(|child| child.process_id()) {
                kill_linux_session(pid)?;
            }
            #[cfg(all(unix, not(target_os = "linux")))]
            if let Some(group) = s.master.as_ref().and_then(|v| v.process_group_leader()) {
                if group > 1 {
                    unsafe {
                        libc::kill(-group, libc::SIGKILL);
                    }
                }
            }
            if let Some(child) = s.child.as_mut() {
                child.kill().map_err(io_error)?;
                child.wait().map_err(io_error)?;
            }
            s.child = None;
            s.writer = None;
            s.master = None;
            s.record.state = "stopped".into();
            s.record.pid = None;
            s._lease = None;
        }
        Ok(s.record.clone())
    }
    pub fn shutdown(&self) {
        if let Ok(sessions) = self.sessions.lock() {
            for session in sessions.values() {
                if let Ok(mut s) = session.lock() {
                    #[cfg(target_os = "linux")]
                    if let Some(pid) = s.child.as_ref().and_then(|child| child.process_id()) {
                        let _ = kill_linux_session(pid);
                    }
                    #[cfg(all(unix, not(target_os = "linux")))]
                    if let Some(group) = s.master.as_ref().and_then(|v| v.process_group_leader()) {
                        if group > 1 {
                            unsafe {
                                libc::kill(-group, libc::SIGKILL);
                            }
                        }
                    }
                    if let Some(child) = s.child.as_mut() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    s.child = None;
                    s.writer = None;
                    s.master = None;
                    s.record.pid = None;
                    s.record.state = "stopped".into();
                    s._lease = None;
                }
            }
        }
    }
}
impl Drop for TerminalService {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn poll(session: &mut Session) {
    if let Some(child) = session.child.as_mut() {
        // Keep the owned leader unreaped while checking/killing its session.
        // This pins the SID and prevents adopting an unrelated reused OS ID.
        #[cfg(target_os = "linux")]
        if let Some(pid) = child.process_id()
            && linux_exit_pending(pid)
            && kill_linux_session(pid).is_err()
        {
            session.record.state = "cleanup_unconfirmed".into();
            return;
        }
        if let Ok(Some(exit)) = child.try_wait() {
            session.record.state = "exited".into();
            session.record.exit_code = Some(exit.exit_code());
            session.record.pid = None;
            session.child = None;
            session.writer = None;
            session.master = None;
            session._lease = None;
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_exit_pending(pid: u32) -> bool {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        ) == 0
            && info.assume_init().si_pid() == pid as libc::pid_t
    }
}

/// portable-pty's process_group_leader is only the foreground job group.
/// Kill all groups in this *live owned child's* SID, using pidfds to avoid PID
/// reuse races. A standalone detached session is outside this PTY's custody.
#[cfg(target_os = "linux")]
fn kill_linux_session(leader: u32) -> cowork_domain::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let sid = libc::pid_t::try_from(leader).map_err(io_error)?;
    if sid <= 1 || unsafe { libc::getsid(sid) } != sid {
        return Err(invalid("Owned terminal session could not be verified"));
    }
    let entries = fs::read_dir("/proc").map_err(io_error)?;
    let mut handles = Vec::new();
    let mut kill = |pid: libc::pid_t| -> cowork_domain::Result<()> {
        if unsafe { libc::getsid(pid) } != sid {
            return Ok(());
        }
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if raw < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(io_error(error));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
        if unsafe { libc::getsid(pid) } != sid {
            return Ok(());
        }
        if unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        } < 0
        {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(io_error(error));
            }
        }
        handles.push(fd);
        Ok(())
    };
    // Stop the shell first, before it can create more job groups. It remains an
    // unreaped child throughout this operation, so its SID cannot be recycled.
    kill(sid)?;
    for entry in entries {
        let entry = entry.map_err(io_error)?;
        if let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|v| v.parse::<libc::pid_t>().ok())
            && pid > 1
            && pid != sid
        {
            kill(pid)?;
        }
    }
    let mut polling: Vec<libc::pollfd> = handles
        .iter()
        .map(|fd| libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if unsafe { libc::poll(polling.as_mut_ptr(), polling.len() as libc::nfds_t, 50) } < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINTR) {
                return Err(io_error(error));
            }
        }
        if polling
            .iter()
            .all(|p| p.revents & (libc::POLLIN | libc::POLLHUP) != 0)
        {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(invalid("Terminal descendants have not confirmed exit"));
        }
    }
}

pub fn router(service: Arc<TerminalService>) -> Router<AppState> {
    Router::new()
        .route("/terminals/status", get(global_status))
        .route("/orders/{order}/terminals", get(list).post(launch))
        .route("/orders/{order}/terminals/preview", post(preview))
        .route("/orders/{order}/terminals/{session}/output", get(output))
        .route(
            "/orders/{order}/terminals/{session}/input",
            post(input_bytes),
        )
        .route("/orders/{order}/terminals/{session}/resize", post(resize))
        .route("/orders/{order}/terminals/{session}/stop", post(stop))
        .layer(axum::Extension(service))
        .layer(axum::extract::DefaultBodyLimit::max(32768))
}
async fn global_status(
    axum::Extension(t): axum::Extension<Arc<TerminalService>>,
) -> Answer<OwnerTerminalStatus> {
    let count = t.active_count();
    Ok(Json(OwnerTerminalStatus {
        active_sessions: u32::try_from(count).map_err(|_| ApiFailure(Error::Conflict))?,
    }))
}
async fn check(s: &AppState, order: &str) -> cowork_domain::Result<()> {
    s.runtime.store.native_chat_available(order).await?;
    Ok(())
}
async fn list(
    State(s): State<AppState>,
    axum::Extension(t): axum::Extension<Arc<TerminalService>>,
    Path(order): Path<String>,
) -> Answer<Vec<OwnerTerminalSession>> {
    check(&s, &order).await?;
    Ok(Json(t.list(&order)?))
}
async fn preview(
    State(s): State<AppState>,
    axum::Extension(t): axum::Extension<Arc<TerminalService>>,
    Path(order): Path<String>,
    body: Input<OwnerTerminalLaunch>,
) -> Answer<OwnerTerminalPreview> {
    check(&s, &order).await?;
    Ok(Json(t.preview(&order, &super::input(body)?)?))
}
async fn launch(
    State(s): State<AppState>,
    axum::Extension(t): axum::Extension<Arc<TerminalService>>,
    Path(order): Path<String>,
    body: Input<OwnerTerminalLaunch>,
) -> Answer<OwnerTerminalSession> {
    check(&s, &order).await?;
    let request = super::input(body)?;
    let runtime = tokio::runtime::Handle::current();
    let store = s.runtime.store.clone();
    Ok(Json(
        tokio::task::spawn_blocking(move || {
            t.launch_checked(&order, &request, || {
                runtime.block_on(store.native_chat_available(&order))
            })
        })
        .await
        .map_err(|_| ApiFailure(Error::UncertainEffect))??,
    ))
}
#[derive(Deserialize)]
struct Cursor {
    #[serde(default)]
    after: u64,
}
async fn output(
    State(s): State<AppState>,
    axum::Extension(t): axum::Extension<Arc<TerminalService>>,
    Path((order, id)): Path<(String, String)>,
    Query(q): Query<Cursor>,
) -> Answer<OwnerTerminalOutput> {
    check(&s, &order).await?;
    Ok(Json(t.output(&order, &id, q.after)?))
}
async fn input_bytes(
    State(s): State<AppState>,
    axum::Extension(t): axum::Extension<Arc<TerminalService>>,
    Path((order, id)): Path<(String, String)>,
    body: Input<OwnerTerminalInput>,
) -> Answer<OwnerTerminalSession> {
    check(&s, &order).await?;
    let request = super::input(body)?;
    Ok(Json(
        tokio::task::spawn_blocking(move || t.input(&order, &id, request))
            .await
            .map_err(|_| ApiFailure(Error::UncertainEffect))??,
    ))
}
async fn resize(
    State(s): State<AppState>,
    axum::Extension(t): axum::Extension<Arc<TerminalService>>,
    Path((order, id)): Path<(String, String)>,
    body: Input<OwnerTerminalResize>,
) -> Answer<OwnerTerminalSession> {
    check(&s, &order).await?;
    Ok(Json(t.resize(&order, &id, super::input(body)?)?))
}
async fn stop(
    State(s): State<AppState>,
    axum::Extension(t): axum::Extension<Arc<TerminalService>>,
    Path((order, id)): Path<(String, String)>,
) -> Answer<OwnerTerminalSession> {
    check(&s, &order).await?;
    Ok(Json(
        tokio::task::spawn_blocking(move || t.stop(&order, &id))
            .await
            .map_err(|_| ApiFailure(Error::UncertainEffect))??,
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) const TEST_ORDER: &str = "00000000-0000-4000-8000-000000000001";
    pub(crate) fn service() -> (tempfile::TempDir, Arc<TerminalService>, String) {
        let temp = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let (name, path) = (
            "powershell",
            PathBuf::from(std::env::var("SystemRoot").unwrap())
                .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
        );
        #[cfg(unix)]
        let (name, path) = ("sh", PathBuf::from("/bin/sh"));
        let environment = ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP", "LANG"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
            .collect();
        let toolbox = cowork_toolbox::Toolbox::open(cowork_toolbox::Config {
            root: temp.path().join("toolbox"),
            programs: BTreeMap::from([(name.into(), path)]),
            environment,
            github_token: None,
        })
        .unwrap();
        (temp, TerminalService::open(toolbox).unwrap(), name.into())
    }
    #[test]
    fn live_chat_guard_is_held_under_workspace_lease_before_process_or_receipt() {
        let (_temp, service, program) = service();
        let mut request = OwnerTerminalLaunch {
            idempotency_key: "retired-chat".into(),
            program,
            rows: 24,
            cols: 80,
            approval_sha256: None,
        };
        request.approval_sha256 = Some(
            service
                .preview(TEST_ORDER, &request)
                .unwrap()
                .approval_sha256,
        );
        let result = service.launch_checked(TEST_ORDER, &request, || {
            assert!(
                service.toolbox.workspace_lease(TEST_ORDER).is_err(),
                "deletion cannot acquire custody while the launch checks chat state"
            );
            Err(Error::NotFound)
        });
        assert!(matches!(result, Err(Error::NotFound)));
        assert!(service.list(TEST_ORDER).unwrap().is_empty());
        assert!(
            !service
                .receipt_path(TEST_ORDER, &request.idempotency_key)
                .exists()
        );
        assert!(service.toolbox.workspace_lease(TEST_ORDER).is_ok());
    }
    #[test]
    fn recent_terminal_list_and_exact_receipts_survive_more_than_two_thousand_launches() {
        let (_temp, service, program) = service();
        let mut last = None;
        for index in 0..2055 {
            let key = format!("historical-{index}");
            let record = OwnerTerminalSession {
                id: id(),
                order_id: TEST_ORDER.into(),
                program: program.clone(),
                state: "starting".into(),
                pid: None,
                rows: 24,
                cols: 80,
                created_at: 1000 + index,
                exit_code: None,
                input_sequence: 0,
            };
            let receipt = Receipt {
                key: key.clone(),
                digest: "a".repeat(64),
                session: record.clone(),
            };
            fs::write(
                service.receipt_path(TEST_ORDER, &key),
                serde_json::to_vec(&receipt).unwrap(),
            )
            .unwrap();
            last = Some(record);
        }
        let rows = service.list(TEST_ORDER).unwrap();
        assert_eq!(rows.len(), 128);
        assert_eq!(rows.last().unwrap().id, last.unwrap().id);
        assert_eq!(rows.first().unwrap().created_at, 1000 + 2055 - 128);
        let original = service
            .launch(
                TEST_ORDER,
                &OwnerTerminalLaunch {
                    idempotency_key: "historical-0".into(),
                    program,
                    rows: 24,
                    cols: 80,
                    approval_sha256: Some("a".repeat(64)),
                },
            )
            .unwrap();
        assert_eq!(original.state, "interrupted");
        assert!(original.pid.is_none());
        assert_eq!(service.active_count(), 0);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_stop_reaps_owned_background_session_without_touching_outside_process() {
        struct Outside(std::process::Child);
        impl Drop for Outside {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut outside = Outside(
            std::process::Command::new("/bin/sleep")
                .arg("600")
                .spawn()
                .unwrap(),
        );
        let (_temp, service, program) = service();
        let mut request = OwnerTerminalLaunch {
            idempotency_key: "background".into(),
            program,
            rows: 24,
            cols: 80,
            approval_sha256: None,
        };
        request.approval_sha256 = Some(
            service
                .preview(TEST_ORDER, &request)
                .unwrap()
                .approval_sha256,
        );
        let session = service.launch(TEST_ORDER, &request).unwrap();
        let command = "sleep 600 </dev/null >/dev/null 2>&1 & printf 'PTY_%s:%s\\n' 'BG' \"$!\"\n";
        service
            .input(
                TEST_ORDER,
                &session.id,
                OwnerTerminalInput {
                    sequence: 1,
                    bytes_base64: STANDARD.encode(command),
                },
            )
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let background = loop {
            let output = service.output(TEST_ORDER, &session.id, 0).unwrap();
            let bytes = STANDARD.decode(output.bytes_base64).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            if let Some(value) = text
                .split("PTY_BG:")
                .nth(1)
                .and_then(|value| {
                    value
                        .trim_start()
                        .split(|c: char| !c.is_ascii_digit())
                        .next()
                })
                .and_then(|v| v.parse::<i32>().ok())
            {
                break value;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Actual background PID missing"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(
            unsafe { libc::getsid(background) },
            session.pid.unwrap() as i32
        );
        assert_ne!(unsafe { libc::getsid(background) }, unsafe {
            libc::getsid(0)
        });
        assert_eq!(service.active_count(), 1);
        assert_eq!(
            service.stop(TEST_ORDER, &session.id).unwrap().state,
            "stopped"
        );
        assert_eq!(service.active_count(), 0);
        let state = fs::read_to_string(format!("/proc/{background}/stat"))
            .ok()
            .and_then(|v| {
                v.rsplit_once(')')
                    .map(|(_, rest)| rest.trim_start().chars().next().unwrap_or('Z'))
            });
        assert!(
            state.is_none_or(|v| v == 'Z' || v == 'X'),
            "Owned background descendant remains live"
        );
        assert!(
            outside.0.try_wait().unwrap().is_none(),
            "Unrelated process was touched"
        );
    }
    fn answer_cursor_queries(
        service: &TerminalService,
        session: &OwnerTerminalSession,
        output: &[u8],
        answered: &mut usize,
    ) {
        let requested = output
            .windows(4)
            .filter(|bytes| *bytes == b"\x1b[6n")
            .count();
        while *answered < requested {
            let sequence = service
                .output(TEST_ORDER, &session.id, 0)
                .unwrap()
                .session
                .input_sequence
                + 1;
            service
                .input(
                    TEST_ORDER,
                    &session.id,
                    OwnerTerminalInput {
                        sequence,
                        bytes_base64: STANDARD.encode(b"\x1b[1;1R"),
                    },
                )
                .unwrap();
            *answered += 1;
        }
    }
    #[test]
    fn pty_real_input_resize_reconnect_stop_and_launch_receipt() {
        let (_temp, service, program) = service();
        let mut request = OwnerTerminalLaunch {
            idempotency_key: "original".into(),
            program,
            rows: 24,
            cols: 80,
            approval_sha256: None,
        };
        let preview = service.preview(TEST_ORDER, &request).unwrap();
        assert!(service.list(TEST_ORDER).unwrap().is_empty());
        assert!(service.launch(TEST_ORDER, &request).is_err());
        request.approval_sha256 = Some(preview.approval_sha256);
        let session = service.launch(TEST_ORDER, &request).unwrap();
        assert!(session.pid.is_some());
        assert_eq!(service.launch(TEST_ORDER, &request).unwrap().id, session.id);
        let mut changed = request.clone();
        changed.cols = 90;
        assert!(matches!(
            service.launch(TEST_ORDER, &changed),
            Err(Error::Conflict)
        ));
        assert!(matches!(
            service.output("other", &session.id, 0),
            Err(Error::NotFound)
        ));
        let resized = service
            .resize(
                TEST_ORDER,
                &session.id,
                OwnerTerminalResize { rows: 31, cols: 97 },
            )
            .unwrap();
        assert_eq!((resized.rows, resized.cols), (31, 97));
        let real = lock(&service.find(TEST_ORDER, &session.id).unwrap())
            .unwrap()
            .master
            .as_ref()
            .unwrap()
            .get_size()
            .unwrap();
        assert_eq!((real.rows, real.cols), (31, 97));
        let mut output = Vec::new();
        let mut after = 0;
        let mut answered_cursor_queries = 0;
        #[cfg(windows)]
        {
            // A real terminal answers ConPTY's startup cursor query before a
            // user enters a command. Early command bytes can be consumed as
            // that reply while the initial PowerShell prompt is still starting.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                let value = service.output(TEST_ORDER, &session.id, after).unwrap();
                output.extend(STANDARD.decode(value.bytes_base64).unwrap());
                after = value.next_sequence;
                answer_cursor_queries(&service, &session, &output, &mut answered_cursor_queries);
                let text = String::from_utf8_lossy(&output);
                if text.contains("PS ") && text.contains('>') {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "PowerShell prompt missing: {text}"
                );
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        }
        let command_sequence = service
            .output(TEST_ORDER, &session.id, after)
            .unwrap()
            .session
            .input_sequence
            + 1;
        #[cfg(windows)]
        // A terminal Enter is CR. CRLF also injects Ctrl+Enter through ConPTY,
        // which PSReadLine binds to editing the current command instead.
        // Use a CLR expression to keep this lifecycle probe independent of
        // cmdlet modules.
        let command = "[Console]::WriteLine('PTY_'+'ECHO_CONFIRMED')\r";
        #[cfg(unix)]
        let command = "printf 'PTY_%s\\n' 'ECHO_CONFIRMED'\n";
        let input = OwnerTerminalInput {
            sequence: command_sequence,
            bytes_base64: STANDARD.encode(command),
        };
        assert_eq!(
            service
                .input(TEST_ORDER, &session.id, input.clone())
                .unwrap()
                .input_sequence,
            command_sequence
        );
        assert_eq!(
            service
                .input(TEST_ORDER, &session.id, input)
                .unwrap()
                .input_sequence,
            command_sequence
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let value = service.output(TEST_ORDER, &session.id, after).unwrap();
            let bytes = STANDARD.decode(value.bytes_base64).unwrap();
            output.extend(&bytes);
            after = value.next_sequence;
            answer_cursor_queries(&service, &session, &output, &mut answered_cursor_queries);
            if String::from_utf8_lossy(&output).contains("PTY_ECHO_CONFIRMED") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Actual PTY echo missing: {}",
                String::from_utf8_lossy(&output)
            );
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        let same = service.output(TEST_ORDER, &session.id, 0).unwrap();
        assert!(
            STANDARD
                .decode(same.bytes_base64)
                .unwrap()
                .windows(18)
                .any(|v| v == b"PTY_ECHO_CONFIRMED")
        );
        let stopped = service.stop(TEST_ORDER, &session.id).unwrap();
        assert_eq!(stopped.state, "stopped");
        assert!(stopped.pid.is_none());
        assert_eq!(
            service.stop(TEST_ORDER, &session.id).unwrap().state,
            "stopped"
        );
        let mut next = request.clone();
        next.idempotency_key = "next-after-stop".into();
        next.approval_sha256 = Some(service.preview(TEST_ORDER, &next).unwrap().approval_sha256);
        let next_session = service.launch(TEST_ORDER, &next).unwrap();
        service.stop(TEST_ORDER, &next_session.id).unwrap();
        // Reopening the service must never attach/relaunch from stale OS PIDs.
        let restarted = TerminalService::open(service.toolbox.clone()).unwrap();
        let replay = restarted.launch(TEST_ORDER, &request).unwrap();
        assert_eq!(replay.id, session.id);
        assert_eq!(replay.state, "interrupted");
        assert!(replay.pid.is_none());
    }
}
