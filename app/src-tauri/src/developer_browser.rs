use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

const CDP_TIMEOUT: Duration = Duration::from_secs(12);
const START_ATTEMPTS: usize = 50;
const START_DELAY: Duration = Duration::from_millis(100);

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
}

pub struct DeveloperBrowserState {
    inner: Mutex<Option<BrowserProcess>>,
}

impl Default for DeveloperBrowserState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }
}

impl Drop for DeveloperBrowserState {
    fn drop(&mut self) {
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

fn spawn_browser(executable: &Path, profile_dir: &Path, port: u16) -> Result<Child, String> {
    fs::create_dir_all(profile_dir)
        .map_err(|error| format!("could not create the developer browser profile: {error}"))?;

    let mut command = Command::new(executable);
    command
        .arg("--headless=new")
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

fn instrumentation_script() -> &'static str {
    r#"
(() => {
  if (window.__localAICoworkDeveloperTools) return true;
  const boundedPush = (list, value, max = 300) => {
    list.push(value);
    if (list.length > max) list.splice(0, list.length - max);
  };
  const state = { console: [], network: [] };
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
    if (this.__localAICoworkRequest) this.__localAICoworkRequest.started = performance.now();
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
    }, { once: true });
    return originalSend.apply(this, args);
  };
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

async fn wait_for_page(target: &BrowserTarget) {
    for _ in 0..30 {
        if evaluate_value(target, "document.readyState")
            .await
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .is_some_and(|state| state == "interactive" || state == "complete")
        {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    let _ = ensure_instrumentation(target).await;
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

#[tauri::command]
pub async fn developer_browser_start(
    app: AppHandle,
    state: tauri::State<'_, DeveloperBrowserState>,
) -> Result<BrowserSessionInfo, String> {
    if let Ok(guard) = state.inner.lock() {
        if let Some(process) = guard.as_ref() {
            return Ok(BrowserSessionInfo {
                active: true,
                browser_name: process.target.browser_name.clone(),
                debugger_port: process.target.port,
                profile_path: process.profile_dir.display().to_string(),
            });
        }
    }

    let (browser_name, executable) = browser_candidates().into_iter().next().ok_or_else(|| {
        "No Chromium browser found. Install Microsoft Edge, Google Chrome, or Chromium.".to_string()
    })?;
    let profile_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("could not resolve app data directory: {error}"))?
        .join("developer-browser-profile");
    let port = reserve_debugger_port()?;
    let mut child = spawn_browser(&executable, &profile_dir, port)?;
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
        });
    Ok(info)
}

#[tauri::command]
pub async fn developer_browser_stop(
    state: tauri::State<'_, DeveloperBrowserState>,
) -> Result<(), String> {
    let target = current_target(&state).ok();
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

#[tauri::command]
pub async fn developer_browser_navigate(
    state: tauri::State<'_, DeveloperBrowserState>,
    request: BrowserNavigateRequest,
) -> Result<(), String> {
    let target = current_target(&state)?;
    let url = normalize_navigation_url(&request.url)?;
    ensure_instrumentation(&target).await?;
    cdp_call(&target, "Page.navigate", json!({ "url": url })).await?;
    wait_for_page(&target).await;
    Ok(())
}

#[tauri::command]
pub async fn developer_browser_reload(
    state: tauri::State<'_, DeveloperBrowserState>,
) -> Result<(), String> {
    let target = current_target(&state)?;
    cdp_call(&target, "Page.reload", json!({ "ignoreCache": false })).await?;
    wait_for_page(&target).await;
    Ok(())
}

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
    wait_for_page(&target).await;
    Ok(())
}

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
