//! One original private portal helper per desktop stream. Dropping the stream
//! kills/reaps that exact child and joins its IO worker before releasing the gate.
use super::{Backend, Frame, MAX_CLIPBOARD_BYTES};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, OnceLock, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tokio_util::sync::CancellationToken;

const SCRIPT: &str = include_str!("desktop_wayland.py");
const MAX_HEADER: usize = 64 * 1024;
const MAX_COMMAND: usize = 6 * MAX_CLIPBOARD_BYTES + 4096;
const MAX_FRAME: usize = 7680 * 4320 * 4;

fn command(probe: bool) -> Result<Command> {
    let path = std::path::Path::new("/usr/bin/python3");
    let actual =
        std::fs::canonicalize(path).context("Wayland requires operator-installed Python3")?;
    if !actual.is_file() {
        bail!("Wayland Python executable unavailable");
    }
    let mut command = Command::new(actual);
    command.env_clear().args(["-I", "-u", "-c", SCRIPT]);
    if probe {
        command.arg("--probe");
    }
    for name in [
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_RUNTIME_DIR",
        "WAYLAND_DISPLAY",
        "XDG_SESSION_ID",
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "LANG",
        "LC_ALL",
        "HOME",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let parent = unsafe { libc::getpid() };
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
            }
            Ok(())
        });
    }
    Ok(command)
}

struct OriginalChild {
    child: Mutex<Option<Child>>,
    stop: CancellationToken,
}
impl OriginalChild {
    fn end(&self) {
        self.stop.cancel();
        if let Ok(mut owned) = self.child.lock()
            && let Some(mut child) = owned.take()
        {
            // Only the unreaped original Child handle is used. Never select
            // a helper by process name, inherited PID, or portal token.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Drop for OriginalChild {
    fn drop(&mut self) {
        self.end();
    }
}

struct Response {
    header: Value,
    payload: Vec<u8>,
}
struct Request {
    value: Value,
    reply: oneshot::Sender<Result<Response>>,
}

struct Helper {
    original: Arc<OriginalChild>,
    sender: mpsc::SyncSender<Request>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl Helper {
    fn new(stop: CancellationToken) -> Result<Self> {
        let mut child = command(false)?
            .spawn()
            .context("Wayland portal helper could not start")?;
        let Some(mut input) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Wayland helper input unavailable");
        };
        let Some(mut output) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Wayland helper output unavailable");
        };
        let original = Arc::new(OriginalChild {
            child: Mutex::new(Some(child)),
            stop,
        });
        let (sender, receiver) = mpsc::sync_channel::<Request>(1);
        let worker_owner = original.clone();
        let worker = std::thread::Builder::new()
            .name("wayland-portal-io".into())
            .spawn(move || {
                while !worker_owner.stop.is_cancelled() {
                    let request = match receiver.recv_timeout(Duration::from_millis(100)) {
                        Ok(request) => request,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let result = (|| -> Result<Response> {
                        if worker_owner.stop.is_cancelled() {
                            bail!("Wayland session canceled");
                        }
                        let raw = serde_json::to_vec(&request.value)?;
                        if raw.len() > MAX_COMMAND {
                            bail!("Wayland command exceeds its bound");
                        }
                        input.write_all(&raw)?;
                        input.write_all(b"\n")?;
                        input.flush()?;
                        let maximum = match request.value.get("op").and_then(Value::as_str) {
                            Some("frame") => MAX_FRAME,
                            Some("clipboard_read") => MAX_CLIPBOARD_BYTES,
                            _ => 0,
                        };
                        read_response(&mut output, maximum)
                    })();
                    let failed = result.is_err();
                    let _ = request.reply.send(result);
                    if failed {
                        worker_owner.end();
                        break;
                    }
                }
                worker_owner.end();
            })
            .context("Wayland helper IO worker could not start")?;
        Ok(Self {
            original,
            sender,
            worker: Mutex::new(Some(worker)),
        })
    }
    async fn request(&self, value: Value, timeout: Duration) -> Result<Response> {
        if self.original.stop.is_cancelled() {
            bail!("Wayland session ended");
        }
        let (reply, receive) = oneshot::channel();
        self.sender
            .try_send(Request { value, reply })
            .map_err(|_| anyhow::anyhow!("Wayland helper busy or closed"))?;
        let result = tokio::select! { biased;
            _=self.original.stop.cancelled()=>Err(anyhow::anyhow!("Wayland session canceled")),
            result=tokio::time::timeout(timeout,receive)=>match result {
                Ok(Ok(result))=>result,
                _=>Err(anyhow::anyhow!("Wayland portal operation unconfirmed")),
            },
        };
        if result.is_err() {
            self.original.end();
        }
        result
    }
    fn end(&self) {
        self.original.end();
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        self.end();
    }
}

fn read_response(output: &mut impl Read, maximum: usize) -> Result<Response> {
    let mut length = [0_u8; 4];
    output.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if !(1..=MAX_HEADER).contains(&length) {
        bail!("Wayland response header exceeds its bound");
    }
    let mut raw = vec![0_u8; length];
    output.read_exact(&mut raw)?;
    let header: Value = serde_json::from_slice(&raw).context("Invalid Wayland helper header")?;
    let bytes = header
        .get("bytes")
        .and_then(Value::as_u64)
        .context("Wayland response byte count missing")?;
    if bytes > maximum as u64 {
        bail!("Wayland response payload exceeds this operation's bound");
    }
    if header.get("ok") != Some(&Value::Bool(true)) {
        // No helper-provided paths, clipboard data, portal handles or error
        // texts are copied to stderr/the network.
        bail!("Wayland portal rejected or revoked this operation");
    }
    let mut payload = vec![0_u8; bytes as usize];
    output.read_exact(&mut payload)?;
    Ok(Response { header, payload })
}

pub(super) fn available() -> bool {
    static CACHE: OnceLock<Mutex<Option<(Instant, bool)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let Ok(mut cached) = cache.lock() else {
        return false;
    };
    if let Some((at, value)) = *cached
        && at.elapsed() < Duration::from_secs(2)
    {
        return value;
    }
    let value = (|| -> Result<bool> {
        struct Probe(Child);
        impl Drop for Probe {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Probe(command(true)?.stdin(Stdio::null()).spawn()?);
        let started = Instant::now();
        loop {
            if let Some(status) = child.0.try_wait()? {
                if !status.success() {
                    return Ok(false);
                }
                let mut output = child
                    .0
                    .stdout
                    .take()
                    .context("Wayland probe output unavailable")?;
                let response = read_response(&mut output, 0)?;
                return Ok(response.payload.is_empty()
                    && response.header.get("available") == Some(&Value::Bool(true)));
            }
            if started.elapsed() > Duration::from_secs(3) {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })()
    .unwrap_or(false);
    *cached = Some((Instant::now(), value));
    value
}

pub(super) struct Session {
    helper: Helper,
    dimensions: (u16, u16),
    control: bool,
    clipboard: bool,
    _gate: OwnedSemaphorePermit,
}
impl Session {
    pub(super) async fn open(control: bool, stop: CancellationToken) -> Result<Self> {
        static GATE: OnceLock<Arc<Semaphore>> = OnceLock::new();
        let gate = GATE
            .get_or_init(|| Arc::new(Semaphore::new(1)))
            .clone()
            .try_acquire_owned()
            .context("Another explicit Wayland desktop session is already active")?;
        let helper = Helper::new(stop)?;
        let response = helper
            .request(
                json!({"op":"open","control":control}),
                Duration::from_secs(100),
            )
            .await?;
        if !response.payload.is_empty() {
            bail!("Unexpected Wayland open payload");
        }
        let dimensions = dimensions(&response.header)?;
        let clipboard = control && response.header.get("clipboard") == Some(&Value::Bool(true));
        Ok(Self {
            helper,
            dimensions,
            control,
            clipboard,
            _gate: gate,
        })
    }
}
fn dimensions(header: &Value) -> Result<(u16, u16)> {
    let width = header
        .get("width")
        .and_then(Value::as_u64)
        .context("Wayland width missing")?;
    let height = header
        .get("height")
        .and_then(Value::as_u64)
        .context("Wayland height missing")?;
    if !(1..=7680).contains(&width) || !(1..=4320).contains(&height) {
        bail!("Invalid Wayland framebuffer dimensions");
    }
    Ok((width as u16, height as u16))
}

#[async_trait::async_trait]
impl Backend for Session {
    fn dimensions(&self) -> (u16, u16) {
        self.dimensions
    }
    async fn capture(&self) -> Result<Frame> {
        let response = self
            .helper
            .request(json!({"op":"frame"}), Duration::from_secs(5))
            .await?;
        let (width, height) = dimensions(&response.header)?;
        if response.payload.len() != width as usize * height as usize * 4 {
            bail!("Wayland frame byte geometry differs");
        }
        Ok(Frame {
            width,
            height,
            bgra: response.payload,
        })
    }
    async fn key(&self, keysym: u32, down: bool) -> Result<()> {
        if !self.control {
            bail!("Wayland session is view-only");
        }
        self.helper
            .request(
                json!({"op":"key","keysym":keysym,"down":down}),
                Duration::from_secs(3),
            )
            .await?;
        Ok(())
    }
    async fn pointer(
        &self,
        x: u16,
        y: u16,
        buttons: u8,
        previous: u8,
        width: u16,
        height: u16,
    ) -> Result<()> {
        if !self.control {
            bail!("Wayland session is view-only");
        }
        self.helper.request(json!({"op":"pointer","x":x,"y":y,"buttons":buttons,"previous":previous,"width":width,"height":height}),Duration::from_secs(3)).await?;
        Ok(())
    }
    async fn clipboard_read(&self) -> Result<Option<String>> {
        if !self.clipboard {
            return Ok(None);
        }
        let response = self
            .helper
            .request(json!({"op":"clipboard_read"}), Duration::from_secs(3))
            .await?;
        if response.payload.len() > MAX_CLIPBOARD_BYTES {
            bail!("Wayland clipboard exceeds its bound");
        }
        let present = response
            .header
            .get("present")
            .and_then(Value::as_bool)
            .context("Wayland clipboard presence missing")?;
        if !present {
            if !response.payload.is_empty() {
                bail!("Absent Wayland clipboard has a payload");
            }
            return Ok(None);
        }
        Ok(Some(String::from_utf8(response.payload)?))
    }
    async fn clipboard_write(&self, text: &str) -> Result<()> {
        if !self.clipboard {
            return Ok(());
        }
        if text.len() > MAX_CLIPBOARD_BYTES {
            bail!("Wayland clipboard exceeds its bound");
        }
        self.helper
            .request(
                json!({"op":"clipboard_write","text":text}),
                Duration::from_secs(3),
            )
            .await?;
        Ok(())
    }
    fn release(&self, _keys: &BTreeSet<u32>, _buttons: u8) {
        self.helper.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn wire(header: Value, payload: &[u8]) -> Vec<u8> {
        let header = serde_json::to_vec(&header).unwrap();
        let mut wire = (header.len() as u32).to_be_bytes().to_vec();
        wire.extend(header);
        wire.extend(payload);
        wire
    }

    #[test]
    fn portal_frames_enforce_operation_and_geometry_bounds() {
        let parsed = read_response(
            &mut Cursor::new(wire(
                json!({"ok":true,"bytes":4,"width":1,"height":1}),
                &[1, 2, 3, 4],
            )),
            4,
        )
        .unwrap();
        assert_eq!(dimensions(&parsed.header).unwrap(), (1, 1));
        assert_eq!(parsed.payload, [1, 2, 3, 4]);
        for header in [
            json!({"ok":true,"bytes":5}),
            json!({"ok":true,"bytes":-1}),
            json!({"ok":true}),
            json!({"ok":false,"bytes":0}),
        ] {
            assert!(read_response(&mut Cursor::new(wire(header, &[])), 4).is_err());
        }
        assert!(read_response(&mut Cursor::new((MAX_HEADER as u32 + 1).to_be_bytes()), 4).is_err());
        assert!(
            read_response(
                &mut Cursor::new(wire(json!({"ok":true,"bytes":4}), &[1])),
                4
            )
            .is_err()
        );
        for header in [
            json!({"width":0,"height":1}),
            json!({"width":7681,"height":1}),
            json!({"width":1,"height":4321}),
            json!({"width":true,"height":1}),
        ] {
            assert!(dimensions(&header).is_err());
        }
    }

    #[test]
    fn ending_the_original_child_observes_its_kernel_exit() {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let child = Command::new("/usr/bin/sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id(), 0) };
        let original = OriginalChild {
            child: Mutex::new(Some(child)),
            stop: CancellationToken::new(),
        };
        // Capture the exact child's kernel identity while its original Child
        // remains unreaped; no post-exit PID lookup or process-name adoption.
        if raw < 0 {
            original.end();
            panic!("The Linux probe requires pidfd support");
        }
        let pidfd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
        original.end();
        let mut event = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut event, 1, 0) }, 1);
        assert_ne!(event.revents & libc::POLLIN, 0);
        assert!(original.stop.is_cancelled());
        assert!(original.child.lock().unwrap().is_none());
        original.end();
    }
}
