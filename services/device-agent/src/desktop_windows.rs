//! Native Windows RFB server adapted from Open Cowork's cowork-device-agent,
//! agents/cowork-device-agent/src/windows_desktop.rs (Apache-2.0).
//! V2 uses its local consent queue and run cancellation; there is no V1 IPC,
//! external VNC endpoint, blocking confirmation dialog or detached input task.
use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::{BTreeSet, VecDeque},
    ffi::c_void,
    mem::size_of,
    ptr,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};
use tokio_util::sync::CancellationToken;

use image::{ColorType, codecs::jpeg::JpegEncoder};
use windows_sys::Win32::{
    Foundation::{GlobalFree, HGLOBAL},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC,
        CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC, SRCCOPY,
        SelectObject,
    },
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
            OpenClipboard, SetClipboardData,
        },
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        RemoteDesktop::ProcessIdToSessionId,
        StationsAndDesktops::{
            CloseDesktop, DESKTOP_READOBJECTS, GetUserObjectInformationW, OpenInputDesktop,
            UOI_NAME,
        },
        Threading::GetCurrentProcessId,
    },
    UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
            MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
            MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, SendInput, VkKeyScanW,
            mouse_event,
        },
        WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN, SetCursorPos,
        },
    },
};

const RFB_VERSION: &[u8; 12] = b"RFB 003.008\n";
const ENCODING_RAW: i32 = 0;
const ENCODING_TIGHT: i32 = 7;
const MAX_CLIENT_MESSAGE: usize = 16 * 1024 * 1024;
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
const CF_UNICODETEXT: u32 = 13;
const WHEEL_DELTA: i32 = 120;

pub fn available() -> bool {
    // Capture and input use physical coordinates across the virtual monitor desktop.
    static DPI: std::sync::Once = std::sync::Once::new();
    DPI.call_once(|| unsafe {
        SetProcessDpiAwarenessContext(-4);
    });
    unsafe {
        let mut session = 0;
        if ProcessIdToSessionId(GetCurrentProcessId(), &mut session) == 0 || session == 0 {
            return false;
        }
        let desktop = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if desktop.is_null() {
            return false;
        }
        let mut name = [0u16; 128];
        let mut required = 0;
        let ok = GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            (name.len() * size_of::<u16>()) as u32,
            &mut required,
        );
        CloseDesktop(desktop);
        ok != 0
            && String::from_utf16_lossy(
                &name[..name.iter().position(|v| *v == 0).unwrap_or(name.len())],
            )
            .eq_ignore_ascii_case("Default")
            && screen_dimensions().is_ok()
    }
}
#[link(name = "user32")]
unsafe extern "system" {
    fn SetProcessDpiAwarenessContext(context: isize) -> i32;
}

fn check_access(stop: &CancellationToken) -> Result<()> {
    if stop.is_cancelled() {
        bail!("Device desktop session canceled");
    }
    if !available() {
        bail!("An unlocked interactive Windows desktop is required");
    }
    Ok(())
}

#[derive(Default)]
struct PressedInputs {
    keys: BTreeSet<u32>,
    buttons: u8,
}
impl Drop for PressedInputs {
    fn drop(&mut self) {
        // Release only input held by this stream, including when its future is
        // canceled. There are no detached input workers after returning.
        for key in &self.keys {
            let _ = inject_key(*key, false);
        }
        unsafe {
            for (mask, flag) in [
                (1, MOUSEEVENTF_LEFTUP),
                (2, MOUSEEVENTF_MIDDLEUP),
                (4, MOUSEEVENTF_RIGHTUP),
            ] {
                if self.buttons & mask != 0 {
                    mouse_event(flag, 0, 0, 0, 0);
                }
            }
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
    let (width, height) = screen_dimensions()?;
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
    let mut pressed = PressedInputs::default();
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
                let frame = capture_screen()?;
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
                    if let Ok(Some(text)) = read_clipboard()
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
                    inject_key(keysym, down)?;
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
                    inject_pointer(x, y, buttons, old_buttons, width, height)?;
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
                    set_clipboard(&text)?;
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
    let (width, height) = unsafe {
        (
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if width <= 0 || height <= 0 || width > 7680 || height > 4320 {
        bail!("the interactive Windows desktop has invalid dimensions");
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
    let (width, height) = screen_dimensions()?;
    let byte_len = width as usize * height as usize * 4;
    unsafe {
        let screen_dc = GetDC(ptr::null_mut());
        if screen_dc.is_null() {
            bail!("GetDC failed while capturing the interactive desktop");
        }
        let memory_dc = CreateCompatibleDC(screen_dc);
        if memory_dc.is_null() {
            ReleaseDC(ptr::null_mut(), screen_dc);
            bail!("CreateCompatibleDC failed while capturing the desktop");
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: byte_len as u32,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut bits: *mut c_void = ptr::null_mut();
        let bitmap = CreateDIBSection(
            screen_dc,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            ptr::null_mut(),
            0,
        );
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(memory_dc);
            ReleaseDC(ptr::null_mut(), screen_dc);
            bail!("CreateDIBSection failed while capturing the desktop");
        }
        let old = SelectObject(memory_dc, bitmap);
        let copied = BitBlt(
            memory_dc,
            0,
            0,
            width as i32,
            height as i32,
            screen_dc,
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            SRCCOPY | CAPTUREBLT,
        );
        let bgra = if copied != 0 {
            std::slice::from_raw_parts(bits.cast::<u8>(), byte_len).to_vec()
        } else {
            Vec::new()
        };
        SelectObject(memory_dc, old);
        DeleteObject(bitmap);
        DeleteDC(memory_dc);
        ReleaseDC(ptr::null_mut(), screen_dc);
        if copied == 0 {
            bail!("BitBlt failed while capturing the desktop");
        }
        Ok(Frame {
            width,
            height,
            bgra,
        })
    }
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

fn inject_key(keysym: u32, down: bool) -> Result<()> {
    let virtual_key = keysym_to_virtual_key(keysym);
    let (w_vk, w_scan, mut flags) = if let Some(key) = virtual_key {
        (key, 0, 0)
    } else if let Some(character) = char::from_u32(keysym) {
        let units: Vec<u16> = character.encode_utf16(&mut [0; 2]).to_vec();
        for unit in units {
            send_key_input(
                0,
                unit,
                KEYEVENTF_UNICODE | (!down as u32 * KEYEVENTF_KEYUP),
            )?;
        }
        return Ok(());
    } else {
        bail!("unsupported RFB keysym {keysym:#x}");
    };
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    send_key_input(w_vk, w_scan, flags)
}

fn send_key_input(w_vk: u16, w_scan: u16, flags: u32) -> Result<()> {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: w_vk,
                wScan: w_scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    if unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) } != 1 {
        bail!("SendInput rejected a keyboard event");
    }
    Ok(())
}

fn keysym_to_virtual_key(keysym: u32) -> Option<u16> {
    let special = match keysym {
        0xff08 => 0x08,
        0xff09 => 0x09,
        0xff0d => 0x0d,
        0xff1b => 0x1b,
        0xff50 => 0x24,
        0xff51 => 0x25,
        0xff52 => 0x26,
        0xff53 => 0x27,
        0xff54 => 0x28,
        0xff55 => 0x21,
        0xff56 => 0x22,
        0xff57 => 0x23,
        0xff63 => 0x2d,
        0xffff => 0x2e,
        0xffe1 | 0xffe2 => 0x10,
        0xffe3 | 0xffe4 => 0x11,
        0xffe9 | 0xffea => 0x12,
        0xffeb | 0xffec => 0x5b,
        0xffbe..=0xffd5 => 0x70 + (keysym - 0xffbe) as u16,
        _ => 0,
    };
    if special != 0 {
        return Some(special);
    }
    if keysym <= 0x7f {
        let mapped = unsafe { VkKeyScanW(keysym as u16) };
        if mapped != -1 {
            return Some((mapped as u16) & 0xff);
        }
    }
    None
}

fn inject_pointer(
    x: u16,
    y: u16,
    buttons: u8,
    previous: u8,
    width: u16,
    height: u16,
) -> Result<()> {
    let x =
        i32::from(x.min(width.saturating_sub(1))) + unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let y =
        i32::from(y.min(height.saturating_sub(1))) + unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    if unsafe { SetCursorPos(x, y) } == 0 {
        bail!("SetCursorPos rejected a pointer event");
    }
    for (mask, down_flag, up_flag) in [
        (1_u8, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        (2_u8, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        (4_u8, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
    ] {
        if buttons & mask != previous & mask {
            let flag = if buttons & mask != 0 {
                down_flag
            } else {
                up_flag
            };
            unsafe { mouse_event(flag, 0, 0, 0, 0) };
        }
    }
    if buttons & 0x08 != 0 {
        unsafe { mouse_event(MOUSEEVENTF_WHEEL, 0, 0, WHEEL_DELTA, 0) };
    }
    if buttons & 0x10 != 0 {
        unsafe { mouse_event(MOUSEEVENTF_WHEEL, 0, 0, -WHEEL_DELTA, 0) };
    }
    Ok(())
}

fn set_clipboard(text: &str) -> Result<()> {
    if text.len() > MAX_CLIPBOARD_BYTES {
        bail!("Clipboard text exceeds its size limit");
    }
    let mut utf16: Vec<u16> = text.encode_utf16().collect();
    utf16.push(0);
    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            bail!("OpenClipboard failed");
        }
        if EmptyClipboard() == 0 {
            CloseClipboard();
            bail!("EmptyClipboard failed");
        }
        let allocation: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, utf16.len() * size_of::<u16>());
        if allocation.is_null() {
            CloseClipboard();
            bail!("GlobalAlloc failed for clipboard text");
        }
        let destination = GlobalLock(allocation).cast::<u16>();
        if destination.is_null() {
            GlobalFree(allocation);
            CloseClipboard();
            bail!("GlobalLock failed for clipboard text");
        }
        ptr::copy_nonoverlapping(utf16.as_ptr(), destination, utf16.len());
        GlobalUnlock(allocation);
        if SetClipboardData(CF_UNICODETEXT, allocation).is_null() {
            GlobalFree(allocation);
            CloseClipboard();
            bail!("SetClipboardData failed");
        }
        CloseClipboard();
    }
    Ok(())
}

fn read_clipboard() -> Result<Option<String>> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
            return Ok(None);
        }
        if OpenClipboard(ptr::null_mut()) == 0 {
            bail!("OpenClipboard failed");
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            CloseClipboard();
            bail!("GetClipboardData failed");
        }
        let bytes = GlobalSize(handle);
        if bytes > MAX_CLIPBOARD_BYTES * 2 {
            CloseClipboard();
            bail!("Clipboard text exceeds its size limit");
        }
        let source = GlobalLock(handle).cast::<u16>();
        if source.is_null() || bytes < size_of::<u16>() {
            if !source.is_null() {
                GlobalUnlock(handle);
            }
            CloseClipboard();
            bail!("GlobalLock failed for clipboard text");
        }
        let units = std::slice::from_raw_parts(source, bytes / size_of::<u16>());
        let length = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        let text = String::from_utf16_lossy(&units[..length]);
        GlobalUnlock(handle);
        CloseClipboard();
        if text.len() > MAX_CLIPBOARD_BYTES {
            bail!("Clipboard text exceeds its size limit");
        }
        Ok(Some(text))
    }
}
