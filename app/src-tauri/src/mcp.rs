use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use thiserror::Error;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "windows")]
fn suppress_command_window(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn suppress_command_window(_command: &mut Command) {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerRequest {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCallRequest {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub tool_name: String,
    pub tool_args: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpProbeResponse {
    pub server_name: String,
    pub protocol_version: Option<String>,
    pub server_info: Option<String>,
    pub tools: Vec<McpTool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCallResponse {
    pub server_name: String,
    pub tool_name: String,
    pub success: bool,
    pub result: String,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRuntimeServerStatus {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub pid: Option<u32>,
    pub started_at: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Error)]
pub enum McpError {
    #[error("invalid command")]
    InvalidCommand,
    #[error("failed to spawn process: {0}")]
    SpawnFailed(String),
    #[error("failed to communicate with process: {0}")]
    IoFailed(String),
    #[error("MCP server rejected the request: {0}")]
    RpcFailed(String),
    #[error("timed out waiting for MCP response")]
    Timeout,
}

struct RuntimeMcpServer {
    name: String,
    command: String,
    args: Vec<String>,
    child: Child,
    stdin: ChildStdin,
    stdout_rx: mpsc::Receiver<String>,
    next_id: i64,
    started_at: String,
    last_error: Option<String>,
}

impl RuntimeMcpServer {
    fn status(&self) -> McpRuntimeServerStatus {
        McpRuntimeServerStatus {
            name: self.name.clone(),
            command: self.command.clone(),
            args: self.args.clone(),
            pid: Some(self.child.id()),
            started_at: self.started_at.clone(),
            last_error: self.last_error.clone(),
        }
    }

    fn next_request_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

static MCP_RUNTIME_SERVERS: OnceLock<Mutex<HashMap<String, RuntimeMcpServer>>> = OnceLock::new();

fn runtime_registry() -> &'static Mutex<HashMap<String, RuntimeMcpServer>> {
    MCP_RUNTIME_SERVERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn spawn_process(
    command_name: &str,
    args: &[String],
    env: &HashMap<String, String>,
) -> Result<Child, McpError> {
    let mut command = Command::new(command_name.trim());
    command
        .args(args.iter().map(String::as_str))
        .envs(
            env.iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    suppress_command_window(&mut command);

    command
        .spawn()
        .map_err(|error| McpError::SpawnFailed(error.to_string()))
}

fn spawn_runtime_server(req: &McpServerRequest) -> Result<RuntimeMcpServer, McpError> {
    let mut child = spawn_process(&req.command, &req.args, &req.env)?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| McpError::IoFailed("missing stdin pipe".to_string()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| McpError::IoFailed("missing stdout pipe".to_string()))?;

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    Ok(RuntimeMcpServer {
        name: req.name.clone(),
        command: req.command.clone(),
        args: req.args.clone(),
        child,
        stdin,
        stdout_rx: rx,
        next_id: 1,
        started_at: Utc::now().to_rfc3339(),
        last_error: None,
    })
}

fn runtime_send_rpc(
    server: &mut RuntimeMcpServer,
    method: &str,
    params: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, McpError> {
    let request_id = server.next_request_id();
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": params,
    });

    writeln!(server.stdin, "{}", payload).map_err(|error| McpError::IoFailed(error.to_string()))?;
    server
        .stdin
        .flush()
        .map_err(|error| McpError::IoFailed(error.to_string()))?;

    let start = Instant::now();
    while start.elapsed() < timeout {
        let remaining = timeout.saturating_sub(start.elapsed());
        let wait_for = remaining.min(Duration::from_millis(400));

        match server.stdout_rx.recv_timeout(wait_for) {
            Ok(line) => {
                let value: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(parsed) => parsed,
                    Err(_) => continue,
                };

                if value.get("id").and_then(|entry| entry.as_i64()) != Some(request_id) {
                    continue;
                }

                if let Some(error) = value.get("error") {
                    let message = error
                        .get("message")
                        .and_then(|entry| entry.as_str())
                        .unwrap_or("unknown MCP error")
                        .to_string();
                    return Err(McpError::RpcFailed(message));
                }

                return Ok(value
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Err(McpError::IoFailed("MCP process output closed".to_string())),
        }
    }

    Err(McpError::Timeout)
}

fn runtime_send_initialized_notification(server: &mut RuntimeMcpServer) -> Result<(), McpError> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {},
    });

    writeln!(server.stdin, "{}", payload).map_err(|error| McpError::IoFailed(error.to_string()))?;
    server
        .stdin
        .flush()
        .map_err(|error| McpError::IoFailed(error.to_string()))
}

fn runtime_initialize(server: &mut RuntimeMcpServer) -> Result<serde_json::Value, McpError> {
    let result = runtime_send_rpc(
        server,
        "initialize",
        serde_json::json!({
            "protocolVersion": "2024-11-05",
            "clientInfo": {
                "name": "LocalAI Cowork",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {}
        }),
        Duration::from_secs(8),
    )?;

    runtime_send_initialized_notification(server)?;
    Ok(result)
}

fn runtime_shutdown_server(server: &mut RuntimeMcpServer) {
    let _ = server.child.kill();
    let _ = server.child.wait();
}

fn parse_tools_from_result(result: &serde_json::Value) -> Vec<McpTool> {
    let mut tools = Vec::new();
    if let Some(items) = result.get("tools").and_then(|entry| entry.as_array()) {
        for item in items {
            tools.push(McpTool {
                name: item
                    .get("name")
                    .and_then(|entry| entry.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                description: item
                    .get("description")
                    .and_then(|entry| entry.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }
    }
    tools
}

fn format_call_result(result: &serde_json::Value) -> String {
    if let Some(content) = result.get("content").and_then(|entry| entry.as_array()) {
        let combined = content
            .iter()
            .filter_map(|entry| entry.get("text").and_then(|text| text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        if !combined.is_empty() {
            return combined;
        }
    }

    serde_json::to_string_pretty(result).unwrap_or_default()
}

pub fn runtime_has_server(name: &str) -> bool {
    runtime_registry()
        .lock()
        .map(|servers| servers.contains_key(name))
        .unwrap_or(false)
}

pub fn runtime_list_servers() -> Result<Vec<McpRuntimeServerStatus>, McpError> {
    let servers = runtime_registry()
        .lock()
        .map_err(|_| McpError::IoFailed("runtime registry lock poisoned".to_string()))?;
    Ok(servers.values().map(RuntimeMcpServer::status).collect())
}

pub fn runtime_start_server(req: McpServerRequest) -> Result<McpRuntimeServerStatus, McpError> {
    if req.command.trim().is_empty() {
        return Err(McpError::InvalidCommand);
    }

    let mut servers = runtime_registry()
        .lock()
        .map_err(|_| McpError::IoFailed("runtime registry lock poisoned".to_string()))?;

    if let Some(existing) = servers.get(&req.name) {
        return Ok(existing.status());
    }

    let mut server = spawn_runtime_server(&req)?;
    if let Err(error) = runtime_initialize(&mut server).and_then(|_| {
        runtime_send_rpc(
            &mut server,
            "tools/list",
            serde_json::json!({}),
            Duration::from_secs(8),
        )
        .map(|_| ())
    }) {
        server.last_error = Some(error.to_string());
        runtime_shutdown_server(&mut server);
        return Err(error);
    }

    let status = server.status();
    servers.insert(req.name, server);
    Ok(status)
}

pub fn runtime_stop_server(name: &str) -> Result<bool, McpError> {
    let mut servers = runtime_registry()
        .lock()
        .map_err(|_| McpError::IoFailed("runtime registry lock poisoned".to_string()))?;

    let Some(mut server) = servers.remove(name) else {
        return Ok(false);
    };

    runtime_shutdown_server(&mut server);
    Ok(true)
}

pub fn runtime_restart_server(req: McpServerRequest) -> Result<McpRuntimeServerStatus, McpError> {
    let _ = runtime_stop_server(&req.name)?;
    runtime_start_server(req)
}

pub fn runtime_probe_server(name: &str) -> Result<McpProbeResponse, McpError> {
    let mut servers = runtime_registry()
        .lock()
        .map_err(|_| McpError::IoFailed("runtime registry lock poisoned".to_string()))?;

    let server = servers
        .get_mut(name)
        .ok_or_else(|| McpError::SpawnFailed(format!("runtime server not started: {}", name)))?;

    let result = runtime_send_rpc(
        server,
        "tools/list",
        serde_json::json!({}),
        Duration::from_secs(8),
    )?;

    Ok(McpProbeResponse {
        server_name: server.name.clone(),
        protocol_version: Some("2024-11-05".to_string()),
        server_info: Some(format!(
            "runtime pid={} command={} {}",
            server.child.id(),
            server.command,
            server.args.join(" ")
        )),
        tools: parse_tools_from_result(&result),
    })
}

pub fn runtime_call_tool(
    name: &str,
    tool_name: &str,
    tool_args: HashMap<String, serde_json::Value>,
) -> Result<McpCallResponse, McpError> {
    let mut servers = runtime_registry()
        .lock()
        .map_err(|_| McpError::IoFailed("runtime registry lock poisoned".to_string()))?;

    let server = servers
        .get_mut(name)
        .ok_or_else(|| McpError::SpawnFailed(format!("runtime server not started: {}", name)))?;

    let result = runtime_send_rpc(
        server,
        "tools/call",
        serde_json::json!({
            "name": tool_name,
            "arguments": tool_args,
        }),
        Duration::from_secs(15),
    )?;

    Ok(McpCallResponse {
        server_name: name.to_string(),
        tool_name: tool_name.to_string(),
        success: true,
        result: format_call_result(&result),
        error: None,
    })
}

pub fn probe_server(req: McpServerRequest) -> Result<McpProbeResponse, McpError> {
    if req.command.trim().is_empty() {
        return Err(McpError::InvalidCommand);
    }

    let mut server = spawn_runtime_server(&req)?;
    let result = (|| {
        let initialized = runtime_initialize(&mut server)?;
        let listed = runtime_send_rpc(
            &mut server,
            "tools/list",
            serde_json::json!({}),
            Duration::from_secs(8),
        )?;
        let protocol_version = initialized
            .get("protocolVersion")
            .and_then(|entry| entry.as_str())
            .map(ToString::to_string);
        let server_info = initialized.get("serverInfo").map(|info| {
            let name = info
                .get("name")
                .and_then(|entry| entry.as_str())
                .unwrap_or("unknown");
            let version = info
                .get("version")
                .and_then(|entry| entry.as_str())
                .unwrap_or("unknown");
            format!("{} {}", name, version)
        });
        Ok(McpProbeResponse {
            server_name: req.name,
            protocol_version,
            server_info,
            tools: parse_tools_from_result(&listed),
        })
    })();
    runtime_shutdown_server(&mut server);
    result
}

pub fn call_tool(req: McpCallRequest) -> Result<McpCallResponse, McpError> {
    if req.command.trim().is_empty() {
        return Err(McpError::InvalidCommand);
    }

    let server_name = req.name;
    let tool_name = req.tool_name;
    let tool_args = req.tool_args;
    let mut server = spawn_runtime_server(&McpServerRequest {
        name: server_name.clone(),
        command: req.command,
        args: req.args,
        env: req.env,
    })?;
    let result = (|| {
        runtime_initialize(&mut server)?;
        match runtime_send_rpc(
            &mut server,
            "tools/call",
            serde_json::json!({
                "name": tool_name,
                "arguments": tool_args,
            }),
            Duration::from_secs(15),
        ) {
            Ok(value) => Ok(McpCallResponse {
                server_name: server_name.clone(),
                tool_name: tool_name.clone(),
                success: true,
                result: format_call_result(&value),
                error: None,
            }),
            Err(McpError::RpcFailed(message)) => Ok(McpCallResponse {
                server_name: server_name.clone(),
                tool_name: tool_name.clone(),
                success: false,
                result: String::new(),
                error: Some(message),
            }),
            Err(error) => Err(error),
        }
    })();
    runtime_shutdown_server(&mut server);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRICT_SERVER: &str = r#"
const readline = require('node:readline');
let initialized = false;
const rl = readline.createInterface({ input: process.stdin });
const send = (value) => process.stdout.write(JSON.stringify(value) + '\n');
rl.on('line', (line) => {
  const request = JSON.parse(line);
  if (request.method === 'initialize') {
    send({ jsonrpc: '2.0', id: request.id, result: { protocolVersion: '2024-11-05', serverInfo: { name: 'strict-test', version: '1.0.0' }, capabilities: { tools: {} } } });
    return;
  }
  if (request.method === 'notifications/initialized') { initialized = true; return; }
  if (!initialized) { send({ jsonrpc: '2.0', id: request.id, error: { code: -32002, message: 'client not initialized' } }); return; }
  if (request.method === 'tools/list') { send({ jsonrpc: '2.0', id: request.id, result: { tools: [{ name: 'echo', description: 'Echo input' }] } }); return; }
  if (request.method === 'tools/call') { send({ jsonrpc: '2.0', id: request.id, result: { content: [{ type: 'text', text: String(request.params.arguments.value || '') }] } }); }
});
"#;

    fn node_available() -> bool {
        Command::new("node")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn server_request() -> McpServerRequest {
        McpServerRequest {
            name: "strict-test".to_string(),
            command: "node".to_string(),
            args: vec!["-e".to_string(), STRICT_SERVER.to_string()],
            env: HashMap::new(),
        }
    }

    #[test]
    fn one_shot_probe_completes_the_mcp_initialization_handshake() {
        if !node_available() {
            return;
        }
        let response = probe_server(server_request()).unwrap();
        assert_eq!(response.protocol_version.as_deref(), Some("2024-11-05"));
        assert_eq!(response.server_info.as_deref(), Some("strict-test 1.0.0"));
        assert_eq!(response.tools.len(), 1);
        assert_eq!(response.tools[0].name, "echo");
    }

    #[test]
    fn one_shot_tool_call_runs_only_after_initialized_notification() {
        if !node_available() {
            return;
        }
        let request = McpCallRequest {
            name: "strict-test".to_string(),
            command: "node".to_string(),
            args: vec!["-e".to_string(), STRICT_SERVER.to_string()],
            env: HashMap::new(),
            tool_name: "echo".to_string(),
            tool_args: HashMap::from([(
                "value".to_string(),
                serde_json::Value::String("handshake-ok".to_string()),
            )]),
        };
        let response = call_tool(request).unwrap();
        assert!(response.success);
        assert_eq!(response.result, "handshake-ok");
    }
}
