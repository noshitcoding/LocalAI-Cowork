use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

#[cfg(not(any(windows, target_os = "linux")))]
pub async fn serve<S>(_socket: WebSocketStream<S>, _control: bool) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    bail!("desktop streaming is only available on Windows and Linux executors")
}

#[cfg(not(any(windows, target_os = "linux")))]
pub async fn confirm_personal_session(
    _run_id: uuid::Uuid,
    _session_id: uuid::Uuid,
    _control: bool,
) -> Result<bool> {
    bail!("personal desktop confirmation requires an interactive Windows or Linux session")
}

pub fn available() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(target_os = "linux")]
    {
        linux::available()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
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

#[cfg(target_os = "linux")]
pub async fn serve<S>(socket: WebSocketStream<S>, control: bool) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    linux::serve(socket, control).await
}

#[cfg(target_os = "linux")]
pub async fn confirm_personal_session(
    run_id: uuid::Uuid,
    session_id: uuid::Uuid,
    control: bool,
) -> Result<bool> {
    linux::confirm_personal_session(run_id, session_id, control).await
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{collections::VecDeque, env, process::Stdio, time::Duration};

    use image::{codecs::jpeg::JpegEncoder, ColorType};
    use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

    use super::*;

    const RFB_VERSION: &[u8; 12] = b"RFB 003.008\n";
    const ENCODING_RAW: i32 = 0;
    const ENCODING_TIGHT: i32 = 7;
    const MAX_CLIENT_MESSAGE: usize = 16 * 1024 * 1024;
    const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

    pub fn available() -> bool {
        env::var_os("DISPLAY").is_some()
            && command_available("xdotool")
            && command_available("import")
            && (command_available("xclip") || command_available("xsel"))
    }

    pub async fn confirm_personal_session(
        run_id: uuid::Uuid,
        session_id: uuid::Uuid,
        control: bool,
    ) -> Result<bool> {
        if env::var_os("DISPLAY").is_none() {
            bail!("personal Linux desktop confirmation requires DISPLAY");
        }
        let access = if control {
            "view your screen and control mouse, keyboard, and clipboard"
        } else {
            "view your screen"
        };
        let message = format!(
            "Open Cowork wants to {access}.\n\nRun: {run_id}\nSession: {session_id}\n\nAllow access for this session?"
        );
        let mut command = if command_available("zenity") {
            let mut command = Command::new("zenity");
            command.args([
                "--question",
                "--title=Open Cowork - local confirmation",
                "--no-wrap",
                &format!("--text={message}"),
            ]);
            command
        } else if command_available("kdialog") {
            let mut command = Command::new("kdialog");
            command.args([
                "--title",
                "Open Cowork - local confirmation",
                "--yesno",
                &message,
            ]);
            command
        } else if command_available("xmessage") {
            let mut command = Command::new("xmessage");
            command.args([
                "-center",
                "-title",
                "Open Cowork - local confirmation",
                "-buttons",
                "Allow:0,Deny:1",
                "-default",
                "Deny",
                &message,
            ]);
            command
        } else {
            bail!(
                "install zenity, kdialog, or xmessage for per-session Linux desktop confirmation"
            );
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let status = timeout(Duration::from_secs(5 * 60), command.status())
            .await
            .context("local Linux desktop confirmation timed out")??;
        Ok(status.success())
    }

    pub async fn serve<S>(socket: WebSocketStream<S>, control: bool) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if !available() {
            bail!("Linux desktop streaming requires DISPLAY, xdotool, ImageMagick import, and xclip or xsel");
        }
        let (width, height) = screen_dimensions().await?;
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
        stream
            .send(server_init(width, height, "Open Cowork Linux Device"))
            .await?;

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
                    let frame = capture_screen().await?;
                    let update_width = width.min(frame.width).min(requested_width.max(1));
                    let update_height = height.min(frame.height).min(requested_height.max(1));
                    let update = if supports_tight {
                        tight_update(&frame, update_width, update_height)?
                    } else {
                        raw_update(&frame, update_width, update_height)?
                    };
                    stream.send(update).await?;
                    if control {
                        if let Some(text) = read_clipboard().await? {
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
                            u32::from_be_bytes(event[3..7].try_into().expect("four-byte keysym"));
                        inject_key(keysym, down).await?;
                    }
                }
                5 => {
                    let event = stream.read_exact(5).await?;
                    if control {
                        let buttons = event[0];
                        let x = u16::from_be_bytes([event[1], event[2]]);
                        let y = u16::from_be_bytes([event[3], event[4]]);
                        inject_pointer(x, y, buttons, previous_buttons, width, height).await?;
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
                    if length > MAX_CLIPBOARD_BYTES {
                        bail!("RFB clipboard message exceeds the Linux clipboard safety limit");
                    }
                    let text = stream.read_exact(length).await?;
                    if control {
                        let text: String = text.into_iter().map(char::from).collect();
                        set_clipboard(&text).await?;
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

    struct Frame {
        width: u16,
        height: u16,
        bgra: Vec<u8>,
    }

    async fn screen_dimensions() -> Result<(u16, u16)> {
        let output = run_output("xdotool", &["getdisplaygeometry"]).await?;
        let mut values = output.split_whitespace();
        let width = values
            .next()
            .context("xdotool did not report a display width")?
            .parse::<u32>()?;
        let height = values
            .next()
            .context("xdotool did not report a display height")?
            .parse::<u32>()?;
        if width == 0 || height == 0 || width > u16::MAX as u32 || height > u16::MAX as u32 {
            bail!("the interactive Linux desktop has invalid dimensions");
        }
        Ok((width as u16, height as u16))
    }

    async fn capture_screen() -> Result<Frame> {
        let output = timeout(
            Duration::from_secs(15),
            Command::new("import")
                .args(["-silent", "-window", "root", "png:-"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("Linux desktop capture timed out")??;
        if !output.status.success() {
            bail!(
                "ImageMagick import failed: {}",
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(2_000)
                    .collect::<String>()
            );
        }
        if output.stdout.len() > 128 * 1024 * 1024 {
            bail!("captured Linux desktop image exceeds the safety limit");
        }
        let rgba = image::load_from_memory(&output.stdout)
            .context("ImageMagick returned an invalid desktop image")?
            .to_rgba8();
        let (width, height) = rgba.dimensions();
        if width == 0 || height == 0 || width > u16::MAX as u32 || height > u16::MAX as u32 {
            bail!("captured Linux desktop has invalid dimensions");
        }
        let mut bgra = rgba.into_raw();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 0;
        }
        Ok(Frame {
            width: width as u16,
            height: height as u16,
            bgra,
        })
    }

    fn server_init(width: u16, height: u16, name: &str) -> Vec<u8> {
        let name = name.as_bytes();
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

    fn server_cut_text(text: &str) -> Vec<u8> {
        let latin1: Vec<u8> = text
            .chars()
            .map(|character| u8::try_from(character as u32).unwrap_or(b'?'))
            .take(MAX_CLIPBOARD_BYTES)
            .collect();
        let mut bytes = Vec::with_capacity(8 + latin1.len());
        bytes.extend([3, 0, 0, 0]);
        bytes.extend((latin1.len() as u32).to_be_bytes());
        bytes.extend(latin1);
        bytes
    }

    async fn inject_key(keysym: u32, down: bool) -> Result<()> {
        let key = keysym_name(keysym)?;
        let action = if down { "keydown" } else { "keyup" };
        run_output("xdotool", &[action, "--clearmodifiers", &key]).await?;
        Ok(())
    }

    fn keysym_name(keysym: u32) -> Result<String> {
        let special = match keysym {
            0x0020 => Some("space"),
            0xff08 => Some("BackSpace"),
            0xff09 => Some("Tab"),
            0xff0d => Some("Return"),
            0xff1b => Some("Escape"),
            0xff50 => Some("Home"),
            0xff51 => Some("Left"),
            0xff52 => Some("Up"),
            0xff53 => Some("Right"),
            0xff54 => Some("Down"),
            0xff55 => Some("Page_Up"),
            0xff56 => Some("Page_Down"),
            0xff57 => Some("End"),
            0xff63 => Some("Insert"),
            0xffff => Some("Delete"),
            0xffe1 => Some("Shift_L"),
            0xffe2 => Some("Shift_R"),
            0xffe3 => Some("Control_L"),
            0xffe4 => Some("Control_R"),
            0xffe9 => Some("Alt_L"),
            0xffea => Some("Alt_R"),
            0xffeb => Some("Super_L"),
            0xffec => Some("Super_R"),
            _ => None,
        };
        if let Some(key) = special {
            return Ok(key.to_owned());
        }
        if (0xffbe..=0xffd5).contains(&keysym) {
            return Ok(format!("F{}", keysym - 0xffbd));
        }
        let character = if keysym & 0xff00_0000 == 0x0100_0000 {
            char::from_u32(keysym & 0x00ff_ffff)
        } else {
            char::from_u32(keysym)
        }
        .filter(|character| !character.is_control())
        .context("unsupported Linux RFB keysym")?;
        Ok(character.to_string())
    }

    async fn inject_pointer(
        x: u16,
        y: u16,
        buttons: u8,
        previous_buttons: u8,
        width: u16,
        height: u16,
    ) -> Result<()> {
        let x = x.min(width.saturating_sub(1));
        let y = y.min(height.saturating_sub(1));
        run_output("xdotool", &["mousemove", &x.to_string(), &y.to_string()]).await?;
        for (mask, button) in [(0x01, "1"), (0x02, "2"), (0x04, "3")] {
            let was_down = previous_buttons & mask != 0;
            let is_down = buttons & mask != 0;
            if was_down != is_down {
                run_output(
                    "xdotool",
                    &[if is_down { "mousedown" } else { "mouseup" }, button],
                )
                .await?;
            }
        }
        for (mask, button) in [(0x08, "4"), (0x10, "5"), (0x20, "6"), (0x40, "7")] {
            if buttons & mask != 0 {
                run_output("xdotool", &["click", button]).await?;
            }
        }
        Ok(())
    }

    async fn set_clipboard(text: &str) -> Result<()> {
        if text.len() > MAX_CLIPBOARD_BYTES {
            bail!("Linux clipboard text exceeds the safety limit");
        }
        let mut command = if command_available("xsel") {
            let mut command = Command::new("xsel");
            command.args(["--clipboard", "--input"]);
            command
        } else {
            let mut command = Command::new("xclip");
            command.args(["-selection", "clipboard", "-in"]);
            command
        };
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("failed to start Linux clipboard writer")?;
        child
            .stdin
            .take()
            .context("Linux clipboard writer has no stdin")?
            .write_all(text.as_bytes())
            .await?;
        let status = timeout(Duration::from_secs(3), child.wait())
            .await
            .context("Linux clipboard writer timed out")??;
        if !status.success() {
            bail!("Linux clipboard writer failed");
        }
        Ok(())
    }

    async fn read_clipboard() -> Result<Option<String>> {
        let (program, arguments): (&str, &[&str]) = if command_available("xsel") {
            ("xsel", &["--clipboard", "--output"])
        } else {
            ("xclip", &["-selection", "clipboard", "-out"])
        };
        let output = timeout(
            Duration::from_secs(2),
            Command::new(program)
                .args(arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("Linux clipboard reader timed out")??;
        if !output.status.success() || output.stdout.is_empty() {
            return Ok(None);
        }
        if output.stdout.len() > MAX_CLIPBOARD_BYTES {
            bail!("Linux clipboard contents exceed the safety limit");
        }
        Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()))
    }

    fn command_available(program: &str) -> bool {
        std::process::Command::new("which")
            .arg(program)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    async fn run_output(program: &str, arguments: &[&str]) -> Result<String> {
        let output = timeout(
            Duration::from_secs(15),
            Command::new(program)
                .args(arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .with_context(|| format!("{program} timed out"))??;
        if !output.status.success() {
            bail!(
                "{} failed: {}",
                program,
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(2_000)
                    .collect::<String>()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn linux_rfb_helpers_encode_tight_frames_and_keysyms() {
            let frame = Frame {
                width: 2,
                height: 2,
                bgra: vec![0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0],
            };
            let update = tight_update(&frame, 2, 2).unwrap();
            assert_eq!(
                i32::from_be_bytes(update[12..16].try_into().unwrap()),
                ENCODING_TIGHT
            );
            assert_eq!(update[16], 0x90);
            assert_eq!(keysym_name(0xff0d).unwrap(), "Return");
            assert_eq!(keysym_name(0xffc9).unwrap(), "F12");
            assert_eq!(keysym_name('x' as u32).unwrap(), "x");
        }

        #[tokio::test]
        async fn xvfb_personal_linux_viewer_streams_sustained_tight_jpeg_frames() {
            let required = env::var("COWORK_REQUIRE_LINUX_DESKTOP_TEST").as_deref() == Ok("1");
            if !available() {
                assert!(
                    !required,
                    "Linux desktop acceptance dependencies are unavailable"
                );
                return;
            }
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
            assert_eq!(&init[24..], b"Open Cowork Linux Device");

            let mut encodings = vec![2, 0, 0, 1];
            encodings.extend(ENCODING_TIGHT.to_be_bytes());
            client
                .send(Message::Binary(encodings.into()))
                .await
                .unwrap();
            let frame_count = env::var("COWORK_LINUX_DESKTOP_SOAK_FRAMES")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(3);
            assert!((1..=1_000).contains(&frame_count));
            let mut total_bytes = 0_usize;
            for frame_index in 0..frame_count {
                let mut request = vec![3, u8::from(frame_index > 0), 0, 0, 0, 0];
                request.extend(320_u16.to_be_bytes());
                request.extend(200_u16.to_be_bytes());
                client.send(Message::Binary(request.into())).await.unwrap();
                let message = timeout(Duration::from_secs(20), client.next())
                    .await
                    .expect("Linux desktop frame timed out")
                    .expect("Linux desktop stream closed")
                    .expect("Linux desktop stream failed");
                let update = binary(message);
                assert_tight_jpeg_update(&update);
                total_bytes = total_bytes
                    .checked_add(update.len())
                    .expect("Linux desktop stream byte count overflowed");
            }
            assert!(total_bytes >= frame_count * 20);
            let _ = client.close(None).await;
            server.abort();
        }

        fn assert_tight_jpeg_update(update: &[u8]) {
            assert!(update.len() > 20);
            assert_eq!(
                i32::from_be_bytes(update[12..16].try_into().unwrap()),
                ENCODING_TIGHT
            );
            assert_eq!(update[16], 0x90);
            let (jpeg_length, compact_length_bytes) = decode_compact_length(&update[17..]);
            let jpeg = &update[17 + compact_length_bytes..];
            assert_eq!(jpeg.len(), jpeg_length);
            assert!(jpeg.starts_with(&[0xff, 0xd8]));
            assert!(jpeg.ends_with(&[0xff, 0xd9]));
        }

        fn decode_compact_length(bytes: &[u8]) -> (usize, usize) {
            assert!(!bytes.is_empty());
            let mut value = usize::from(bytes[0] & 0x7f);
            if bytes[0] & 0x80 == 0 {
                return (value, 1);
            }
            assert!(bytes.len() >= 2);
            value |= usize::from(bytes[1] & 0x7f) << 7;
            if bytes[1] & 0x80 == 0 {
                return (value, 2);
            }
            assert!(bytes.len() >= 3);
            (value | (usize::from(bytes[2]) << 14), 3)
        }

        fn binary(message: Message) -> Vec<u8> {
            match message {
                Message::Binary(bytes) => bytes.to_vec(),
                other => panic!("expected a binary WebSocket message, got {other:?}"),
            }
        }
    }
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
