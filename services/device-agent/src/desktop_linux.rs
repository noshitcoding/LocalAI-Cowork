//! Native X11 or explicitly portal-granted Wayland over the same V2 RFB channel.
//! Only the operator's active unlocked logind session is accepted.
//! No system package installation, external VNC or V1 transport is involved.
use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::{BTreeSet, VecDeque},
    time::Duration,
};
use std::{
    ffi::c_void,
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};
use tokio_util::sync::CancellationToken;

use image::{ColorType, codecs::jpeg::JpegEncoder};
const RFB_VERSION: &[u8; 12] = b"RFB 003.008\n";
const ENCODING_RAW: i32 = 0;
const ENCODING_TIGHT: i32 = 7;
const MAX_CLIENT_MESSAGE: usize = 16 * 1024 * 1024;
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

#[path = "desktop_wayland.rs"]
mod wayland;

fn uses_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|value| value == "wayland")
        || std::env::var("XDG_SESSION_TYPE").is_err()
            && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub fn available() -> bool {
    local_unlocked_session()
        && if uses_wayland() {
            wayland::available()
        } else {
            api().is_ok() && screen_dimensions().is_ok()
        }
}

fn check_access(stop: &CancellationToken) -> Result<()> {
    if stop.is_cancelled() {
        bail!("Device desktop session canceled");
    }
    if !local_unlocked_session() {
        bail!("The operator's active unlocked local desktop is required");
    }
    Ok(())
}

struct PressedInputs {
    backend: Arc<dyn Backend>,
    keys: BTreeSet<u32>,
    buttons: u8,
}
impl Drop for PressedInputs {
    fn drop(&mut self) {
        // Release only input held by this stream, including when its future is
        // canceled. There are no detached input workers after returning.
        self.backend.release(&self.keys, self.buttons);
    }
}

#[async_trait::async_trait]
trait Backend: Send + Sync {
    fn dimensions(&self) -> (u16, u16);
    async fn capture(&self) -> Result<Frame>;
    async fn key(&self, keysym: u32, down: bool) -> Result<()>;
    async fn pointer(
        &self,
        x: u16,
        y: u16,
        buttons: u8,
        previous: u8,
        width: u16,
        height: u16,
    ) -> Result<()>;
    async fn clipboard_read(&self) -> Result<Option<String>>;
    async fn clipboard_write(&self, text: &str) -> Result<()>;
    fn release(&self, keys: &BTreeSet<u32>, buttons: u8);
}

struct X11Backend {
    dimensions: (u16, u16),
    clipboard: Mutex<Option<Child>>,
}

#[async_trait::async_trait]
impl Backend for X11Backend {
    fn dimensions(&self) -> (u16, u16) {
        self.dimensions
    }
    async fn capture(&self) -> Result<Frame> {
        capture_screen()
    }
    async fn key(&self, keysym: u32, down: bool) -> Result<()> {
        inject_key(keysym, down)
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
        inject_pointer(x, y, buttons, previous, width, height)
    }
    async fn clipboard_read(&self) -> Result<Option<String>> {
        read_clipboard()
    }
    async fn clipboard_write(&self, text: &str) -> Result<()> {
        let mut owned = self
            .clipboard
            .lock()
            .map_err(|_| anyhow::anyhow!("Local clipboard state unavailable"))?;
        stop_clipboard(&mut owned);
        *owned = Some(set_clipboard(text)?);
        Ok(())
    }
    fn release(&self, keys: &BTreeSet<u32>, buttons: u8) {
        for key in keys {
            let _ = inject_key(*key, false);
        }
        for (mask, button) in [(1, 1), (2, 2), (4, 3)] {
            if buttons & mask != 0 {
                let _ = inject_button(button, false);
            }
        }
        if let Ok(mut child) = self.clipboard.lock() {
            stop_clipboard(&mut child);
        }
    }
}

pub async fn serve<S>(
    socket: WebSocketStream<S>,
    control: bool,
    stop: CancellationToken,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    check_access(&stop)?;
    let backend: Arc<dyn Backend> = if uses_wayland() {
        Arc::new(wayland::Session::open(control, stop.clone()).await?)
    } else {
        Arc::new(X11Backend {
            dimensions: screen_dimensions()?,
            clipboard: Mutex::new(None),
        })
    };
    let (width, height) = backend.dimensions();
    // Guard exists before the first await on RFB; handshake loss closes the
    // exact original portal helper too, including held virtual input devices.
    let mut pressed = PressedInputs {
        backend: backend.clone(),
        keys: BTreeSet::new(),
        buttons: 0,
    };
    let mut stream = WsBytes::new(socket, stop);
    stream.send(RFB_VERSION.to_vec()).await?;
    let version = stream.read_exact(12).await?;
    if version.as_slice() != RFB_VERSION {
        bail!("desktop client requested an unsupported RFB version");
    }
    stream.send(vec![1, 1]).await?;
    if stream.read_exact(1).await?[0] != 1 {
        bail!("desktop client rejected the RFB None security type");
    }
    stream.send(vec![0, 0, 0, 0]).await?;
    let _shared = stream.read_exact(1).await?;
    stream.send(server_init(width, height)).await?;

    let mut supports_tight = false;
    let mut previous_buttons = 0_u8;
    let mut last_remote_clipboard: Option<String> = None;
    loop {
        match stream.read_u8().await? {
            0 => {
                let format = stream.read_exact(19).await?;
                if format[3..16] != [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0] {
                    bail!(
                        "The native desktop requires its advertised 32-bit true-color pixel format"
                    );
                }
            }
            2 => {
                let header = stream.read_exact(3).await?;
                let count = u16::from_be_bytes([header[1], header[2]]) as usize;
                if count > 4096 {
                    bail!("RFB client advertised too many encodings");
                }
                supports_tight = false;
                for _ in 0..count {
                    let bytes = stream.read_exact(4).await?;
                    if i32::from_be_bytes(bytes.try_into().expect("four bytes")) == ENCODING_TIGHT {
                        supports_tight = true;
                    }
                }
            }
            3 => {
                let request = stream.read_exact(9).await?;
                let requested_width = u16::from_be_bytes([request[5], request[6]]);
                let requested_height = u16::from_be_bytes([request[7], request[8]]);
                check_access(&stream.stop)?;
                let frame = backend.capture().await?;
                check_access(&stream.stop)?;
                if (frame.width, frame.height) != (width, height) {
                    bail!("Desktop geometry changed; open a new explicit desktop session");
                }
                let update = if supports_tight {
                    tight_update(
                        &frame,
                        width.min(requested_width.max(1)),
                        height.min(requested_height.max(1)),
                    )?
                } else {
                    raw_update(
                        &frame,
                        width.min(requested_width.max(1)),
                        height.min(requested_height.max(1)),
                    )?
                };
                stream.send(update).await?;
                if control {
                    check_access(&stream.stop)?;
                    if let Ok(Some(text)) = backend.clipboard_read().await
                        && last_remote_clipboard.as_deref() != Some(&text)
                    {
                        stream.send(server_cut_text(&text)).await?;
                        last_remote_clipboard = Some(text);
                    }
                }
            }
            4 => {
                let event = stream.read_exact(7).await?;
                if control {
                    check_access(&stream.stop)?;
                    let down = event[0] != 0;
                    let keysym = u32::from_be_bytes(event[3..7].try_into().expect("four bytes"));
                    if down && pressed.keys.len() >= 256 && !pressed.keys.contains(&keysym) {
                        bail!("Too many held remote keys");
                    }
                    backend.key(keysym, down).await?;
                    if down {
                        pressed.keys.insert(keysym);
                    } else {
                        pressed.keys.remove(&keysym);
                    }
                }
            }
            5 => {
                let event = stream.read_exact(5).await?;
                if control {
                    check_access(&stream.stop)?;
                    let buttons = event[0];
                    let x = u16::from_be_bytes([event[1], event[2]]);
                    let y = u16::from_be_bytes([event[3], event[4]]);
                    let old_buttons = previous_buttons;
                    backend
                        .pointer(x, y, buttons, old_buttons, width, height)
                        .await?;
                    previous_buttons = buttons & 0x07;
                    pressed.buttons = previous_buttons;
                }
            }
            6 => {
                let header = stream.read_exact(7).await?;
                let signed_length =
                    i32::from_be_bytes(header[3..7].try_into().expect("four bytes"));
                if signed_length < 0 {
                    let length = signed_length.unsigned_abs() as usize;
                    if length > MAX_CLIENT_MESSAGE {
                        bail!("extended RFB clipboard message exceeds the safety limit");
                    }
                    let _unsupported_extended_clipboard = stream.read_exact(length).await?;
                    continue;
                }
                let length = signed_length as usize;
                if length > MAX_CLIPBOARD_BYTES {
                    bail!("RFB clipboard message exceeds the safety limit");
                }
                let text = stream.read_exact(length).await?;
                if control {
                    check_access(&stream.stop)?;
                    let text: String = text.into_iter().map(char::from).collect();
                    backend.clipboard_write(&text).await?;
                }
            }
            message_type => bail!("unsupported RFB client message type {message_type}"),
        }
    }
}

struct WsBytes<S> {
    socket: WebSocketStream<S>,
    pending: VecDeque<u8>,
    stop: CancellationToken,
}

impl<S> WsBytes<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn new(socket: WebSocketStream<S>, stop: CancellationToken) -> Self {
        Self {
            socket,
            pending: VecDeque::new(),
            stop,
        }
    }

    async fn send(&mut self, bytes: Vec<u8>) -> Result<()> {
        if bytes.len() > MAX_CLIENT_MESSAGE {
            bail!("RFB frame exceeds its size limit");
        }
        tokio::select! { biased;
            _ = self.stop.cancelled() => bail!("Device desktop session canceled"),
            result = tokio::time::timeout(Duration::from_secs(5), self.socket.send(Message::Binary(bytes.into()))) => result.context("RFB write timed out")??,
        }
        Ok(())
    }

    async fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_exact(1).await?[0])
    }

    async fn read_exact(&mut self, length: usize) -> Result<Vec<u8>> {
        if self.stop.is_cancelled() {
            bail!("Device desktop session canceled");
        }
        if length > MAX_CLIENT_MESSAGE {
            bail!("RFB request exceeds its size limit");
        }
        while self.pending.len() < length {
            let message = tokio::select! { biased;
                _ = self.stop.cancelled() => bail!("Device desktop session canceled"),
                message = tokio::time::timeout(Duration::from_secs(120), self.socket.next()) => message.context("RFB read timed out")?.context("RFB WebSocket closed")??,
            };
            match message {
                Message::Binary(bytes) => {
                    if self.pending.len().saturating_add(bytes.len()) > MAX_CLIENT_MESSAGE {
                        bail!("buffered RFB input exceeds the safety limit");
                    }
                    self.pending.extend(bytes);
                }
                Message::Ping(bytes) => {
                    tokio::select! { biased;
                        _ = self.stop.cancelled() => bail!("Device desktop session canceled"),
                        result = tokio::time::timeout(Duration::from_secs(5), self.socket.send(Message::Pong(bytes))) => result.context("RFB pong timed out")??,
                    }
                }
                Message::Pong(_) => {}
                Message::Close(_) => bail!("RFB WebSocket closed"),
                Message::Text(_) | Message::Frame(_) => {
                    bail!("RFB transport accepts binary WebSocket frames only")
                }
            }
        }
        Ok(self.pending.drain(..length).collect())
    }
}

fn screen_dimensions() -> Result<(u16, u16)> {
    let display = Display::open()?;
    let api = api()?;
    let screen = unsafe { (api.default_screen)(display.0) };
    let width = unsafe { (api.display_width)(display.0, screen) };
    let height = unsafe { (api.display_height)(display.0, screen) };
    if width <= 0 || height <= 0 || width > 7680 || height > 4320 {
        bail!("Invalid local X11 framebuffer dimensions");
    }
    Ok((width as u16, height as u16))
}

fn server_init(width: u16, height: u16) -> Vec<u8> {
    let name = b"LocalAI Cowork V2 Device";
    let mut bytes = Vec::with_capacity(24 + name.len());
    bytes.extend(width.to_be_bytes());
    bytes.extend(height.to_be_bytes());
    bytes.extend([32, 24, 0, 1]);
    bytes.extend(255_u16.to_be_bytes());
    bytes.extend(255_u16.to_be_bytes());
    bytes.extend(255_u16.to_be_bytes());
    bytes.extend([16, 8, 0, 0, 0, 0]);
    bytes.extend((name.len() as u32).to_be_bytes());
    bytes.extend(name);
    bytes
}

struct Frame {
    width: u16,
    height: u16,
    bgra: Vec<u8>,
}

fn capture_screen() -> Result<Frame> {
    let display = Display::open()?;
    let api = api()?;
    let screen = unsafe { (api.default_screen)(display.0) };
    let width = unsafe { (api.display_width)(display.0, screen) };
    let height = unsafe { (api.display_height)(display.0, screen) };
    if width <= 0 || height <= 0 || width > 7680 || height > 4320 {
        bail!("Invalid local X11 framebuffer dimensions");
    }
    let image = unsafe {
        (api.get_image)(
            display.0,
            (api.root)(display.0),
            0,
            0,
            width as u32,
            height as u32,
            libc::c_ulong::MAX,
            2,
        )
    };
    if image.is_null() {
        bail!("X11 framebuffer capture failed");
    }
    let mut bgra = Vec::with_capacity(width as usize * height as usize * 4);
    let red = unsafe { (*image).red_mask };
    let green = unsafe { (*image).green_mask };
    let blue = unsafe { (*image).blue_mask };
    for y in 0..height {
        for x in 0..width {
            let pixel = unsafe { (api.get_pixel)(image, x, y) };
            let channel = |mask: libc::c_ulong| -> u8 {
                if mask == 0 {
                    0
                } else {
                    let shift = mask.trailing_zeros();
                    (((pixel & mask) >> shift) * 255 / (mask >> shift)) as u8
                }
            };
            bgra.extend([channel(blue), channel(green), channel(red), 0]);
        }
    }
    unsafe {
        (api.destroy_image)(image);
    }
    Ok(Frame {
        width: width as u16,
        height: height as u16,
        bgra,
    })
}

fn cropped_bgra(frame: &Frame, width: u16, height: u16) -> Result<Vec<u8>> {
    if width > frame.width || height > frame.height {
        bail!("requested framebuffer rectangle exceeds the captured desktop");
    }
    let row_bytes = width as usize * 4;
    let source_stride = frame.width as usize * 4;
    let mut output = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as usize {
        let start = row * source_stride;
        output.extend_from_slice(&frame.bgra[start..start + row_bytes]);
    }
    Ok(output)
}

fn rectangle_header(width: u16, height: u16, encoding: i32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16);
    bytes.extend([0, 0]);
    bytes.extend(1_u16.to_be_bytes());
    bytes.extend(0_u16.to_be_bytes());
    bytes.extend(0_u16.to_be_bytes());
    bytes.extend(width.to_be_bytes());
    bytes.extend(height.to_be_bytes());
    bytes.extend(encoding.to_be_bytes());
    bytes
}

fn raw_update(frame: &Frame, width: u16, height: u16) -> Result<Vec<u8>> {
    let mut bytes = rectangle_header(width, height, ENCODING_RAW);
    bytes.extend(cropped_bgra(frame, width, height)?);
    Ok(bytes)
}

fn tight_update(frame: &Frame, width: u16, height: u16) -> Result<Vec<u8>> {
    let bgra = cropped_bgra(frame, width, height)?;
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for pixel in bgra.chunks_exact(4) {
        rgb.extend([pixel[2], pixel[1], pixel[0]]);
    }
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 72).encode(
        &rgb,
        width as u32,
        height as u32,
        ColorType::Rgb8.into(),
    )?;
    let mut bytes = rectangle_header(width, height, ENCODING_TIGHT);
    bytes.push(0x90);
    encode_compact_length(jpeg.len(), &mut bytes)?;
    bytes.extend(jpeg);
    Ok(bytes)
}

fn server_cut_text(text: &str) -> Vec<u8> {
    let latin1: Vec<u8> = text
        .chars()
        .map(|character| u8::try_from(character as u32).unwrap_or(b'?'))
        .collect();
    let mut bytes = Vec::with_capacity(8 + latin1.len());
    bytes.extend([3, 0, 0, 0]);
    bytes.extend((latin1.len() as u32).to_be_bytes());
    bytes.extend(latin1);
    bytes
}

fn encode_compact_length(length: usize, output: &mut Vec<u8>) -> Result<()> {
    if length > 0x3f_ffff {
        bail!("Tight JPEG frame exceeds the RFB compact-length limit");
    }
    let mut value = length as u32;
    let mut first = (value & 0x7f) as u8;
    value >>= 7;
    if value != 0 {
        first |= 0x80;
    }
    output.push(first);
    if value != 0 {
        let mut second = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            second |= 0x80;
        }
        output.push(second);
        if value != 0 {
            output.push(value as u8);
        }
    }
    Ok(())
}

#[repr(C)]
#[allow(dead_code)]
struct XImage {
    width: libc::c_int,
    height: libc::c_int,
    xoffset: libc::c_int,
    format: libc::c_int,
    data: *mut libc::c_char,
    byte_order: libc::c_int,
    bitmap_unit: libc::c_int,
    bitmap_bit_order: libc::c_int,
    bitmap_pad: libc::c_int,
    depth: libc::c_int,
    bytes_per_line: libc::c_int,
    bits_per_pixel: libc::c_int,
    red_mask: libc::c_ulong,
    green_mask: libc::c_ulong,
    blue_mask: libc::c_ulong,
}
type D = *mut c_void;
struct Api {
    open: unsafe extern "C" fn(*const libc::c_char) -> D,
    close: unsafe extern "C" fn(D) -> libc::c_int,
    root: unsafe extern "C" fn(D) -> libc::c_ulong,
    default_screen: unsafe extern "C" fn(D) -> libc::c_int,
    display_width: unsafe extern "C" fn(D, libc::c_int) -> libc::c_int,
    display_height: unsafe extern "C" fn(D, libc::c_int) -> libc::c_int,
    get_image: unsafe extern "C" fn(
        D,
        libc::c_ulong,
        libc::c_int,
        libc::c_int,
        libc::c_uint,
        libc::c_uint,
        libc::c_ulong,
        libc::c_int,
    ) -> *mut XImage,
    get_pixel: unsafe extern "C" fn(*mut XImage, libc::c_int, libc::c_int) -> libc::c_ulong,
    destroy_image: unsafe extern "C" fn(*mut XImage) -> libc::c_int,
    keysym_to_keycode: unsafe extern "C" fn(D, libc::c_ulong) -> u8,
    flush: unsafe extern "C" fn(D) -> libc::c_int,
    test_query: unsafe extern "C" fn(
        D,
        *mut libc::c_int,
        *mut libc::c_int,
        *mut libc::c_int,
        *mut libc::c_int,
    ) -> libc::c_int,
    fake_key: unsafe extern "C" fn(D, libc::c_uint, libc::c_int, libc::c_ulong) -> libc::c_int,
    fake_button: unsafe extern "C" fn(D, libc::c_uint, libc::c_int, libc::c_ulong) -> libc::c_int,
    fake_motion: unsafe extern "C" fn(
        D,
        libc::c_int,
        libc::c_int,
        libc::c_int,
        libc::c_ulong,
    ) -> libc::c_int,
}
fn api() -> Result<&'static Api> {
    static NATIVE: OnceLock<Option<Api>> = OnceLock::new();
    let native = NATIVE.get_or_init(|| unsafe {
        let x11 = libc::dlopen(c"libX11.so.6".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        let xtst = libc::dlopen(c"libXtst.so.6".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if x11.is_null() || xtst.is_null() {
            return None;
        }
        unsafe fn symbol<T: Copy>(handle: D, name: &std::ffi::CStr) -> Option<T> {
            let value = unsafe { libc::dlsym(handle, name.as_ptr()) };
            if value.is_null() || std::mem::size_of::<T>() != std::mem::size_of::<D>() {
                None
            } else {
                Some(unsafe { std::mem::transmute_copy(&value) })
            }
        }
        let initialize: unsafe extern "C" fn() -> libc::c_int = symbol(x11, c"XInitThreads")?;
        if initialize() == 0 {
            return None;
        }
        Some(Api {
            open: symbol(x11, c"XOpenDisplay")?,
            close: symbol(x11, c"XCloseDisplay")?,
            root: symbol(x11, c"XDefaultRootWindow")?,
            default_screen: symbol(x11, c"XDefaultScreen")?,
            display_width: symbol(x11, c"XDisplayWidth")?,
            display_height: symbol(x11, c"XDisplayHeight")?,
            get_image: symbol(x11, c"XGetImage")?,
            get_pixel: symbol(x11, c"XGetPixel")?,
            destroy_image: symbol(x11, c"XDestroyImage")?,
            keysym_to_keycode: symbol(x11, c"XKeysymToKeycode")?,
            flush: symbol(x11, c"XFlush")?,
            test_query: symbol(xtst, c"XTestQueryExtension")?,
            fake_key: symbol(xtst, c"XTestFakeKeyEvent")?,
            fake_button: symbol(xtst, c"XTestFakeButtonEvent")?,
            fake_motion: symbol(xtst, c"XTestFakeMotionEvent")?,
        })
    });
    native
        .as_ref()
        .context("Native X11/XTest libraries are unavailable")
}
struct Display(D);
impl Display {
    fn open() -> Result<Self> {
        let value = std::env::var("DISPLAY").context("No local X11 DISPLAY")?;
        if !(value.starts_with(':') || value.starts_with("unix:"))
            || value.len() > 100
            || value.as_bytes().contains(&0)
        {
            bail!("Only a local Unix X11 display is supported");
        }
        if std::env::var("XDG_SESSION_TYPE").is_ok_and(|value| value != "x11") {
            bail!("Select an X11 desktop session; Wayland does not grant ambient capture/input");
        }
        let api = api()?;
        let name = std::ffi::CString::new(value)?;
        let display = unsafe { (api.open)(name.as_ptr()) };
        if display.is_null() {
            bail!("Cannot open the operator's local X11 desktop");
        }
        let display = Self(display);
        let (mut event, mut error, mut major, mut minor) = (0, 0, 0, 0);
        if unsafe { (api.test_query)(display.0, &mut event, &mut error, &mut major, &mut minor) }
            == 0
        {
            bail!("XTest input extension is unavailable");
        }
        Ok(display)
    }
}
impl Drop for Display {
    fn drop(&mut self) {
        if let Ok(api) = api() {
            unsafe {
                (api.close)(self.0);
            }
        }
    }
}
fn local_unlocked_session() -> bool {
    let Ok(session) = std::env::var("XDG_SESSION_ID") else {
        return false;
    };
    if session.is_empty()
        || session.len() > 100
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return false;
    }
    let mut command = Command::new("/usr/bin/loginctl");
    command.args([
        "show-session",
        &session,
        "--property=Active",
        "--property=LockedHint",
        "--property=User",
        "--property=Type",
    ]);
    bounded_output(command, Duration::from_millis(500), 4096).is_ok_and(|bytes| {
        let value = String::from_utf8_lossy(&bytes);
        value.lines().any(|line| line == "Active=yes")
            && value.lines().any(|line| line == "LockedHint=no")
            && value
                .lines()
                .any(|line| line == format!("User={}", unsafe { libc::geteuid() }))
            && value.lines().any(|line| {
                line == if uses_wayland() {
                    "Type=wayland"
                } else {
                    "Type=x11"
                }
            })
    })
}
fn bounded_output(mut command: Command, timeout: Duration, maximum: u64) -> Result<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("Local desktop helper refused the request");
            }
            let mut output = Vec::new();
            child
                .stdout
                .take()
                .context("Missing desktop helper output")?
                .take(maximum + 1)
                .read_to_end(&mut output)?;
            if output.len() as u64 > maximum {
                bail!("Local desktop helper output exceeded its limit");
            }
            return Ok(output);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Local desktop helper timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn inject_key(keysym: u32, down: bool) -> Result<()> {
    let display = Display::open()?;
    let api = api()?;
    let code = unsafe { (api.keysym_to_keycode)(display.0, keysym as libc::c_ulong) };
    if code == 0 {
        bail!("Remote keysym is not mapped in the local X11 keyboard layout");
    }
    if unsafe { (api.fake_key)(display.0, code as u32, i32::from(down), 0) } == 0 {
        bail!("XTest keyboard input failed");
    }
    unsafe {
        (api.flush)(display.0);
    }
    Ok(())
}
fn inject_button(button: u32, down: bool) -> Result<()> {
    let display = Display::open()?;
    let api = api()?;
    if unsafe { (api.fake_button)(display.0, button, i32::from(down), 0) } == 0 {
        bail!("XTest pointer input failed");
    }
    unsafe {
        (api.flush)(display.0);
    }
    Ok(())
}
fn inject_pointer(
    x: u16,
    y: u16,
    buttons: u8,
    previous: u8,
    width: u16,
    height: u16,
) -> Result<()> {
    let display = Display::open()?;
    let api = api()?;
    if unsafe {
        (api.fake_motion)(
            display.0,
            -1,
            x.min(width - 1) as i32,
            y.min(height - 1) as i32,
            0,
        )
    } == 0
    {
        bail!("XTest pointer movement failed");
    }
    for (mask, button) in [(1, 1), (2, 2), (4, 3)] {
        if (buttons & mask) != (previous & mask)
            && unsafe { (api.fake_button)(display.0, button, i32::from(buttons & mask != 0), 0) }
                == 0
        {
            bail!("XTest pointer input failed");
        }
    }
    for (mask, button) in [(8, 4), (16, 5), (32, 6), (64, 7)] {
        if buttons & mask != 0 && previous & mask == 0 {
            unsafe {
                (api.fake_button)(display.0, button, 1, 0);
                (api.fake_button)(display.0, button, 0, 0);
            }
        }
    }
    unsafe {
        (api.flush)(display.0);
    }
    Ok(())
}
fn stop_clipboard(owned: &mut Option<Child>) {
    if let Some(mut child) = owned.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}
fn set_clipboard(text: &str) -> Result<Child> {
    if text.len() > MAX_CLIPBOARD_BYTES {
        bail!("Clipboard text exceeds its size limit");
    }
    if !std::path::Path::new("/usr/bin/xclip").is_file() {
        bail!("X11 clipboard requires the operator-installed xclip program");
    }
    let mut child = Command::new("/usr/bin/xclip")
        .args(["-selection", "clipboard", "-in", "-quiet", "-loops", "1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Err(error) = child
        .stdin
        .take()
        .context("Missing local clipboard input")?
        .write_all(text.as_bytes())
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    Ok(child)
}
fn read_clipboard() -> Result<Option<String>> {
    if !std::path::Path::new("/usr/bin/xclip").is_file() {
        return Ok(None);
    }
    let mut command = Command::new("/usr/bin/xclip");
    command.args(["-selection", "clipboard", "-out", "-quiet"]);
    match bounded_output(
        command,
        Duration::from_millis(250),
        MAX_CLIPBOARD_BYTES as u64,
    ) {
        Ok(bytes) => Ok(Some(String::from_utf8(bytes)?)),
        Err(_) => Ok(None),
    }
}
