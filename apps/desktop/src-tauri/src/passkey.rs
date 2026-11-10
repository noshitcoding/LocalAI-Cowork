//! Browser-origin WebAuthn with a one-use local callback and PKCE. No daemon/native
//! secret is supplied to a remote server or exposed through callback URLs.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::State;
#[derive(Default)]
pub struct PasskeyGate(Mutex<Option<Arc<AtomicBool>>>);
struct Guard<'a>(&'a PasskeyGate);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.0.0.lock() {
            if let Some(cancel) = active.as_ref() {
                cancel.store(true, Ordering::Release);
            }
            *active = None;
        }
    }
}
fn nonce() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn base64url(bytes: &[u8]) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut bits = 0u32;
    let mut count = 0;
    for b in bytes {
        bits = (bits << 8) | u32::from(*b);
        count += 8;
        while count >= 6 {
            count -= 6;
            out.push(ABC[((bits >> count) & 63) as usize] as char);
        }
    }
    if count > 0 {
        out.push(ABC[((bits << (6 - count)) & 63) as usize] as char);
    }
    out
}
pub(crate) fn server(endpoint: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(endpoint).map_err(|_| "Invalid server address")?;
    if !(url.scheme() == "https"
        || (url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err("Use an HTTPS server origin or local loopback origin".into());
    }
    url.set_path("/");
    Ok(url)
}
pub(crate) async fn bounded(mut response: reqwest::Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!(
            "Passkey request failed ({})",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Passkey response unavailable")?
    {
        if bytes.len() + chunk.len() > 65536 {
            return Err("Passkey response exceeds its limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid passkey response".into())
}
fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(0x08000000)
            .spawn()
            .map_err(|_| "Cannot open the default browser")?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|_| "Cannot open the default browser")?;
    }
    Ok(())
}
fn callback(
    listener: TcpListener,
    state: String,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(180);
    listener
        .set_nonblocking(true)
        .map_err(|_| "Callback unavailable")?;
    while Instant::now() < deadline && !cancel.load(Ordering::SeqCst) {
        let (mut stream, peer) = match listener.accept() {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(_) => return Err("Callback unavailable".into()),
        };
        if !peer.ip().is_loopback() {
            continue;
        }
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 512];
        while bytes.len() < 4096 && !bytes.windows(4).any(|v| v == b"\r\n\r\n") {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            }
        }
        let line = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|v| v.lines().next())
            .unwrap_or("");
        let mut fields = line.split_whitespace();
        if fields.next() != Some("GET") {
            continue;
        }
        let Some(target) = fields.next() else {
            continue;
        };
        let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
            continue;
        };
        let pairs = url.query_pairs().collect::<Vec<_>>();
        if url.path() != format!("/callback/{state}")
            || pairs.len() != 2
            || pairs.iter().filter(|(k, _)| k == "state").count() != 1
            || pairs.iter().filter(|(k, _)| k == "code").count() != 1
        {
            continue;
        }
        let response_state = pairs
            .iter()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.as_ref())
            .unwrap_or("");
        let code = pairs
            .iter()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();
        if response_state != state || code.len() > 256 || !code.starts_with("cw2a_") {
            continue;
        }
        let _=stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\n\r\n<!doctype html><title>LocalAI Cowork</title><p>Passkey confirmed. Return to LocalAI Cowork. / Passkey bestaetigt. Zur App zurueckkehren.</p>");
        return Ok(code);
    }
    Err(if cancel.load(Ordering::SeqCst) {
        "Passkey sign-in canceled"
    } else {
        "Passkey sign-in expired"
    }
    .into())
}
#[tauri::command]
pub async fn native_passkey_login(
    window: tauri::WebviewWindow,
    gate: State<'_, PasskeyGate>,
    endpoint: String,
) -> Result<Value, String> {
    handoff(window, gate, endpoint, None).await
}
#[tauri::command]
pub async fn native_passkey_register(
    window: tauri::WebviewWindow,
    gate: State<'_, PasskeyGate>,
    endpoint: String,
    access_token: String,
    label: String,
) -> Result<Value, String> {
    handoff(window, gate, endpoint, Some((access_token, label))).await
}
async fn handoff(
    window: tauri::WebviewWindow,
    gate: State<'_, PasskeyGate>,
    endpoint: String,
    registration: Option<(String, String)>,
) -> Result<Value, String> {
    if !super::application_window(&window) {
        return Err("Forbidden".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut active = gate.0.lock().map_err(|_| "Passkey gate unavailable")?;
        if active.is_some() {
            return Err("A passkey sign-in is already pending".into());
        }
        *active = Some(cancel.clone());
    }
    let _guard = Guard(&gate);
    let endpoint = server(&endpoint)?;
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "Passkey connection unavailable")?;
    super::passkey_transport::ready(&window, endpoint.as_str()).await?;
    let original_actor = if let Some((token, _)) = registration.as_ref() {
        Some(super::passkey_transport::actor(&http, &endpoint, token).await?)
    } else {
        None
    };
    let config = bounded(
        http.get(
            endpoint
                .join("api/v1/auth/passkeys/config")
                .map_err(|_| "Invalid address")?,
        )
        .send()
        .await
        .map_err(|_| "Passkey configuration unavailable")?,
    )
    .await?;
    let origin = server(
        config["origin"]
            .as_str()
            .ok_or("Passkeys are not configured")?,
    )?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| "Cannot bind the local callback")?;
    let port = listener
        .local_addr()
        .map_err(|_| "Callback unavailable")?
        .port();
    let state = nonce();
    let verifier = nonce();
    let device = uuid::Uuid::new_v4().to_string();
    let redirect = format!("http://127.0.0.1:{port}/callback/{state}");
    let mut authorize = origin
        .join("api/v1/auth/native/passkey/authorize")
        .map_err(|_| "Invalid authorize origin")?;
    authorize
        .query_pairs_mut()
        .append_pair("state", &state)
        .append_pair(
            "code_challenge",
            &base64url(&Sha256::digest(verifier.as_bytes())),
        )
        .append_pair("device_id", &device)
        .append_pair("redirect_uri", &redirect);
    let registering = registration.is_some();
    if let Some((access_token, label)) = registration {
        let operation = uuid::Uuid::new_v4();
        let body=zeroize::Zeroizing::new(json!({"label":label,"device_id":device,"code_challenge":base64url(&Sha256::digest(verifier.as_bytes())),"state":state,"redirect_uri":redirect}).to_string());
        let access_token = zeroize::Zeroizing::new(access_token);
        let mut result = None;
        for _ in 0..2 {
            let sent = http
                .post(
                    endpoint
                        .join("api/v1/auth/native/passkey/register/start")
                        .map_err(|_| "Invalid address")?,
                )
                .bearer_auth(access_token.as_str())
                .header("Content-Type", "application/json")
                .header("Idempotency-Key", operation.to_string())
                .body(body.as_str().to_owned())
                .send()
                .await;
            if let Ok(response) = sent
                && let Ok(value) = bounded(response).await
            {
                result = Some(value);
                break;
            }
        }
        let ticket = result.ok_or("Passkey registration original response unavailable")?;
        let secret = ticket["ticket"]
            .as_str()
            .ok_or("Registration ticket unavailable")?;
        if !secret.starts_with("cw2a_") || secret.len() > 256 {
            return Err("Invalid registration ticket".into());
        }
        authorize.set_fragment(Some(&format!("ticket={secret}")));
    }
    open_browser(authorize.as_str())?;
    let code = tauri::async_runtime::spawn_blocking(move || callback(listener, state, cancel))
        .await
        .map_err(|_| "Passkey callback interrupted")??;
    let body = zeroize::Zeroizing::new(
        json!({"code":code,"code_verifier":verifier,"device_id":device}).to_string(),
    );
    super::passkey_transport::perform(
        &window,
        &http,
        endpoint.as_str(),
        if registering {
            "registration_ack"
        } else {
            "native_token"
        },
        &body,
        original_actor,
    )
    .await
}
#[tauri::command]
pub fn native_passkey_cancel(
    window: tauri::WebviewWindow,
    gate: State<'_, PasskeyGate>,
) -> Result<(), String> {
    if !super::application_window(&window) {
        return Err("Forbidden".into());
    }
    if let Some(active) = gate
        .0
        .lock()
        .map_err(|_| "Passkey gate unavailable")?
        .as_ref()
    {
        active.store(true, Ordering::SeqCst);
    }
    Ok(())
}
#[tauri::command]
pub async fn native_passkey_pending(
    window: tauri::WebviewWindow,
    endpoint: String,
) -> Result<Vec<cowork_contracts::NativeAuthTransportView>, String> {
    if !super::application_window(&window) {
        return Err("Forbidden".into());
    }
    let endpoint = server(&endpoint)?;
    super::passkey_transport::pending(&window, endpoint.as_str()).await
}
#[tauri::command]
pub async fn native_passkey_resume(
    window: tauri::WebviewWindow,
    gate: State<'_, PasskeyGate>,
    endpoint: String,
    operation_id: String,
    access_token: Option<String>,
) -> Result<Value, String> {
    if !super::application_window(&window) {
        return Err("Forbidden".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut active = gate.0.lock().map_err(|_| "Passkey gate unavailable")?;
        if active.is_some() {
            return Err("A passkey sign-in is already pending".into());
        }
        *active = Some(cancel);
    }
    let _guard = Guard(&gate);
    let id = uuid::Uuid::parse_str(&operation_id).map_err(|_| "native_auth_request_invalid")?;
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "native_auth_connection_unavailable")?;
    let token = access_token.map(zeroize::Zeroizing::new);
    super::passkey_transport::resume(
        &window,
        &http,
        &endpoint,
        id,
        token.as_deref().map(String::as_str),
    )
    .await
}
#[tauri::command]
pub async fn native_passkey_resolve(
    window: tauri::WebviewWindow,
    gate: State<'_, PasskeyGate>,
    operation_id: String,
    acknowledged: bool,
) -> Result<(), String> {
    if !super::application_window(&window) {
        return Err("Forbidden".into());
    }
    {
        let mut active = gate.0.lock().map_err(|_| "Passkey gate unavailable")?;
        if active.is_some() {
            return Err("A passkey sign-in is already pending".into());
        }
        *active = Some(Arc::new(AtomicBool::new(false)));
    }
    let _guard = Guard(&gate);
    let id = uuid::Uuid::parse_str(&operation_id).map_err(|_| "native_auth_request_invalid")?;
    super::passkey_transport::resolve(&window, id, acknowledged).await
}
