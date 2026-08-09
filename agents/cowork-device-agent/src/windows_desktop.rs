use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

#[cfg(not(windows))]
pub async fn serve<S>(_socket: WebSocketStream<S>, _control: bool) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    bail!("Windows desktop streaming is only available on Windows executors")
}

#[cfg(not(windows))]
pub async fn confirm_personal_session(
    _run_id: uuid::Uuid,
    _session_id: uuid::Uuid,
    _control: bool,
) -> Result<bool> {
    bail!("personal desktop confirmation requires an interactive Windows session")
}

#[cfg(windows)]
pub async fn serve<S>(socket: WebSocketStream<S>, control: bool) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    windows::serve(socket, control).await
}

#[cfg(windows)]
pub async fn confirm_personal_session(
    run_id: uuid::Uuid,
    session_id: uuid::Uuid,
    control: bool,
) -> Result<bool> {
    tokio::task::spawn_blocking(move || {
        windows::confirm_personal_session(run_id, session_id, control)
    })
    .await
    .context("personal desktop confirmation worker failed")?
}

#[cfg(windows)]
mod windows {
    use std::{collections::VecDeque, ffi::c_void, mem::size_of, ptr};

    use image::{codecs::jpeg::JpegEncoder, ColorType};
    use windows_sys::Win32::{
        Foundation::{GlobalFree, HGLOBAL},
        Graphics::Gdi::{
            BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
            SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
            SRCCOPY,
        },
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
                OpenClipboard, SetClipboardData,
            },
            Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE},
        },
        UI::{
            Input::KeyboardAndMouse::{
                mouse_event, SendInput, VkKeyScanW, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
                KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
                MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN,
                MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
            },
            WindowsAndMessaging::{
                GetSystemMetrics, MessageBoxW, SetCursorPos, IDYES, MB_ICONWARNING,
                MB_SETFOREGROUND, MB_YESNO, SM_CXSCREEN, SM_CYSCREEN,
            },
        },
    };

    use super::*;

    const RFB_VERSION: &[u8; 12] = b"RFB 003.008\n";
    const ENCODING_RAW: i32 = 0;
    const ENCODING_TIGHT: i32 = 7;
    const MAX_CLIENT_MESSAGE: usize = 16 * 1024 * 1024;
    const CF_UNICODETEXT: u32 = 13;
    const WHEEL_DELTA: i32 = 120;

    pub fn confirm_personal_session(
        run_id: uuid::Uuid,
        session_id: uuid::Uuid,
        control: bool,
    ) -> Result<bool> {
        let access = if control {
            "Bildschirm anzeigen und Maus, Tastatur sowie Zwischenablage steuern"
        } else {
            "Bildschirm anzeigen"
        };
        let message = format!(
            "Open Cowork moechte fuer einen Remote-Run deinen {access}.\n\nRun: {run_id}\nSitzung: {session_id}\n\nZugriff fuer diese Sitzung erlauben?"
        );
        let title = wide("Open Cowork - lokale Bestaetigung");
        let message = wide(&message);
        let result = unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_YESNO | MB_ICONWARNING | MB_SETFOREGROUND,
            )
        };
        Ok(result == IDYES)
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub async fn serve<S>(socket: WebSocketStream<S>, control: bool) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let (width, height) = screen_dimensions()?;
        let mut stream = WsBytes::new(socket);
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
                    let _pixel_format = stream.read_exact(19).await?;
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
                        if i32::from_be_bytes(bytes.try_into().expect("four bytes"))
                            == ENCODING_TIGHT
                        {
                            supports_tight = true;
                        }
                    }
                }
                3 => {
                    let request = stream.read_exact(9).await?;
                    let requested_width = u16::from_be_bytes([request[5], request[6]]);
                    let requested_height = u16::from_be_bytes([request[7], request[8]]);
                    let frame = tokio::task::spawn_blocking(capture_screen)
                        .await
                        .context("Windows screen capture worker failed")??;
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
                        if let Ok(Some(text)) = tokio::task::spawn_blocking(read_clipboard)
                            .await
                            .context("Windows clipboard read worker failed")?
                        {
                            if last_remote_clipboard.as_deref() != Some(&text) {
                                stream.send(server_cut_text(&text)).await?;
                                last_remote_clipboard = Some(text);
                            }
                        }
                    }
                }
                4 => {
                    let event = stream.read_exact(7).await?;
                    if control {
                        let down = event[0] != 0;
                        let keysym =
                            u32::from_be_bytes(event[3..7].try_into().expect("four bytes"));
                        tokio::task::spawn_blocking(move || inject_key(keysym, down))
                            .await
                            .context("Windows key input worker failed")??;
                    }
                }
                5 => {
                    let event = stream.read_exact(5).await?;
                    if control {
                        let buttons = event[0];
                        let x = u16::from_be_bytes([event[1], event[2]]);
                        let y = u16::from_be_bytes([event[3], event[4]]);
                        let old_buttons = previous_buttons;
                        tokio::task::spawn_blocking(move || {
                            inject_pointer(x, y, buttons, old_buttons, width, height)
                        })
                        .await
                        .context("Windows pointer input worker failed")??;
                        previous_buttons = buttons & 0x07;
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
                    if length > MAX_CLIENT_MESSAGE {
                        bail!("RFB clipboard message exceeds the safety limit");
                    }
                    let text = stream.read_exact(length).await?;
                    if control {
                        let text: String = text.into_iter().map(char::from).collect();
                        tokio::task::spawn_blocking(move || set_clipboard(&text))
                            .await
                            .context("Windows clipboard worker failed")??;
                    }
                }
                message_type => bail!("unsupported RFB client message type {message_type}"),
            }
        }
    }

    struct WsBytes<S> {
        socket: WebSocketStream<S>,
        pending: VecDeque<u8>,
    }

    impl<S> WsBytes<S>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        fn new(socket: WebSocketStream<S>) -> Self {
            Self {
                socket,
                pending: VecDeque::new(),
            }
        }

        async fn send(&mut self, bytes: Vec<u8>) -> Result<()> {
            self.socket.send(Message::Binary(bytes.into())).await?;
            Ok(())
        }

        async fn read_u8(&mut self) -> Result<u8> {
            Ok(self.read_exact(1).await?[0])
        }

        async fn read_exact(&mut self, length: usize) -> Result<Vec<u8>> {
            while self.pending.len() < length {
                let message = self.socket.next().await.context("RFB WebSocket closed")??;
                match message {
                    Message::Binary(bytes) => {
                        if self.pending.len().saturating_add(bytes.len()) > MAX_CLIENT_MESSAGE {
                            bail!("buffered RFB input exceeds the safety limit");
                        }
                        self.pending.extend(bytes);
                    }
                    Message::Ping(bytes) => self.socket.send(Message::Pong(bytes)).await?,
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
        let (width, height) =
            unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        if width <= 0 || height <= 0 || width > u16::MAX as i32 || height > u16::MAX as i32 {
            bail!("the interactive Windows desktop has invalid dimensions");
        }
        Ok((width as u16, height as u16))
    }

    fn server_init(width: u16, height: u16) -> Vec<u8> {
        let name = b"Open Cowork Windows Executor";
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
                0,
                0,
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
        let x = i32::from(x.min(width.saturating_sub(1)));
        let y = i32::from(y.min(height.saturating_sub(1)));
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
            Ok(Some(text))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn compact_lengths_follow_tight_encoding() {
            let cases = [
                (0, vec![0]),
                (127, vec![127]),
                (128, vec![128, 1]),
                (16_383, vec![255, 127]),
                (16_384, vec![128, 128, 1]),
            ];
            for (length, expected) in cases {
                let mut actual = Vec::new();
                encode_compact_length(length, &mut actual).unwrap();
                assert_eq!(actual, expected);
            }
        }

        #[test]
        fn server_init_has_expected_geometry_and_name() {
            let init = server_init(1440, 900);
            assert_eq!(&init[0..2], &1440_u16.to_be_bytes());
            assert_eq!(&init[2..4], &900_u16.to_be_bytes());
            assert_eq!(&init[24..], b"Open Cowork Windows Executor");
        }

        #[test]
        fn server_clipboard_uses_standard_rfb_latin1() {
            let message = server_cut_text("Grüße €");
            assert_eq!(message[0], 3);
            assert_eq!(u32::from_be_bytes(message[4..8].try_into().unwrap()), 7);
            assert_eq!(&message[8..], b"Gr\xfc\xdfe ?");
        }

        #[tokio::test]
        #[ignore = "requires an interactive Windows desktop"]
        async fn interactive_rfb_viewer_receives_a_tight_jpeg_frame() {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let websocket = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let _ = serve(websocket, false).await;
            });
            let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
                .await
                .unwrap();

            assert_eq!(binary(client.next().await.unwrap().unwrap()), RFB_VERSION);
            client
                .send(Message::Binary(RFB_VERSION.to_vec().into()))
                .await
                .unwrap();
            assert_eq!(binary(client.next().await.unwrap().unwrap()), &[1, 1]);
            client.send(Message::Binary(vec![1].into())).await.unwrap();
            assert_eq!(binary(client.next().await.unwrap().unwrap()), &[0, 0, 0, 0]);
            client.send(Message::Binary(vec![1].into())).await.unwrap();
            let init = binary(client.next().await.unwrap().unwrap()).to_vec();
            assert!(init.len() >= 24);

            let mut encodings = vec![2, 0, 0, 1];
            encodings.extend(ENCODING_TIGHT.to_be_bytes());
            client
                .send(Message::Binary(encodings.into()))
                .await
                .unwrap();
            let mut request = vec![3, 0, 0, 0, 0, 0];
            request.extend(320_u16.to_be_bytes());
            request.extend(200_u16.to_be_bytes());
            client.send(Message::Binary(request.into())).await.unwrap();

            let update = binary(client.next().await.unwrap().unwrap()).to_vec();
            assert!(update.len() > 20);
            assert_eq!(
                i32::from_be_bytes(update[12..16].try_into().unwrap()),
                ENCODING_TIGHT
            );
            assert_eq!(update[16], 0x90);
            let (jpeg_length, compact_bytes) = decode_compact_length(&update[17..]);
            let jpeg = &update[17 + compact_bytes..];
            assert_eq!(jpeg.len(), jpeg_length);
            assert!(jpeg.starts_with(&[0xff, 0xd8]));
            assert!(jpeg.ends_with(&[0xff, 0xd9]));
            let _ = client.close(None).await;
            server.abort();
        }

        fn binary(message: Message) -> Vec<u8> {
            match message {
                Message::Binary(bytes) => bytes.to_vec(),
                other => panic!("expected a binary WebSocket message, got {other:?}"),
            }
        }

        fn decode_compact_length(bytes: &[u8]) -> (usize, usize) {
            let mut value = usize::from(bytes[0] & 0x7f);
            if bytes[0] & 0x80 == 0 {
                return (value, 1);
            }
            value |= usize::from(bytes[1] & 0x7f) << 7;
            if bytes[1] & 0x80 == 0 {
                return (value, 2);
            }
            (value | (usize::from(bytes[2]) << 14), 3)
        }
    }
}
