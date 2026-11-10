//! Private bounded FIFO capture. Workloads get only stdout/stderr descriptors;
//! the control directory, FIFO paths and root journal never become mounts.
use super::*;
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::Dir;
use std::{
    os::fd::AsRawFd,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};

#[derive(Clone, Copy)]
struct NativeId {
    dev: u64,
    ino: u64,
}
impl NativeId {
    fn of(file: &File) -> Result<Self> {
        let m = file.metadata()?;
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
        })
    }
    fn matches(self, file: &File) -> Result<bool> {
        let m = file.metadata()?;
        Ok(self.dev == m.dev() && self.ino == m.ino())
    }
}
struct Observation {
    finish: AtomicBool,
    drain: AtomicBool,
    stop: AtomicBool,
    exceeded: AtomicBool,
    failed: AtomicBool,
    total: AtomicU64,
    maximum: u64,
}
type Reader = std::thread::JoinHandle<Result<(cowork_executor::Capture, bool)>>;
pub struct BoundedOutput {
    directory: Dir,
    directory_id: NativeId,
    path: PathBuf,
    fifo_ids: [NativeId; 2],
    keepers: [Option<File>; 2],
    readers: [Option<Reader>; 2],
    observation: Arc<Observation>,
}
pub struct CapturedOutput {
    pub stdout: cowork_executor::Capture,
    pub stderr: cowork_executor::Capture,
    pub complete: bool,
    pub limit_exceeded: bool,
}
impl BoundedOutput {
    /// Fresh private control directory, not the UID-1000 data mount. This does
    /// not authorize execution or create a physical stop/completion witness.
    pub fn create(path: &Path, keep_per_stream: usize, maximum_total_bytes: u64) -> Result<Self> {
        if unsafe { libc::geteuid() } != 0
            || !path.is_absolute()
            || !(1..=4 * 1024 * 1024).contains(&keep_per_stream)
            || !(1..=128 * 1024 * 1024).contains(&maximum_total_bytes)
        {
            return Err(Error::Output);
        }
        for parent in path.ancestors() {
            if std::fs::symlink_metadata(parent)?.file_type().is_symlink() {
                return Err(Error::Output);
            }
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)?;
        let m = file.metadata()?;
        if m.uid() != 0 || m.mode() & 0o077 != 0 {
            return Err(Error::Output);
        }
        let directory_id = NativeId::of(&file)?;
        let directory = Dir::from_std_file(file);
        for name in [c"stdout", c"stderr"] {
            if unsafe { libc::mkfifoat(directory.as_raw_fd(), name.as_ptr(), 0o600) } < 0 {
                return Err(io::Error::last_os_error().into());
            }
        }
        let (stdout, stdout_keeper) = open_fifo(&directory, "stdout")?;
        let (stderr, stderr_keeper) = open_fifo(&directory, "stderr")?;
        let fifo_ids = [NativeId::of(&stdout)?, NativeId::of(&stderr)?];
        let observation = Arc::new(Observation {
            finish: AtomicBool::new(false),
            drain: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            exceeded: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            total: AtomicU64::new(0),
            maximum: maximum_total_bytes,
        });
        let mut output = Self {
            directory,
            directory_id,
            path: path.into(),
            fifo_ids,
            keepers: [Some(stdout_keeper), Some(stderr_keeper)],
            readers: [None, None],
            observation,
        };
        for (i, mut file) in [stdout, stderr].into_iter().enumerate() {
            let state = output.observation.clone();
            output.readers[i] = Some(
                std::thread::Builder::new()
                    .name(format!("cowork-output-{i}"))
                    .spawn(move || {
                        let mut capture = cowork_executor::Capture::default();
                        let mut buffer = [0u8; 8192];
                        let mut finishing = None;
                        let complete = loop {
                            if state.stop.load(Ordering::Acquire) {
                                break false;
                            }
                            if state.finish.load(Ordering::Acquire) {
                                let start = finishing.get_or_insert_with(Instant::now);
                                if start.elapsed() >= Duration::from_secs(2) {
                                    break false;
                                }
                            } else if state.exceeded.load(Ordering::Acquire)
                                && !state.drain.load(Ordering::Acquire)
                            {
                                // Backpressure bounds the unread kernel pipe. The
                                // adapter cancels the exact task before final draining.
                                std::thread::sleep(Duration::from_millis(2));
                                continue;
                            }
                            match file.read(&mut buffer) {
                                Ok(0) => break true,
                                Ok(n) => {
                                    capture.total_bytes =
                                        capture.total_bytes.saturating_add(n as u64);
                                    let keep =
                                        n.min(keep_per_stream.saturating_sub(capture.bytes.len()));
                                    capture.bytes.extend_from_slice(&buffer[..keep]);
                                    capture.truncated |= keep < n;
                                    let before = state
                                        .total
                                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                                            Some(v.saturating_add(n as u64))
                                        })
                                        .unwrap();
                                    if before.saturating_add(n as u64) > state.maximum {
                                        state.exceeded.store(true, Ordering::Release);
                                    }
                                }
                                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                                    std::thread::sleep(Duration::from_millis(2))
                                }
                                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                                Err(_) => {
                                    state.failed.store(true, Ordering::Release);
                                    return Err(Error::Output);
                                }
                            }
                        };
                        capture.invalid_utf8 = std::str::from_utf8(&capture.bytes).is_err();
                        Ok((capture, complete))
                    })?,
            );
        }
        output.verify()?;
        Ok(output)
    }
    pub fn stdout_path(&self) -> PathBuf {
        self.path.join("stdout")
    }
    pub fn stderr_path(&self) -> PathBuf {
        self.path.join("stderr")
    }
    pub fn limit_exceeded(&self) -> bool {
        self.observation.exceeded.load(Ordering::Acquire)
    }
    pub fn observer_failed(&self) -> bool {
        self.observation.failed.load(Ordering::Acquire)
    }
    /// The adapter has separately observed the exact protected workload branch
    /// empty with every creator acknowledged. Drain bounded in-flight pipe
    /// buffers so shim deletion can join its copier. Keepers stay open until
    /// acknowledged deletion; this does not claim EOF or physical completion.
    pub(super) fn drain_stopped_workload(&self) -> Result<()> {
        self.verify()?;
        self.observation.drain.store(true, Ordering::Release);
        Ok(())
    }
    pub fn verify(&self) -> Result<()> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&self.path)?;
        if !self.directory_id.matches(&file)? {
            return Err(Error::Output);
        }
        let m = file.metadata()?;
        if m.uid() != 0 || m.mode() & 0o077 != 0 {
            return Err(Error::Output);
        }
        for (i, name) in ["stdout", "stderr"].into_iter().enumerate() {
            let mut options = cap_std::fs::OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
            let file = self.directory.open_with(name, &options)?.into_std();
            if !self.fifo_ids[i].matches(&file)? || !valid_fifo(&file)? {
                return Err(Error::Output);
            }
        }
        Ok(())
    }
    /// Call only after the owned task cannot create another output writer. EOF
    /// is observed independently; timing out never turns into complete output.
    pub fn finish(mut self) -> Result<CapturedOutput> {
        self.keepers = [None, None];
        self.observation.finish.store(true, Ordering::Release);
        let mut captures = Vec::new();
        let mut complete = true;
        for reader in &mut self.readers {
            let (capture, eof) = reader
                .take()
                .ok_or(Error::Output)?
                .join()
                .map_err(|_| Error::Output)??;
            captures.push(capture);
            complete &= eof;
        }
        let stderr = captures.pop().ok_or(Error::Output)?;
        let stdout = captures.pop().ok_or(Error::Output)?;
        Ok(CapturedOutput {
            stdout,
            stderr,
            complete,
            limit_exceeded: self.limit_exceeded(),
        })
    }
}
impl Drop for BoundedOutput {
    fn drop(&mut self) {
        self.observation.stop.store(true, Ordering::Release);
        self.keepers = [None, None];
        for reader in &mut self.readers {
            if let Some(reader) = reader.take() {
                let _ = reader.join();
            }
        }
        // FIFO/control files remain for explicit physical reconciliation.
    }
}
fn open_fifo(dir: &Dir, name: &str) -> Result<(File, File)> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    use cap_std::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    let reader = dir.open_with(name, &options)?.into_std();
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .write(true)
        .follow(FollowSymlinks::No)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    let writer = dir.open_with(name, &options)?.into_std();
    if !valid_fifo(&reader)? || !valid_fifo(&writer)? || !NativeId::of(&reader)?.matches(&writer)? {
        return Err(Error::Output);
    }
    Ok((reader, writer))
}
fn valid_fifo(file: &File) -> Result<bool> {
    let m = file.metadata()?;
    Ok(m.file_type().is_fifo() && m.uid() == 0 && m.mode() & 0o077 == 0 && m.nlink() == 1)
}
