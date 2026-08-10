#[cfg(not(feature = "tauri-shell"))]
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;
#[cfg(feature = "tauri-shell")]
use tauri::{AppHandle, Manager};
use tokio::time::{sleep, timeout};
#[cfg(not(feature = "tauri-shell"))]
use tokio::{sync::oneshot, task::JoinHandle};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

const CDP_TIMEOUT: Duration = Duration::from_secs(12);
// Cold Chromium startup on constrained executor and CI hosts can exceed five
// seconds even though the process is healthy. Keep discovery bounded, but give
// the DevTools endpoint the same startup envelope as other CDP operations.
const START_ATTEMPTS: usize = 150;
const START_DELAY: Duration = Duration::from_millis(100);
#[cfg(not(feature = "tauri-shell"))]
const DEFAULT_ACTION_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(not(feature = "tauri-shell"))]
const MAX_ACTION_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(not(feature = "tauri-shell"))]
const MAX_TRACE_BYTES: usize = 64 * 1024 * 1024;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Clone)]
struct BrowserTarget {
    port: u16,
    web_socket_url: String,
    browser_name: String,
}

struct BrowserProcess {
    child: Child,
    target: BrowserTarget,
    profile_dir: PathBuf,
    visible: bool,
}

#[cfg(not(feature = "tauri-shell"))]
struct BrowserTraceSession {
    stop: oneshot::Sender<()>,
    task: JoinHandle<Result<Vec<Value>, String>>,
}

pub struct DeveloperBrowserState {
    inner: Mutex<Option<BrowserProcess>>,
    #[cfg(not(feature = "tauri-shell"))]
    trace: Mutex<Option<BrowserTraceSession>>,
}

impl Default for DeveloperBrowserState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
            #[cfg(not(feature = "tauri-shell"))]
            trace: Mutex::new(None),
        }
    }
}

impl Drop for DeveloperBrowserState {
    fn drop(&mut self) {
        #[cfg(not(feature = "tauri-shell"))]
        if let Ok(trace) = self.trace.get_mut() {
            if let Some(trace) = trace.take() {
                trace.task.abort();
            }
        }
        if let Ok(inner) = self.inner.get_mut() {
            if let Some(process) = inner.as_mut() {
                let _ = process.child.kill();
            }
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSessionInfo {
    pub active: bool,
    pub browser_name: String,
    pub debugger_port: u16,
    pub profile_path: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserConsoleEntry {
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub timestamp: f64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserNetworkEntry {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub status: i64,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub duration_ms: f64,
    #[serde(default)]
    pub transfer_size: f64,
    #[serde(default)]
    pub timestamp: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshot {
    pub active: bool,
    pub url: String,
    pub title: String,
    pub viewport_width: f64,
    pub viewport_height: f64,
    pub device_scale_factor: f64,
    pub screenshot_data_url: String,
    pub dom: String,
    pub text: String,
    pub active_element: String,
    pub console_entries: Vec<BrowserConsoleEntry>,
    pub network_entries: Vec<BrowserNetworkEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserElementInspection {
    pub selector: String,
    pub tag_name: String,
    pub id: String,
    pub classes: Vec<String>,
    pub text: String,
    pub attributes: Value,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserPointerRequest {
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub double_click: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserScrollRequest {
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    pub delta_x: f64,
    pub delta_y: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTextRequest {
    pub text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserKeyRequest {
    pub key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserNavigateRequest {
    pub url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserHistoryRequest {
    pub direction: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCdpRequest {
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

fn browser_candidates() -> Vec<(String, PathBuf)> {
    let mut candidates = Vec::new();
    let mut add = |name: &str, path: PathBuf| {
        if path.is_file() && !candidates.iter().any(|(_, known)| known == &path) {
            candidates.push((name.to_string(), path));
        }
    };

    #[cfg(target_os = "windows")]
    {
        for root in [
            std::env::var_os("PROGRAMFILES"),
            std::env::var_os("PROGRAMFILES(X86)"),
            std::env::var_os("LOCALAPPDATA"),
        ]
        .into_iter()
        .flatten()
        {
            let root = PathBuf::from(root);
            add(
                "Microsoft Edge",
                root.join("Microsoft/Edge/Application/msedge.exe"),
            );
            add(
                "Google Chrome",
                root.join("Google/Chrome/Application/chrome.exe"),
            );
            add(
                "Google Chrome",
                root.join("Google/Chrome SxS/Application/chrome.exe"),
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        add(
            "Google Chrome",
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        );
        add(
            "Microsoft Edge",
            PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
        );
        add(
            "Chromium",
            PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        );
    }

    #[cfg(target_os = "linux")]
    {
        for (name, command) in [
            ("Google Chrome", "google-chrome"),
            ("Microsoft Edge", "microsoft-edge"),
            ("Chromium", "chromium"),
            ("Chromium", "chromium-browser"),
        ] {
            if let Some(path) = resolve_path_command(command) {
                add(name, path);
            }
        }
    }

    #[cfg(target_os = "windows")]
    for (name, command) in [
        ("Microsoft Edge", "msedge.exe"),
        ("Google Chrome", "chrome.exe"),
    ] {
        if let Some(path) = resolve_path_command(command) {
            add(name, path);
        }
    }

    candidates
}

fn resolve_path_command(command: &str) -> Option<PathBuf> {
    let lookup = if cfg!(target_os = "windows") {
        ("where", vec![command])
    } else {
        ("which", vec![command])
    };
    let output = Command::new(lookup.0).args(lookup.1).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
}

fn reserve_debugger_port() -> Result<u16, String> {
    TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| format!("could not reserve a browser debugger port: {error}"))?
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| format!("could not read the browser debugger port: {error}"))
}

fn spawn_browser(
    executable: &Path,
    profile_dir: &Path,
    port: u16,
    visible: bool,
) -> Result<Child, String> {
    fs::create_dir_all(profile_dir)
        .map_err(|error| format!("could not create the developer browser profile: {error}"))?;

    let mut command = Command::new(executable);
    if !visible {
        command.arg("--headless=new");
    }
    command
        .arg(format!("--remote-debugging-port={port}"))
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg("--remote-allow-origins=*")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking")
        .arg("--disable-component-update")
        .arg("--disable-sync")
        .arg("--force-device-scale-factor=1")
        .arg("--window-size=1440,900")
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);

    command
        .spawn()
        .map_err(|error| format!("could not start Chromium developer browser: {error}"))
}

async fn discover_page_target(port: u16, browser_name: &str) -> Result<BrowserTarget, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|error| format!("could not configure developer browser discovery: {error}"))?;
    let endpoint = format!("http://127.0.0.1:{port}/json/list");

    for _ in 0..START_ATTEMPTS {
        if let Ok(response) = client.get(&endpoint).send().await {
            if let Ok(targets) = response.json::<Vec<Value>>().await {
                if let Some(target) = targets.iter().find(|target| {
                    target.get("type").and_then(Value::as_str) == Some("page")
                        && target
                            .get("webSocketDebuggerUrl")
                            .and_then(Value::as_str)
                            .is_some()
                }) {
                    let web_socket_url = target
                        .get("webSocketDebuggerUrl")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    return Ok(BrowserTarget {
                        port,
                        web_socket_url,
                        browser_name: browser_name.to_string(),
                    });
                }
            }
        }
        sleep(START_DELAY).await;
    }

    Err("Chromium started but its DevTools endpoint did not become ready".to_string())
}

fn current_target(state: &DeveloperBrowserState) -> Result<BrowserTarget, String> {
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "developer browser state is unavailable".to_string())?;
    let exited = guard
        .as_mut()
        .and_then(|process| process.child.try_wait().ok().flatten())
        .is_some();
    if exited {
        *guard = None;
        return Err("developer browser stopped unexpectedly".to_string());
    }
    guard
        .as_ref()
        .map(|process| process.target.clone())
        .ok_or_else(|| "developer browser is not running".to_string())
}

async fn cdp_call(target: &BrowserTarget, method: &str, params: Value) -> Result<Value, String> {
    let (mut socket, _) = timeout(CDP_TIMEOUT, connect_async(&target.web_socket_url))
        .await
        .map_err(|_| format!("CDP connection timed out for {method}"))?
        .map_err(|error| format!("could not connect to Chromium CDP: {error}"))?;

    let request = json!({
        "id": 1,
        "method": method,
        "params": params,
    });
    timeout(
        CDP_TIMEOUT,
        socket.send(Message::Text(request.to_string().into())),
    )
    .await
    .map_err(|_| format!("CDP request timed out for {method}"))?
    .map_err(|error| format!("could not send CDP request {method}: {error}"))?;

    loop {
        let next = timeout(CDP_TIMEOUT, socket.next())
            .await
            .map_err(|_| format!("CDP response timed out for {method}"))?;
        let Some(message) = next else {
            return Err(format!("CDP connection closed before {method} completed"));
        };
        let message = message.map_err(|error| format!("CDP response failed: {error}"))?;
        let Message::Text(text) = message else {
            continue;
        };
        let payload: Value = serde_json::from_str(text.as_ref())
            .map_err(|error| format!("CDP returned invalid JSON: {error}"))?;
        if payload.get("id").and_then(Value::as_i64) != Some(1) {
            continue;
        }
        if let Some(error) = payload.get("error") {
            return Err(format!("CDP {method} failed: {error}"));
        }
        return Ok(payload.get("result").cloned().unwrap_or(Value::Null));
    }
}

#[cfg(not(feature = "tauri-shell"))]
fn abort_browser_trace(state: &DeveloperBrowserState) -> Result<(), String> {
    if let Some(trace) = state
        .trace
        .lock()
        .map_err(|_| "developer browser trace state is unavailable".to_string())?
        .take()
    {
        trace.task.abort();
    }
    Ok(())
}

#[cfg(not(feature = "tauri-shell"))]
async fn collect_browser_trace(
    target: BrowserTarget,
    mut stop: oneshot::Receiver<()>,
    ready: oneshot::Sender<Result<(), String>>,
) -> Result<Vec<Value>, String> {
    let mut ready = Some(ready);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|error| format!("could not configure Chromium trace discovery: {error}"))?;
    let version = client
        .get(format!("http://127.0.0.1:{}/json/version", target.port))
        .send()
        .await
        .map_err(|error| format!("could not discover Chromium trace endpoint: {error}"))?
        .json::<Value>()
        .await
        .map_err(|error| format!("Chromium returned invalid trace endpoint data: {error}"))?;
    let browser_web_socket_url = version
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| "Chromium did not expose its browser trace endpoint".to_string())?;
    let connection = timeout(CDP_TIMEOUT, connect_async(browser_web_socket_url)).await;
    let (mut socket, _) = match connection {
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) => {
            let message = format!("could not connect to Chromium trace stream: {error}");
            if let Some(ready) = ready.take() {
                let _ = ready.send(Err(message.clone()));
            }
            return Err(message);
        }
        Err(_) => {
            let message = "Chromium trace connection timed out".to_string();
            if let Some(ready) = ready.take() {
                let _ = ready.send(Err(message.clone()));
            }
            return Err(message);
        }
    };
    let start = json!({
        "id": 1,
        "method": "Tracing.start",
        "params": {
            "categories": "-*,devtools.timeline,v8.execute,blink.user_timing,loading",
            "options": "record-as-much-as-possible",
            "transferMode": "ReportEvents"
        }
    });
    if let Err(error) = timeout(
        CDP_TIMEOUT,
        socket.send(Message::Text(start.to_string().into())),
    )
    .await
    .map_err(|_| "Chromium trace start request timed out".to_string())
    .and_then(|result| result.map_err(|error| format!("could not start Chromium trace: {error}")))
    {
        if let Some(ready) = ready.take() {
            let _ = ready.send(Err(error.clone()));
        }
        return Err(error);
    }

    let mut events = Vec::new();
    let mut encoded_bytes = 0_usize;
    let mut ending = false;
    loop {
        tokio::select! {
            _ = &mut stop, if !ending => {
                ending = true;
                let end = json!({"id":2,"method":"Tracing.end","params":{}});
                timeout(CDP_TIMEOUT, socket.send(Message::Text(end.to_string().into())))
                    .await
                    .map_err(|_| "Chromium trace stop request timed out".to_string())?
                    .map_err(|error| format!("could not stop Chromium trace: {error}"))?;
            }
            next = socket.next() => {
                let Some(message) = next else {
                    let error = "Chromium trace connection closed unexpectedly".to_string();
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Err(error.clone()));
                    }
                    return Err(error);
                };
                let message = message.map_err(|error| format!("Chromium trace stream failed: {error}"))?;
                let Message::Text(text) = message else { continue };
                let payload: Value = serde_json::from_str(text.as_ref())
                    .map_err(|error| format!("Chromium trace returned invalid JSON: {error}"))?;
                if payload.get("id").and_then(Value::as_i64) == Some(1) {
                    if let Some(error) = payload.get("error") {
                        let message = format!("Chromium trace start failed: {error}");
                        if let Some(ready) = ready.take() {
                            let _ = ready.send(Err(message.clone()));
                        }
                        return Err(message);
                    }
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Ok(()));
                    }
                    continue;
                }
                if payload.get("method").and_then(Value::as_str) == Some("Tracing.dataCollected") {
                    if let Some(values) = payload.pointer("/params/value").and_then(Value::as_array) {
                        for value in values {
                            encoded_bytes = encoded_bytes.saturating_add(value.to_string().len());
                            if encoded_bytes > MAX_TRACE_BYTES {
                                return Err("browser trace exceeded the 64 MiB artifact limit".to_string());
                            }
                            events.push(value.clone());
                        }
                    }
                    continue;
                }
                if payload.get("method").and_then(Value::as_str) == Some("Tracing.tracingComplete") {
                    if !ending {
                        return Err("Chromium ended the trace before it was requested".to_string());
                    }
                    return Ok(events);
                }
            }
        }
    }
}

#[cfg(not(feature = "tauri-shell"))]
async fn start_browser_trace(
    state: &DeveloperBrowserState,
    target: BrowserTarget,
) -> Result<(), String> {
    if state
        .trace
        .lock()
        .map_err(|_| "developer browser trace state is unavailable".to_string())?
        .is_some()
    {
        return Err("a browser trace is already active".to_string());
    }
    let (stop_sender, stop_receiver) = oneshot::channel();
    let (ready_sender, ready_receiver) = oneshot::channel();
    let task = tokio::spawn(collect_browser_trace(target, stop_receiver, ready_sender));
    *state
        .trace
        .lock()
        .map_err(|_| "developer browser trace state is unavailable".to_string())? =
        Some(BrowserTraceSession {
            stop: stop_sender,
            task,
        });
    match timeout(CDP_TIMEOUT, ready_receiver).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(error))) => {
            abort_browser_trace(state)?;
            Err(error)
        }
        Ok(Err(_)) => {
            abort_browser_trace(state)?;
            Err("browser trace stopped before initialization completed".to_string())
        }
        Err(_) => {
            abort_browser_trace(state)?;
            Err("browser trace initialization timed out".to_string())
        }
    }
}

#[cfg(not(feature = "tauri-shell"))]
async fn stop_browser_trace(state: &DeveloperBrowserState) -> Result<Vec<Value>, String> {
    let trace = state
        .trace
        .lock()
        .map_err(|_| "developer browser trace state is unavailable".to_string())?
        .take()
        .ok_or_else(|| "no browser trace is active".to_string())?;
    let _ = trace.stop.send(());
    timeout(Duration::from_secs(30), trace.task)
        .await
        .map_err(|_| "browser trace finalization timed out".to_string())?
        .map_err(|error| format!("browser trace task failed: {error}"))?
}

fn instrumentation_script() -> &'static str {
    r#"
(() => {
  if (window.__localAICoworkDeveloperTools) return true;
  const boundedPush = (list, value, max = 300) => {
    list.push(value);
    if (list.length > max) list.splice(0, list.length - max);
  };
  const state = { console: [], network: [], pendingRequests: 0, lastNetworkActivity: performance.now() };
  Object.defineProperty(window, '__localAICoworkDeveloperTools', {
    value: state,
    configurable: false,
    enumerable: false,
    writable: false
  });
  const serialize = (value) => {
    if (typeof value === 'string') return value;
    try { return JSON.stringify(value); } catch (_) { return String(value); }
  };
  for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
    const original = console[level]?.bind(console);
    if (!original) continue;
    console[level] = (...args) => {
      boundedPush(state.console, {
        level,
        message: args.map(serialize).join(' '),
        timestamp: Date.now()
      });
      original(...args);
    };
  }
  window.addEventListener('error', (event) => boundedPush(state.console, {
    level: 'error',
    message: `${event.message || 'Uncaught error'}${event.filename ? ` (${event.filename}:${event.lineno || 0})` : ''}`,
    timestamp: Date.now()
  }));
  window.addEventListener('unhandledrejection', (event) => boundedPush(state.console, {
    level: 'error',
    message: `Unhandled promise rejection: ${serialize(event.reason)}`,
    timestamp: Date.now()
  }));
  const originalFetch = window.fetch.bind(window);
  window.fetch = async (...args) => {
    const started = performance.now();
    state.pendingRequests += 1;
    state.lastNetworkActivity = started;
    const input = args[0];
    const init = args[1] || {};
    const url = typeof input === 'string' ? input : input?.url || String(input);
    const method = init.method || (typeof input !== 'string' && input?.method) || 'GET';
    try {
      const response = await originalFetch(...args);
      boundedPush(state.network, {
        url: response.url || url,
        method,
        status: response.status,
        kind: 'fetch',
        durationMs: performance.now() - started,
        transferSize: 0,
        timestamp: Date.now()
      });
      state.pendingRequests = Math.max(0, state.pendingRequests - 1);
      state.lastNetworkActivity = performance.now();
      return response;
    } catch (error) {
      boundedPush(state.network, {
        url,
        method,
        status: 0,
        kind: 'fetch',
        durationMs: performance.now() - started,
        transferSize: 0,
        timestamp: Date.now()
      });
      state.pendingRequests = Math.max(0, state.pendingRequests - 1);
      state.lastNetworkActivity = performance.now();
      throw error;
    }
  };
  const originalOpen = XMLHttpRequest.prototype.open;
  const originalSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function(method, url, ...rest) {
    this.__localAICoworkRequest = { method, url: String(url), started: 0 };
    return originalOpen.call(this, method, url, ...rest);
  };
  XMLHttpRequest.prototype.send = function(...args) {
    if (this.__localAICoworkRequest) {
      this.__localAICoworkRequest.started = performance.now();
      state.pendingRequests += 1;
      state.lastNetworkActivity = performance.now();
    }
    this.addEventListener('loadend', () => {
      const request = this.__localAICoworkRequest || {};
      boundedPush(state.network, {
        url: this.responseURL || request.url || '',
        method: request.method || 'GET',
        status: this.status || 0,
        kind: 'xhr',
        durationMs: request.started ? performance.now() - request.started : 0,
        transferSize: 0,
        timestamp: Date.now()
      });
      state.pendingRequests = Math.max(0, state.pendingRequests - 1);
      state.lastNetworkActivity = performance.now();
    }, { once: true });
    return originalSend.apply(this, args);
  };
  new PerformanceObserver((list) => {
    if (list.getEntries().length > 0) state.lastNetworkActivity = performance.now();
  }).observe({ type: 'resource', buffered: true });
  return true;
})()
"#
}

async fn ensure_instrumentation(target: &BrowserTarget) -> Result<(), String> {
    let script = instrumentation_script();
    let _ = cdp_call(
        target,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": script }),
    )
    .await?;
    let _ = cdp_call(
        target,
        "Runtime.evaluate",
        json!({
            "expression": script,
            "returnByValue": true,
            "awaitPromise": true,
        }),
    )
    .await?;
    Ok(())
}

async fn evaluate_value(target: &BrowserTarget, expression: &str) -> Result<Value, String> {
    let result = cdp_call(
        target,
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
            "userGesture": true,
        }),
    )
    .await?;
    if let Some(exception) = result.get("exceptionDetails") {
        return Err(format!("page evaluation failed: {exception}"));
    }
    Ok(result
        .get("result")
        .and_then(|result| result.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

async fn wait_for_page(
    target: &BrowserTarget,
    wait_until: &str,
    maximum: Duration,
) -> Result<(), String> {
    if wait_until == "commit" {
        return Ok(());
    }
    if !matches!(wait_until, "load" | "domcontentloaded" | "networkidle") {
        return Err(format!("unsupported browser wait condition: {wait_until}"));
    }
    let deadline = std::time::Instant::now() + maximum;
    loop {
        let state = evaluate_value(
            target,
            "(() => { const tools = window.__localAICoworkDeveloperTools; const resources = performance.getEntriesByType('resource'); const lastResource = resources.reduce((latest, entry) => Math.max(latest, entry.responseEnd || entry.startTime || 0), 0); const lastActivity = tools ? Math.max(tools.lastNetworkActivity, lastResource) : lastResource; return { readyState: document.readyState, instrumented: Boolean(tools), pendingRequests: tools?.pendingRequests || 0, quietForMs: performance.now() - lastActivity }; })()",
        )
        .await?;
        if !state
            .get("instrumented")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            ensure_instrumentation(target).await?;
            sleep(Duration::from_millis(50)).await;
            continue;
        }
        let ready_state = state
            .get("readyState")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let ready = match wait_until {
            "load" => ready_state == "complete",
            "domcontentloaded" => matches!(ready_state, "interactive" | "complete"),
            "networkidle" => {
                ready_state == "complete"
                    && state
                        .get("pendingRequests")
                        .and_then(Value::as_u64)
                        .unwrap_or_default()
                        == 0
                    && state
                        .get("quietForMs")
                        .and_then(Value::as_f64)
                        .unwrap_or_default()
                        >= 500.0
            }
            _ => false,
        };
        if ready {
            ensure_instrumentation(target).await?;
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "browser navigation did not reach {wait_until} within {} ms (last state: {state})",
                maximum.as_millis(),
            ));
        }
        sleep(Duration::from_millis(50)).await;
    }
}

fn normalize_navigation_url(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("URL is required".to_string());
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let url = Url::parse(&candidate).map_err(|error| format!("invalid browser URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("developer browser allows only http and https URLs".to_string());
    }
    Ok(url.to_string())
}

fn validate_cdp_method(value: &str) -> Result<String, String> {
    let method = value.trim();
    if method.is_empty()
        || method.len() > 128
        || !method
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_'))
    {
        return Err("CDP method is invalid".to_string());
    }
    let domain = method
        .split_once('.')
        .map(|(domain, _)| domain)
        .unwrap_or("");
    if !matches!(
        domain,
        "Accessibility"
            | "CSS"
            | "DOM"
            | "DOMDebugger"
            | "Emulation"
            | "Input"
            | "Log"
            | "Network"
            | "Overlay"
            | "Page"
            | "Performance"
            | "Runtime"
    ) {
        return Err(format!(
            "CDP domain {domain} is not available in this session"
        ));
    }
    if matches!(
        method,
        "Page.setDownloadBehavior"
            | "Page.crash"
            | "Page.close"
            | "Runtime.terminateExecution"
            | "Runtime.runIfWaitingForDebugger"
    ) {
        return Err(format!(
            "CDP method {method} is blocked by the browser policy"
        ));
    }
    Ok(method.to_string())
}

fn validate_pointer(x: f64, y: f64) -> Result<(), String> {
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 || x > 100_000.0 || y > 100_000.0 {
        return Err("browser coordinates are invalid".to_string());
    }
    Ok(())
}

async fn ensure_browser_started(
    state: &DeveloperBrowserState,
    profile_dir: PathBuf,
    visible: bool,
) -> Result<BrowserSessionInfo, String> {
    let existing_session = state.inner.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|process| (process.visible, process.profile_dir.clone()))
    });
    if existing_session == Some((visible, profile_dir.clone())) {
        let process = state
            .inner
            .lock()
            .map_err(|_| "developer browser state is unavailable".to_string())?;
        let process = process
            .as_ref()
            .ok_or_else(|| "developer browser is not running".to_string())?;
        return Ok(BrowserSessionInfo {
            active: true,
            browser_name: process.target.browser_name.clone(),
            debugger_port: process.target.port,
            profile_path: process.profile_dir.display().to_string(),
        });
    }
    if existing_session.is_some() {
        #[cfg(not(feature = "tauri-shell"))]
        abort_browser_trace(state)?;
        if let Some(mut process) = state
            .inner
            .lock()
            .map_err(|_| "developer browser state is unavailable".to_string())?
            .take()
        {
            let _ = process.child.kill();
            let _ = process.child.wait();
        }
    }
    let (browser_name, executable) = browser_candidates().into_iter().next().ok_or_else(|| {
        "No Chromium browser found. Install Microsoft Edge, Google Chrome, or Chromium.".to_string()
    })?;
    let port = reserve_debugger_port()?;
    let mut child = spawn_browser(&executable, &profile_dir, port, visible)?;
    let target = match discover_page_target(port, &browser_name).await {
        Ok(target) => target,
        Err(error) => {
            let _ = child.kill();
            return Err(error);
        }
    };
    if let Err(error) = ensure_instrumentation(&target).await {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let info = BrowserSessionInfo {
        active: true,
        browser_name: target.browser_name.clone(),
        debugger_port: target.port,
        profile_path: profile_dir.display().to_string(),
    };
    *state
        .inner
        .lock()
        .map_err(|_| "developer browser state is unavailable".to_string())? =
        Some(BrowserProcess {
            child,
            target,
            profile_dir,
            visible,
        });
    Ok(info)
}

#[cfg(not(feature = "tauri-shell"))]
fn local_workspace_path(
    workspace: &Path,
    relative: &str,
    must_exist: bool,
) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if relative.is_empty()
        || relative.contains('\0')
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err("browser path must stay inside the project workspace".to_string());
    }
    let root = workspace
        .canonicalize()
        .map_err(|error| format!("browser workspace is unavailable: {error}"))?;
    let candidate = root.join(path);
    if must_exist {
        let canonical = candidate
            .canonicalize()
            .map_err(|error| format!("browser input file is unavailable: {error}"))?;
        if !canonical.starts_with(&root) || !canonical.is_file() {
            return Err("browser input path must be a project file".to_string());
        }
        return Ok(canonical);
    }
    let mut ancestor = candidate
        .parent()
        .ok_or_else(|| "browser artifact has no parent directory".to_string())?;
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or_else(|| "browser artifact escaped the workspace".to_string())?;
    }
    if !ancestor
        .canonicalize()
        .map_err(|error| format!("browser artifact parent is unavailable: {error}"))?
        .starts_with(&root)
    {
        return Err("browser artifact traverses a symlink outside the workspace".to_string());
    }
    Ok(candidate)
}

#[cfg(not(feature = "tauri-shell"))]
fn browser_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

#[cfg(not(feature = "tauri-shell"))]
fn action_timeout(request: &Value) -> Result<Duration, String> {
    let timeout = request
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_ACTION_TIMEOUT);
    if timeout.is_zero() || timeout > MAX_ACTION_TIMEOUT {
        return Err("browser timeout_ms must be between 1 and 120000".to_string());
    }
    Ok(timeout)
}

#[cfg(not(feature = "tauri-shell"))]
async fn wait_for_selector(
    target: &BrowserTarget,
    selector: &str,
    maximum: Duration,
    visible: bool,
) -> Result<(), String> {
    let selector_json = serde_json::to_string(selector)
        .map_err(|error| format!("could not encode browser selector: {error}"))?;
    let expression = if visible {
        format!(
            "(() => {{ const element = document.querySelector({selector_json}); if (!element || !element.isConnected) return false; const style = getComputedStyle(element); const rect = element.getBoundingClientRect(); return style.visibility !== 'hidden' && style.display !== 'none' && rect.width > 0 && rect.height > 0; }})()"
        )
    } else {
        format!(
            "(() => {{ const element = document.querySelector({selector_json}); return Boolean(element && element.isConnected); }})()"
        )
    };
    let deadline = std::time::Instant::now() + maximum;
    loop {
        if evaluate_value(target, &expression)
            .await?
            .as_bool()
            .unwrap_or(false)
        {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "browser selector {selector:?} did not become {} within {} ms",
                if visible { "visible" } else { "available" },
                maximum.as_millis()
            ));
        }
        sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(not(feature = "tauri-shell"))]
async fn selector_center(
    target: &BrowserTarget,
    selector: &str,
    maximum: Duration,
) -> Result<(f64, f64), String> {
    wait_for_selector(target, selector, maximum, true).await?;
    let selector = serde_json::to_string(selector)
        .map_err(|error| format!("could not encode browser selector: {error}"))?;
    let value = evaluate_value(
        target,
        &format!(
            "(() => {{ const element = document.querySelector({selector}); if (!element) throw new Error('selector did not match an element'); const rect = element.getBoundingClientRect(); element.scrollIntoView({{block:'center',inline:'center'}}); const next = element.getBoundingClientRect(); return {{x: next.left + next.width / 2, y: next.top + next.height / 2}}; }})()"
        ),
    )
    .await?;
    let x = value
        .get("x")
        .and_then(Value::as_f64)
        .ok_or_else(|| "browser selector did not return an x coordinate".to_string())?;
    let y = value
        .get("y")
        .and_then(Value::as_f64)
        .ok_or_else(|| "browser selector did not return a y coordinate".to_string())?;
    Ok((x, y))
}

#[cfg(not(feature = "tauri-shell"))]
async fn click_selector(
    target: &BrowserTarget,
    selector: &str,
    maximum: Duration,
) -> Result<Value, String> {
    let (x, y) = selector_center(target, selector, maximum).await?;
    cdp_call(
        target,
        "Input.dispatchMouseEvent",
        json!({"type":"mousePressed","x":x,"y":y,"button":"left","clickCount":1}),
    )
    .await?;
    cdp_call(
        target,
        "Input.dispatchMouseEvent",
        json!({"type":"mouseReleased","x":x,"y":y,"button":"left","clickCount":1}),
    )
    .await?;
    Ok(json!({"selector": selector, "x": x, "y": y}))
}

/// Executes the browser contract without a WebView. This entry point is used by
/// the durable personal-device daemon; the Tauri commands below remain thin UI
/// wrappers around the same CDP process.
#[cfg(not(feature = "tauri-shell"))]
pub async fn local_browser_execute(
    state: &DeveloperBrowserState,
    profile_dir: &Path,
    workspace: &Path,
    request: &Value,
) -> Result<Value, String> {
    let action = request
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| "browser action is required".to_string())?;
    let visible = request
        .get("visible")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    ensure_browser_started(state, profile_dir.to_path_buf(), visible).await?;
    let target = current_target(state)?;
    let mut artifacts = Vec::new();
    let mut output = match action {
        "navigate" => {
            let url = normalize_navigation_url(
                request
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "browser URL is required".to_string())?,
            )?;
            let wait_until = request
                .get("wait_until")
                .and_then(Value::as_str)
                .unwrap_or("load");
            let maximum = action_timeout(request)?;
            cdp_call(&target, "Page.enable", json!({})).await?;
            let result = cdp_call(&target, "Page.navigate", json!({"url": url})).await?;
            if let Some(error) = result.get("errorText").and_then(Value::as_str) {
                return Err(format!("browser navigation failed: {error}"));
            }
            wait_for_page(&target, wait_until, maximum).await?;
            json!({"navigation": result, "wait_until": wait_until, "timeout_ms": maximum.as_millis()})
        }
        "click" => {
            let selector = request
                .get("selector")
                .and_then(Value::as_str)
                .ok_or_else(|| "browser selector is required".to_string())?;
            let maximum = action_timeout(request)?;
            if request
                .get("expect_download")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let relative = request
                    .get("download_path")
                    .and_then(Value::as_str)
                    .unwrap_or("artifacts/browser/downloads");
                let requested_path = local_workspace_path(workspace, relative, false)?;
                let download_dir = if Path::new(relative).extension().is_some() {
                    requested_path
                        .parent()
                        .ok_or_else(|| "download path has no parent".to_string())?
                        .to_path_buf()
                } else {
                    requested_path.clone()
                };
                fs::create_dir_all(&download_dir)
                    .map_err(|error| format!("could not create download directory: {error}"))?;
                let before = fs::read_dir(&download_dir)
                    .map_err(|error| format!("could not list download directory: {error}"))?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .collect::<std::collections::HashSet<_>>();
                cdp_call(
                    &target,
                    "Browser.setDownloadBehavior",
                    json!({"behavior":"allow","downloadPath":download_dir}),
                )
                .await?;
                click_selector(&target, selector, maximum).await?;
                let deadline = std::time::Instant::now() + maximum;
                let downloaded = loop {
                    let candidate = fs::read_dir(&download_dir)
                        .map_err(|error| format!("could not list download directory: {error}"))?
                        .filter_map(Result::ok)
                        .map(|entry| entry.path())
                        .find(|path| {
                            !before.contains(path)
                                && path.extension().and_then(|value| value.to_str())
                                    != Some("crdownload")
                        });
                    if candidate.is_some() || std::time::Instant::now() >= deadline {
                        break candidate;
                    }
                    sleep(Duration::from_millis(100)).await;
                }
                .ok_or_else(|| {
                    format!(
                        "browser download did not complete within {} ms",
                        maximum.as_millis()
                    )
                })?;
                let root = workspace
                    .canonicalize()
                    .map_err(|error| format!("browser workspace is unavailable: {error}"))?;
                let relative_download = downloaded
                    .canonicalize()
                    .map_err(|error| format!("download is unavailable: {error}"))?
                    .strip_prefix(&root)
                    .map_err(|_| "download escaped the browser workspace".to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                artifacts.push(relative_download.clone());
                json!({"download": relative_download})
            } else {
                click_selector(&target, selector, maximum).await?
            }
        }
        "fill" => {
            let selector_value = request
                .get("selector")
                .and_then(Value::as_str)
                .ok_or_else(|| "browser selector is required".to_string())?;
            wait_for_selector(&target, selector_value, action_timeout(request)?, false).await?;
            let selector =
                serde_json::to_string(selector_value).map_err(|error| error.to_string())?;
            let value =
                serde_json::to_string(request.get("value").and_then(Value::as_str).unwrap_or(""))
                    .map_err(|error| error.to_string())?;
            evaluate_value(
                &target,
                &format!(
                    "(() => {{ const element = document.querySelector({selector}); if (!element) throw new Error('selector did not match an element'); element.focus(); const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), 'value')?.set; if (setter) setter.call(element, {value}); else element.value = {value}; element.dispatchEvent(new Event('input', {{bubbles:true}})); element.dispatchEvent(new Event('change', {{bubbles:true}})); return true; }})()"
                ),
            )
            .await?
        }
        "upload" => {
            let selector = request
                .get("selector")
                .and_then(Value::as_str)
                .ok_or_else(|| "browser selector is required".to_string())?;
            wait_for_selector(&target, selector, action_timeout(request)?, false).await?;
            let relative_paths = request
                .get("paths")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                .unwrap_or_else(|| {
                    request
                        .get("path")
                        .and_then(Value::as_str)
                        .into_iter()
                        .collect()
                });
            if relative_paths.is_empty() || relative_paths.len() > 50 {
                return Err("browser upload requires between one and 50 files".to_string());
            }
            let files = relative_paths
                .iter()
                .map(|path| local_workspace_path(workspace, path, true))
                .collect::<Result<Vec<_>, _>>()?;
            let selector_json =
                serde_json::to_string(selector).map_err(|error| error.to_string())?;
            let evaluated = cdp_call(
                &target,
                "Runtime.evaluate",
                json!({"expression":format!("document.querySelector({selector_json})"),"returnByValue":false}),
            )
            .await?;
            let object_id = evaluated
                .pointer("/result/objectId")
                .and_then(Value::as_str)
                .ok_or_else(|| "browser upload selector did not match an element".to_string())?;
            cdp_call(&target, "DOM.enable", json!({})).await?;
            let node = cdp_call(&target, "DOM.requestNode", json!({"objectId":object_id})).await?;
            let node_id = node
                .get("nodeId")
                .and_then(Value::as_i64)
                .ok_or_else(|| "browser upload element has no DOM node".to_string())?;
            cdp_call(
                &target,
                "DOM.setFileInputFiles",
                json!({"nodeId":node_id,"files":files}),
            )
            .await?;
            json!({"uploaded":relative_paths})
        }
        "trace_start" => {
            start_browser_trace(state, target.clone()).await?;
            json!({"trace_active":true})
        }
        "trace_stop" => {
            let relative = request
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("artifacts/browser/{}-trace.json", browser_stamp()));
            let path = local_workspace_path(workspace, &relative, false)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!("could not create browser trace directory: {error}")
                })?;
            }
            let events = stop_browser_trace(state).await?;
            let trace = json!({
                "schema_version": 1,
                "format": "chrome-trace-event",
                "traceEvents": events,
            });
            let encoded = serde_json::to_vec(&trace)
                .map_err(|error| format!("could not encode browser trace: {error}"))?;
            fs::write(&path, encoded)
                .map_err(|error| format!("could not write browser trace: {error}"))?;
            artifacts.push(relative.clone());
            json!({"trace":relative,"trace_active":false,"event_count":trace["traceEvents"].as_array().map(Vec::len).unwrap_or_default()})
        }
        "screenshot" => {
            let relative = request
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("artifacts/browser/{}-screenshot.png", browser_stamp()));
            let path = local_workspace_path(workspace, &relative, false)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("could not create screenshot directory: {error}"))?;
            }
            let result = cdp_call(
                &target,
                "Page.captureScreenshot",
                json!({"format":"png","captureBeyondViewport":request.get("full_page").and_then(Value::as_bool).unwrap_or(true)}),
            )
            .await?;
            let encoded = result
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| "Chromium did not return screenshot data".to_string())?;
            let bytes = BASE64
                .decode(encoded)
                .map_err(|error| format!("Chromium returned invalid screenshot data: {error}"))?;
            fs::write(&path, bytes)
                .map_err(|error| format!("could not write browser screenshot: {error}"))?;
            artifacts.push(relative.clone());
            json!({"screenshot":relative})
        }
        "inspect" => {
            let maximum = request
                .get("max_chars")
                .and_then(Value::as_u64)
                .unwrap_or(100_000)
                .clamp(1, 200_000);
            evaluate_value(
                &target,
                &format!(
                    "(() => ({{title:document.title,url:location.href,text:(document.body?.innerText||'').slice(0,{maximum}),links:Array.from(document.querySelectorAll('a')).slice(0,500).map(item=>({{text:(item.textContent||'').trim(),href:item.href}})),console_entries:window.__localAICoworkDeveloperTools?.console||[],network_entries:window.__localAICoworkDeveloperTools?.network||[]}}))()"
                ),
            )
            .await?
        }
        "tabs" => {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .map_err(|error| format!("could not configure browser tab discovery: {error}"))?;
            let tabs = client
                .get(format!("http://127.0.0.1:{}/json/list", target.port))
                .send()
                .await
                .map_err(|error| format!("could not list browser tabs: {error}"))?
                .json::<Value>()
                .await
                .map_err(|error| format!("Chromium returned invalid tab data: {error}"))?;
            json!({"tabs":tabs})
        }
        other => return Err(format!("unsupported browser action: {other}")),
    };
    if let Some(wait_ms) = request.get("wait_ms").and_then(Value::as_u64) {
        sleep(Duration::from_millis(wait_ms.min(30_000))).await;
    }
    if let Some(object) = output.as_object_mut() {
        object.insert("visible".to_string(), Value::Bool(visible));
        object.insert("artifacts".to_string(), json!(artifacts));
    }
    Ok(output)
}

#[cfg(not(feature = "tauri-shell"))]
pub fn local_browser_available() -> bool {
    !browser_candidates().is_empty()
}

pub async fn local_browser_stop(state: &DeveloperBrowserState) -> Result<(), String> {
    #[cfg(not(feature = "tauri-shell"))]
    abort_browser_trace(state)?;
    let target = current_target(state).ok();
    if let Some(target) = target {
        let _ = cdp_call(&target, "Browser.close", json!({})).await;
    }
    if let Some(mut process) = state
        .inner
        .lock()
        .map_err(|_| "developer browser state is unavailable".to_string())?
        .take()
    {
        let _ = process.child.kill();
        let _ = process.child.wait();
    }
    Ok(())
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_start(
    app: AppHandle,
    state: tauri::State<'_, DeveloperBrowserState>,
) -> Result<BrowserSessionInfo, String> {
    let profile_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("could not resolve app data directory: {error}"))?
        .join("developer-browser-profile");
    ensure_browser_started(&state, profile_dir, false).await
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_stop(
    state: tauri::State<'_, DeveloperBrowserState>,
) -> Result<(), String> {
    local_browser_stop(&state).await
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_status(
    state: tauri::State<'_, DeveloperBrowserState>,
) -> Result<BrowserSessionInfo, String> {
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "developer browser state is unavailable".to_string())?;
    let exited = guard
        .as_mut()
        .and_then(|process| process.child.try_wait().ok().flatten())
        .is_some();
    if exited {
        *guard = None;
    }
    Ok(match guard.as_ref() {
        Some(process) => BrowserSessionInfo {
            active: true,
            browser_name: process.target.browser_name.clone(),
            debugger_port: process.target.port,
            profile_path: process.profile_dir.display().to_string(),
        },
        None => BrowserSessionInfo {
            active: false,
            browser_name: String::new(),
            debugger_port: 0,
            profile_path: String::new(),
        },
    })
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_navigate(
    state: tauri::State<'_, DeveloperBrowserState>,
    request: BrowserNavigateRequest,
) -> Result<(), String> {
    let target = current_target(&state)?;
    let url = normalize_navigation_url(&request.url)?;
    ensure_instrumentation(&target).await?;
    cdp_call(&target, "Page.navigate", json!({ "url": url })).await?;
    wait_for_page(&target, "load", Duration::from_secs(30)).await?;
    Ok(())
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_reload(
    state: tauri::State<'_, DeveloperBrowserState>,
) -> Result<(), String> {
    let target = current_target(&state)?;
    cdp_call(&target, "Page.reload", json!({ "ignoreCache": false })).await?;
    wait_for_page(&target, "load", Duration::from_secs(30)).await?;
    Ok(())
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_history(
    state: tauri::State<'_, DeveloperBrowserState>,
    request: BrowserHistoryRequest,
) -> Result<(), String> {
    let target = current_target(&state)?;
    let history = cdp_call(&target, "Page.getNavigationHistory", json!({})).await?;
    let current = history
        .get("currentIndex")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let entries = history
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| "browser navigation history is unavailable".to_string())?;
    let desired = if request.direction.eq_ignore_ascii_case("forward") {
        current + 1
    } else {
        current - 1
    };
    let entry = entries
        .get(desired.max(0) as usize)
        .ok_or_else(|| format!("no {} history entry", request.direction))?;
    let entry_id = entry
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| "browser history entry is invalid".to_string())?;
    cdp_call(
        &target,
        "Page.navigateToHistoryEntry",
        json!({ "entryId": entry_id }),
    )
    .await?;
    wait_for_page(&target, "load", Duration::from_secs(30)).await?;
    Ok(())
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_snapshot(
    state: tauri::State<'_, DeveloperBrowserState>,
) -> Result<BrowserSnapshot, String> {
    let target = current_target(&state)?;
    ensure_instrumentation(&target).await?;

    let page_value = evaluate_value(
        &target,
        r#"(() => {
          const tools = window.__localAICoworkDeveloperTools || { console: [], network: [] };
          const performanceEntries = performance.getEntriesByType('resource').slice(-200).map((entry) => ({
            url: entry.name || '',
            method: 'GET',
            status: 0,
            kind: entry.initiatorType || 'resource',
            durationMs: entry.duration || 0,
            transferSize: entry.transferSize || 0,
            timestamp: performance.timeOrigin + (entry.startTime || 0)
          }));
          return {
            url: location.href,
            title: document.title,
            viewportWidth: window.innerWidth,
            viewportHeight: window.innerHeight,
            deviceScaleFactor: window.devicePixelRatio || 1,
            dom: (document.documentElement?.outerHTML || '').slice(0, 250000),
            text: (document.body?.innerText || '').slice(0, 100000),
            activeElement: document.activeElement
              ? `${document.activeElement.tagName.toLowerCase()}${document.activeElement.id ? `#${document.activeElement.id}` : ''}`
              : '',
            consoleEntries: tools.console.slice(-200),
            networkEntries: [...performanceEntries, ...tools.network].slice(-300)
          };
        })()"#,
    )
    .await?;
    let screenshot = cdp_call(
        &target,
        "Page.captureScreenshot",
        json!({
            "format": "png",
            "fromSurface": true,
            "captureBeyondViewport": false,
            "optimizeForSpeed": true,
        }),
    )
    .await?;
    let screenshot_data = screenshot
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| "Chromium did not return a screenshot".to_string())?;

    Ok(BrowserSnapshot {
        active: true,
        url: page_value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        title: page_value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        viewport_width: page_value
            .get("viewportWidth")
            .and_then(Value::as_f64)
            .unwrap_or(1440.0),
        viewport_height: page_value
            .get("viewportHeight")
            .and_then(Value::as_f64)
            .unwrap_or(900.0),
        device_scale_factor: page_value
            .get("deviceScaleFactor")
            .and_then(Value::as_f64)
            .unwrap_or(1.0),
        screenshot_data_url: format!("data:image/png;base64,{screenshot_data}"),
        dom: page_value
            .get("dom")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        text: page_value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        active_element: page_value
            .get("activeElement")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        console_entries: serde_json::from_value(
            page_value
                .get("consoleEntries")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )
        .unwrap_or_default(),
        network_entries: serde_json::from_value(
            page_value
                .get("networkEntries")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )
        .unwrap_or_default(),
    })
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_click(
    state: tauri::State<'_, DeveloperBrowserState>,
    request: BrowserPointerRequest,
) -> Result<(), String> {
    validate_pointer(request.x, request.y)?;
    let target = current_target(&state)?;
    let click_count = if request.double_click { 2 } else { 1 };
    cdp_call(
        &target,
        "Input.dispatchMouseEvent",
        json!({
            "type": "mousePressed",
            "x": request.x,
            "y": request.y,
            "button": "left",
            "buttons": 1,
            "clickCount": click_count,
        }),
    )
    .await?;
    cdp_call(
        &target,
        "Input.dispatchMouseEvent",
        json!({
            "type": "mouseReleased",
            "x": request.x,
            "y": request.y,
            "button": "left",
            "buttons": 0,
            "clickCount": click_count,
        }),
    )
    .await?;
    sleep(Duration::from_millis(180)).await;
    Ok(())
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_scroll(
    state: tauri::State<'_, DeveloperBrowserState>,
    request: BrowserScrollRequest,
) -> Result<(), String> {
    validate_pointer(request.x, request.y)?;
    if !request.delta_x.is_finite()
        || !request.delta_y.is_finite()
        || request.delta_x.abs() > 100_000.0
        || request.delta_y.abs() > 100_000.0
    {
        return Err("browser scroll delta is invalid".to_string());
    }
    let target = current_target(&state)?;
    cdp_call(
        &target,
        "Input.dispatchMouseEvent",
        json!({
            "type": "mouseWheel",
            "x": request.x,
            "y": request.y,
            "deltaX": request.delta_x,
            "deltaY": request.delta_y,
        }),
    )
    .await?;
    sleep(Duration::from_millis(120)).await;
    Ok(())
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_type_text(
    state: tauri::State<'_, DeveloperBrowserState>,
    request: BrowserTextRequest,
) -> Result<(), String> {
    if request.text.len() > 1024 * 1024 {
        return Err("browser text input exceeds 1 MiB".to_string());
    }
    let target = current_target(&state)?;
    cdp_call(&target, "Input.insertText", json!({ "text": request.text })).await?;
    Ok(())
}

fn key_definition(key: &str) -> (&str, &str, i64) {
    match key.to_ascii_lowercase().as_str() {
        "enter" => ("Enter", "\r", 13),
        "tab" => ("Tab", "\t", 9),
        "escape" | "esc" => ("Escape", "", 27),
        "backspace" => ("Backspace", "", 8),
        "delete" => ("Delete", "", 46),
        "arrowup" | "up" => ("ArrowUp", "", 38),
        "arrowdown" | "down" => ("ArrowDown", "", 40),
        "arrowleft" | "left" => ("ArrowLeft", "", 37),
        "arrowright" | "right" => ("ArrowRight", "", 39),
        _ => (key, key, 0),
    }
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_keypress(
    state: tauri::State<'_, DeveloperBrowserState>,
    request: BrowserKeyRequest,
) -> Result<(), String> {
    let target = current_target(&state)?;
    let (key, text, virtual_key) = key_definition(request.key.trim());
    cdp_call(
        &target,
        "Input.dispatchKeyEvent",
        json!({
            "type": "keyDown",
            "key": key,
            "text": text,
            "windowsVirtualKeyCode": virtual_key,
            "nativeVirtualKeyCode": virtual_key,
        }),
    )
    .await?;
    cdp_call(
        &target,
        "Input.dispatchKeyEvent",
        json!({
            "type": "keyUp",
            "key": key,
            "windowsVirtualKeyCode": virtual_key,
            "nativeVirtualKeyCode": virtual_key,
        }),
    )
    .await?;
    Ok(())
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_inspect(
    state: tauri::State<'_, DeveloperBrowserState>,
    request: BrowserPointerRequest,
) -> Result<BrowserElementInspection, String> {
    validate_pointer(request.x, request.y)?;
    let target = current_target(&state)?;
    let expression = format!(
        r#"(() => {{
          const element = document.elementFromPoint({}, {});
          if (!element) return null;
          const selector = (node) => {{
            if (node.id) return `#${{CSS.escape(node.id)}}`;
            const parts = [];
            let current = node;
            while (current && current.nodeType === 1 && parts.length < 6) {{
              let part = current.tagName.toLowerCase();
              const classes = [...current.classList].slice(0, 2);
              if (classes.length) part += classes.map((value) => `.${{CSS.escape(value)}}`).join('');
              const siblings = current.parentElement
                ? [...current.parentElement.children].filter((child) => child.tagName === current.tagName)
                : [];
              if (siblings.length > 1) part += `:nth-of-type(${{siblings.indexOf(current) + 1}})`;
              parts.unshift(part);
              current = current.parentElement;
            }}
            return parts.join(' > ');
          }};
          const rect = element.getBoundingClientRect();
          return {{
            selector: selector(element),
            tagName: element.tagName.toLowerCase(),
            id: element.id || '',
            classes: [...element.classList],
            text: (element.innerText || element.textContent || '').trim().slice(0, 1000),
            attributes: Object.fromEntries([...element.attributes].map((attribute) => [attribute.name, attribute.value])),
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height
          }};
        }})()"#,
        request.x, request.y
    );
    let value = evaluate_value(&target, &expression).await?;
    if value.is_null() {
        return Err("no rendered element exists at this position".to_string());
    }
    Ok(BrowserElementInspection {
        selector: value
            .get("selector")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        tag_name: value
            .get("tagName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        id: value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        classes: serde_json::from_value(value.get("classes").cloned().unwrap_or_else(|| json!([])))
            .unwrap_or_default(),
        text: value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        attributes: value
            .get("attributes")
            .cloned()
            .unwrap_or_else(|| json!({})),
        x: value.get("x").and_then(Value::as_f64).unwrap_or(0.0),
        y: value.get("y").and_then(Value::as_f64).unwrap_or(0.0),
        width: value.get("width").and_then(Value::as_f64).unwrap_or(0.0),
        height: value.get("height").and_then(Value::as_f64).unwrap_or(0.0),
    })
}

#[cfg(feature = "tauri-shell")]
#[tauri::command]
pub async fn developer_browser_cdp_call(
    state: tauri::State<'_, DeveloperBrowserState>,
    request: BrowserCdpRequest,
) -> Result<Value, String> {
    let method = validate_cdp_method(&request.method)?;
    let encoded_params = serde_json::to_vec(&request.params)
        .map_err(|error| format!("CDP parameters are invalid: {error}"))?;
    if encoded_params.len() > 256 * 1024 {
        return Err("CDP parameters exceed 256 KiB".to_string());
    }
    let target = current_target(&state)?;
    let response = cdp_call(&target, &method, request.params).await?;
    if serde_json::to_vec(&response)
        .map(|encoded| encoded.len() > 2 * 1024 * 1024)
        .unwrap_or(true)
    {
        return Err("CDP response exceeds 2 MiB".to_string());
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::{normalize_navigation_url, validate_cdp_method, validate_pointer};

    #[test]
    fn navigation_accepts_http_and_rejects_local_file_urls() {
        assert_eq!(
            normalize_navigation_url("localhost:5173").unwrap(),
            "http://localhost:5173/"
        );
        assert!(normalize_navigation_url("file:///C:/secret.txt").is_err());
    }

    #[test]
    fn cdp_console_is_domain_scoped() {
        assert_eq!(
            validate_cdp_method("Runtime.evaluate").unwrap(),
            "Runtime.evaluate"
        );
        assert!(validate_cdp_method("Browser.getVersion").is_err());
        assert!(validate_cdp_method("Page.crash").is_err());
    }

    #[test]
    fn pointer_rejects_non_finite_values() {
        assert!(validate_pointer(24.0, 42.0).is_ok());
        assert!(validate_pointer(f64::NAN, 42.0).is_err());
    }
}

#[cfg(all(test, not(feature = "tauri-shell")))]
mod daemon_browser_tests {
    use super::{
        local_browser_available, local_browser_execute, local_browser_stop, DeveloperBrowserState,
    };
    use serde_json::{json, Value};
    use std::{fs, sync::Arc};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn daemon_browser_navigates_fills_inspects_and_captures_an_artifact() {
        if !local_browser_available() {
            return;
        }
        let workspace =
            std::env::temp_dir().join(format!("open-cowork-browser-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&workspace);
        fs::create_dir_all(&workspace).unwrap();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..4 {
                let Ok(Ok((mut stream, _))) =
                    tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                        .await
                else {
                    return;
                };
                let mut request = vec![0_u8; 4096];
                let _ = stream.read(&mut request).await;
                let body = "<!doctype html><title>Daemon browser</title><p id='value'></p><script>setTimeout(()=>{const input=document.createElement('input');input.id='name';input.addEventListener('input',e=>document.querySelector('#value').textContent=e.target.value);document.body.appendChild(input)},250)</script>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        let state = Arc::new(DeveloperBrowserState::default());
        let profile = workspace.join("profile");
        local_browser_execute(
            &state,
            &profile,
            &workspace,
            &json!({"action":"navigate","url":format!("http://{address}/"),"wait_until":"networkidle","timeout_ms":5000}),
        )
        .await
        .unwrap();
        local_browser_execute(
            &state,
            &profile,
            &workspace,
            &json!({"action":"trace_start"}),
        )
        .await
        .unwrap();
        local_browser_execute(
            &state,
            &profile,
            &workspace,
            &json!({"action":"fill","selector":"#name","value":"durable","visible":false,"timeout_ms":2000}),
        )
        .await
        .unwrap();
        let missing = local_browser_execute(
            &state,
            &profile,
            &workspace,
            &json!({"action":"fill","selector":"#missing","value":"nope","timeout_ms":100}),
        )
        .await
        .unwrap_err();
        assert!(missing.contains("did not become available within 100 ms"));
        let inspected =
            local_browser_execute(&state, &profile, &workspace, &json!({"action":"inspect"}))
                .await
                .unwrap();
        assert_eq!(
            inspected.get("title").and_then(|value| value.as_str()),
            Some("Daemon browser")
        );
        assert!(inspected
            .get("text")
            .and_then(|value| value.as_str())
            .is_some_and(|text| text.contains("durable")));
        let screenshot = local_browser_execute(
            &state,
            &profile,
            &workspace,
            &json!({"action":"screenshot","path":"artifacts/browser/test.png"}),
        )
        .await
        .unwrap();
        assert_eq!(
            screenshot
                .pointer("/artifacts/0")
                .and_then(|value| value.as_str()),
            Some("artifacts/browser/test.png")
        );
        assert!(workspace.join("artifacts/browser/test.png").is_file());
        let trace = local_browser_execute(
            &state,
            &profile,
            &workspace,
            &json!({"action":"trace_stop","path":"artifacts/browser/test-trace.json"}),
        )
        .await
        .unwrap();
        assert_eq!(
            trace.pointer("/artifacts/0").and_then(Value::as_str),
            Some("artifacts/browser/test-trace.json")
        );
        assert!(trace
            .get("event_count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count > 0));
        let trace_document: Value = serde_json::from_slice(
            &fs::read(workspace.join("artifacts/browser/test-trace.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            trace_document.get("schema_version").and_then(Value::as_u64),
            Some(1)
        );
        local_browser_stop(&state).await.unwrap();
        server.abort();
        let _ = server.await;
        fs::remove_dir_all(&workspace).unwrap();
    }
}
