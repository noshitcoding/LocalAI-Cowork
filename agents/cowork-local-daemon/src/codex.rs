use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout},
    sync::Mutex,
};
use uuid::Uuid;

use cowork_contracts::{RunEventKind, RunState};
use cowork_runtime::{RuntimeHost, ToolInvocation};

use super::{
    append_event_async, await_local_approval, local_run_is_canceled, set_local_run_state_locked,
    Daemon, LocalRuntimeHost, ManagedProcessTree,
};

const CODEX_VERSION: &str = "0.147.0";
const PROTOCOL_SCHEMA: &str = "app-server-0.147.0";
const MAX_PROTOCOL_LINE: usize = 16 * 1024 * 1024;
const MAX_STDERR_CAPTURE: usize = 16_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeManifest {
    version: String,
    protocol_schema: String,
    binary: String,
    sha256: String,
    license: String,
}

struct CodexProtocol {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    process_tree: ManagedProcessTree,
    next_id: u64,
    stderr_task: tokio::task::JoinHandle<String>,
}

type CodexSession = Arc<Mutex<Option<CodexProtocol>>>;

#[derive(Default)]
pub(super) struct CodexSessionPool {
    sessions: Mutex<HashMap<String, CodexSession>>,
}

impl CodexSessionPool {
    async fn session(&self, profile_id: &str) -> CodexSession {
        let mut sessions = self.sessions.lock().await;
        sessions
            .entry(profile_id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone()
    }
}

impl CodexProtocol {
    async fn send(&mut self, value: &Value) -> Result<()> {
        let encoded = serde_json::to_vec(value)?;
        if encoded.len() > MAX_PROTOCOL_LINE {
            bail!("Codex protocol request exceeds 16 MiB");
        }
        self.stdin.write_all(&encoded).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({"method": method, "params": params}))
            .await
    }

    async fn request_id(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"id": id, "method": method, "params": params}))
            .await?;
        Ok(id)
    }

    async fn respond(&mut self, id: u64, result: Value) -> Result<()> {
        self.send(&json!({"id": id, "result": result})).await
    }

    async fn stop(mut self) -> String {
        self.process_tree.terminate();
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        self.stderr_task.await.unwrap_or_default()
    }
}

pub(super) async fn execute_codex_adapter(
    daemon: &Daemon,
    host: &LocalRuntimeHost<'_>,
    run_id: Uuid,
    request: Value,
    workspace: Option<PathBuf>,
    timeout: Duration,
) -> Result<Value> {
    let request = request
        .as_object()
        .context("Codex runtime request must be an object")?;
    let profile_id = required_string(request.get("profile_id"), "profile_id")?.to_owned();
    validate_profile_id(&profile_id)?;
    let prompt = required_string(request.get("prompt"), "prompt")?.to_owned();
    let model = optional_string(request.get("model")).map(str::to_owned);
    let effort = optional_string(request.get("reasoning_effort")).map(str::to_owned);
    let tool_policy = optional_string(request.get("tool_policy"))
        .unwrap_or("autonomous")
        .to_owned();
    let cwd = workspace
        .as_deref()
        .or_else(|| optional_string(request.get("cwd")).map(Path::new))
        .context("Codex runs require a bound project workspace")?
        .canonicalize()
        .context("Codex workspace is unavailable")?;
    let binary = verified_runtime(&daemon.config.runtime_paths.codex_root)?;
    let profile_home = daemon.config.runtime_paths.codex_profiles.join(&profile_id);
    if !profile_home.is_dir() {
        bail!("Codex profile {profile_id} is not initialized on this device");
    }
    if profile_home.join("auth.json").exists() {
        bail!("Codex profile {profile_id} contains forbidden plaintext auth.json");
    }

    let deadline = tokio::time::Instant::now() + timeout;
    let session = daemon.codex_sessions.session(&profile_id).await;
    let mut protocol = match tokio::time::timeout_at(deadline, session.lock()).await {
        Ok(protocol) => protocol,
        Err(_) => return Err(codex_timeout_error(timeout)),
    };
    let result = tokio::time::timeout_at(deadline, async {
        let process_exited = match protocol.as_mut() {
            Some(protocol) => protocol
                .child
                .try_wait()
                .context("failed to inspect the Codex App Server")?
                .is_some(),
            None => false,
        };
        if process_exited {
            let _ = stop_protocol(&mut protocol).await;
        }
        if protocol.is_none() {
            *protocol = Some(spawn_protocol(&binary, &profile_home)?);
            initialize_protocol(
                daemon,
                run_id,
                protocol.as_mut().context("Codex session was not created")?,
            )
            .await?;
        }

        append_event_async(
            daemon,
            run_id,
            RunEventKind::ModelStarted,
            json!({"adapter":"codex","profile_id":profile_id,"model":model}),
        )
        .await?;
        execute_codex_turn(
            daemon,
            host,
            run_id,
            protocol.as_mut().context("Codex session is unavailable")?,
            &profile_id,
            &prompt,
            model.as_deref(),
            effort.as_deref(),
            &tool_policy,
            &cwd,
        )
        .await
    })
    .await;

    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            let stderr = stop_protocol(&mut protocol).await;
            Err(attach_protocol_stderr(error, &stderr))
        }
        Err(_) => {
            let _ = stop_protocol(&mut protocol).await;
            Err(codex_timeout_error(timeout))
        }
    }
}

async fn stop_protocol(protocol: &mut Option<CodexProtocol>) -> String {
    match protocol.take() {
        Some(protocol) => protocol.stop().await,
        None => String::new(),
    }
}

fn attach_protocol_stderr(error: anyhow::Error, stderr: &str) -> anyhow::Error {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        error
    } else {
        error.context(format!("Codex App Server stderr: {stderr}"))
    }
}

fn codex_timeout_error(timeout: Duration) -> anyhow::Error {
    anyhow::anyhow!(
        "Codex runtime exceeded its configured timeout of {} seconds",
        timeout.as_secs()
    )
}

fn spawn_protocol(binary: &Path, profile_home: &Path) -> Result<CodexProtocol> {
    let mut command = tokio::process::Command::new(binary);
    command
        .args(["app-server", "--listen", "stdio://"])
        .env("CODEX_HOME", profile_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for secret in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "AZURE_OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
    ] {
        command.env_remove(secret);
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .context("failed to start Codex App Server")?;
    let process_tree = ManagedProcessTree::attach(&child)?;
    let stdin = child.stdin.take().context("Codex stdin is missing")?;
    let stdout = child.stdout.take().context("Codex stdout is missing")?;
    let mut stderr = child.stderr.take().context("Codex stderr is missing")?;
    let stderr_task = tokio::spawn(async move {
        let mut captured = Vec::with_capacity(MAX_STDERR_CAPTURE);
        let mut chunk = [0_u8; 4096];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(read) => append_bounded(&mut captured, &chunk[..read], MAX_STDERR_CAPTURE),
            }
        }
        String::from_utf8_lossy(&captured).into_owned()
    });
    Ok(CodexProtocol {
        child,
        stdin,
        lines: BufReader::new(stdout).lines(),
        process_tree,
        next_id: 1,
        stderr_task,
    })
}

fn append_bounded(captured: &mut Vec<u8>, chunk: &[u8], limit: usize) {
    if chunk.len() >= limit {
        captured.clear();
        captured.extend_from_slice(&chunk[chunk.len() - limit..]);
        return;
    }
    let overflow = captured
        .len()
        .saturating_add(chunk.len())
        .saturating_sub(limit);
    if overflow > 0 {
        captured.drain(..overflow);
    }
    captured.extend_from_slice(chunk);
}

async fn initialize_protocol(
    daemon: &Daemon,
    run_id: Uuid,
    protocol: &mut CodexProtocol,
) -> Result<()> {
    let initialize_id = protocol
        .request_id(
            "initialize",
            json!({
                "clientInfo": {"name":"open_cowork_daemon","title":"OpenCowork","version":env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": true}
            }),
        )
        .await?;
    let initialized = wait_for_response(daemon, run_id, protocol, initialize_id).await?;
    if initialized
        .get("userAgent")
        .and_then(Value::as_str)
        .is_none()
    {
        bail!("Codex handshake did not match the pinned schema");
    }
    protocol.notify("initialized", json!({})).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn execute_codex_turn(
    daemon: &Daemon,
    host: &LocalRuntimeHost<'_>,
    run_id: Uuid,
    protocol: &mut CodexProtocol,
    profile_id: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    tool_policy: &str,
    cwd: &Path,
) -> Result<Value> {
    let read_only = tool_policy == "read_only";
    let thread_id_request = protocol
        .request_id(
            "thread/start",
            json!({
                "cwd": cwd,
                "model": model,
                "approvalPolicy": "untrusted",
                "sandbox": if read_only { "read-only" } else { "workspace-write" },
                "serviceName": "open_cowork_daemon",
                "dynamicTools": host.tools().into_iter().map(|tool| json!({
                    "type":"function",
                    "name":tool.name,
                    "description":tool.description,
                    "inputSchema":tool.input_schema
                })).collect::<Vec<_>>()
            }),
        )
        .await?;
    let thread = wait_for_response(daemon, run_id, protocol, thread_id_request).await?;
    let thread_id = thread
        .get("thread")
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .context("Codex thread/start returned no thread ID")?
        .to_owned();
    let turn_request = protocol
        .request_id(
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{"type":"text","text":prompt}],
                "cwd": cwd,
                "approvalPolicy": "untrusted",
                "sandboxPolicy": if read_only {
                    json!({"type":"readOnly","networkAccess":false})
                } else {
                    json!({"type":"workspaceWrite","writableRoots":[cwd],"networkAccess":false})
                },
                "model": model,
                "effort": effort
            }),
        )
        .await?;
    let turn = wait_for_response(daemon, run_id, protocol, turn_request).await?;
    let turn_id = turn
        .get("turn")
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .context("Codex turn/start returned no turn ID")?
        .to_owned();

    let mut content = String::new();
    loop {
        let payload = next_payload(daemon, run_id, protocol).await?;
        if let Some(id) = payload.get("id").and_then(Value::as_u64) {
            if let Some(method) = payload.get("method").and_then(Value::as_str) {
                let params = payload.get("params").cloned().unwrap_or(Value::Null);
                if method.ends_with("/requestApproval") {
                    let approved = await_local_approval(
                        daemon,
                        run_id,
                        json!({"adapter":"codex","method":method,"request_id":id,"details":params}),
                    )
                    .await?;
                    protocol
                        .respond(
                            id,
                            json!({"decision": if approved { "accept" } else { "decline" }}),
                        )
                        .await?;
                } else if method == "item/tool/call" {
                    let tool_name = params.get("tool").and_then(Value::as_str).unwrap_or("");
                    let call_id = params
                        .get("callId")
                        .and_then(Value::as_str)
                        .unwrap_or("codex-dynamic-tool")
                        .to_owned();
                    let arguments = params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    append_event_async(
                        daemon,
                        run_id,
                        RunEventKind::ToolStarted,
                        json!({"tool_call_id":call_id,"tool":tool_name,"arguments":arguments}),
                    )
                    .await?;
                    let output = host
                        .execute_tool(ToolInvocation {
                            id: call_id.clone(),
                            name: tool_name.to_owned(),
                            arguments,
                        })
                        .await;
                    let (content, success) = match output {
                        Ok(output) => (output.content, !output.is_error),
                        Err(error) => (error.to_string(), false),
                    };
                    protocol
                        .respond(
                            id,
                            json!({
                                "contentItems":[{"type":"inputText","text":content}],
                                "success":success
                            }),
                        )
                        .await?;
                    append_event_async(
                        daemon,
                        run_id,
                        if success {
                            RunEventKind::ToolCompleted
                        } else {
                            RunEventKind::ToolFailed
                        },
                        json!({"tool_call_id":call_id,"tool":tool_name,"content":content}),
                    )
                    .await?;
                } else {
                    protocol.respond(id, Value::Null).await?;
                }
                continue;
            }
        }
        let method = payload.get("method").and_then(Value::as_str).unwrap_or("");
        let params = payload.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "item/agentMessage/delta" => {
                if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                    content.push_str(delta);
                    append_event_async(
                        daemon,
                        run_id,
                        RunEventKind::ModelDelta,
                        json!({"adapter":"codex","delta":delta}),
                    )
                    .await?;
                }
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                    append_event_async(
                        daemon,
                        run_id,
                        RunEventKind::ModelDelta,
                        json!({"adapter":"codex","thinking":delta}),
                    )
                    .await?;
                }
            }
            "item/started" => {
                let item = params.get("item").cloned().unwrap_or(Value::Null);
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("item");
                if is_codex_tool_item(item_type) {
                    append_event_async(
                        daemon,
                        run_id,
                        RunEventKind::ToolStarted,
                        json!({"tool_call_id":item.get("id"),"tool":format!("Codex:{item_type}"),"arguments":item}),
                    )
                    .await?;
                }
            }
            "item/completed" => {
                let item = params.get("item").cloned().unwrap_or(Value::Null);
                if item.get("type").and_then(Value::as_str) == Some("agentMessage")
                    && content.is_empty()
                {
                    content = item
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                }
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("item");
                if is_codex_tool_item(item_type) {
                    append_event_async(
                        daemon,
                        run_id,
                        RunEventKind::ToolCompleted,
                        json!({"tool_call_id":item.get("id"),"tool":format!("Codex:{item_type}"),"content":item.to_string()}),
                    )
                    .await?;
                }
            }
            "turn/completed" => {
                let turn = params.get("turn").cloned().unwrap_or(Value::Null);
                let status = turn
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                if status != "completed" {
                    let message = turn
                        .get("error")
                        .and_then(|value| value.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("Codex turn failed");
                    bail!("{message}");
                }
                append_event_async(
                    daemon,
                    run_id,
                    RunEventKind::ModelCompleted,
                    json!({"adapter":"codex","content":content,"thread_id":thread_id,"turn_id":turn_id}),
                )
                .await?;
                return Ok(json!({
                    "content": content,
                    "codex": {"profile_id":profile_id,"thread_id":thread_id,"turn_id":turn_id,"model":model}
                }));
            }
            "error" => {
                let message = params
                    .get("error")
                    .and_then(|value| value.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Codex runtime error");
                bail!("{message}");
            }
            _ => {}
        }
    }
}

async fn wait_for_response(
    daemon: &Daemon,
    run_id: Uuid,
    protocol: &mut CodexProtocol,
    expected_id: u64,
) -> Result<Value> {
    loop {
        let payload = next_payload(daemon, run_id, protocol).await?;
        if let (Some(request_id), Some(method)) = (
            payload.get("id").and_then(Value::as_u64),
            payload.get("method").and_then(Value::as_str),
        ) {
            if method.ends_with("/requestApproval") {
                let approved = await_local_approval(
                    daemon,
                    run_id,
                    json!({
                        "adapter": "codex",
                        "method": method,
                        "request_id": request_id,
                        "details": payload.get("params").cloned().unwrap_or(Value::Null),
                    }),
                )
                .await?;
                protocol
                    .respond(
                        request_id,
                        json!({"decision": if approved { "accept" } else { "decline" }}),
                    )
                    .await?;
            } else {
                protocol.respond(request_id, Value::Null).await?;
            }
            continue;
        }
        if payload.get("id").and_then(Value::as_u64) != Some(expected_id) {
            continue;
        }
        if let Some(error) = payload.get("error").filter(|value| !value.is_null()) {
            bail!("Codex App Server request failed: {error}");
        }
        return Ok(payload.get("result").cloned().unwrap_or(Value::Null));
    }
}

async fn next_payload(
    daemon: &Daemon,
    run_id: Uuid,
    protocol: &mut CodexProtocol,
) -> Result<Value> {
    loop {
        tokio::select! {
            line = protocol.lines.next_line() => {
                let line = line?.context("Codex App Server stopped before completing the turn")?;
                if line.len() > MAX_PROTOCOL_LINE {
                    bail!("Codex protocol response exceeds 16 MiB");
                }
                return serde_json::from_str(&line).context("Codex emitted invalid JSONL");
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if local_run_is_canceled(daemon, run_id).await? {
                    protocol.process_tree.terminate();
                    let _ = protocol.child.kill().await;
                    let database = daemon.database.lock().await;
                    if let Err(error) = set_local_run_state_locked(&database, run_id, RunState::Canceled) {
                        tracing::warn!(?error, "failed to retain canceled Codex run state");
                    }
                    bail!("Codex run was canceled");
                }
            }
        }
    }
}

fn required_string<'a>(value: Option<&'a Value>, name: &str) -> Result<&'a str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("Codex {name} is required"))
}

fn optional_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn is_codex_tool_item(item_type: &str) -> bool {
    matches!(
        item_type,
        "commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall" | "webSearch"
    )
}

fn validate_profile_id(profile_id: &str) -> Result<()> {
    if profile_id.len() > 128
        || !profile_id
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_'))
    {
        bail!("invalid Codex profile identifier");
    }
    Ok(())
}

fn verified_runtime(root: &Path) -> Result<PathBuf> {
    let manifest_path = root.join("runtime-bundle-manifest.json");
    let manifest: RuntimeManifest =
        serde_json::from_slice(&std::fs::read(&manifest_path).with_context(|| {
            format!(
                "Codex runtime manifest is missing: {}",
                manifest_path.display()
            )
        })?)?;
    if manifest.version != CODEX_VERSION || manifest.protocol_schema != PROTOCOL_SCHEMA {
        bail!("Codex bundle/schema does not match the pinned runtime");
    }
    for relative in [&manifest.binary, &manifest.license] {
        let path = Path::new(relative);
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            bail!("Codex manifest contains an unsafe resource path");
        }
        if !root.join(path).is_file() {
            bail!("Codex runtime resource is missing: {relative}");
        }
    }
    let binary = root.join(&manifest.binary);
    let actual = Sha256::digest(std::fs::read(&binary)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if !actual.eq_ignore_ascii_case(manifest.sha256.trim()) {
        bail!("Codex executable failed SHA-256 verification");
    }
    Ok(binary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_pool_reuses_and_serializes_each_profile_independently() {
        let pool = CodexSessionPool::default();
        let first = pool.session("profile-a").await;
        let reused = pool.session("profile-a").await;
        let other = pool.session("profile-b").await;

        assert!(Arc::ptr_eq(&first, &reused));
        assert!(!Arc::ptr_eq(&first, &other));

        let first_guard = first.lock().await;
        assert!(reused.try_lock().is_err());
        assert!(other.try_lock().is_ok());
        drop(first_guard);
        assert!(reused.try_lock().is_ok());
    }

    #[test]
    fn stderr_capture_retains_only_the_latest_bounded_output() {
        let mut captured = b"1234".to_vec();
        append_bounded(&mut captured, b"5678", 6);
        assert_eq!(captured, b"345678");

        append_bounded(&mut captured, b"abcdefgh", 6);
        assert_eq!(captured, b"cdefgh");
    }

    #[test]
    fn profile_ids_cannot_escape_the_profile_root() {
        assert!(validate_profile_id("codex-account_1").is_ok());
        assert!(validate_profile_id("../account").is_err());
        assert!(validate_profile_id("account/profile").is_err());
    }

    #[test]
    fn only_action_items_are_exposed_as_codex_tool_events() {
        for item_type in [
            "commandExecution",
            "fileChange",
            "mcpToolCall",
            "dynamicToolCall",
            "webSearch",
        ] {
            assert!(is_codex_tool_item(item_type));
        }
        for item_type in ["agentMessage", "reasoning", "plan", "contextCompaction"] {
            assert!(!is_codex_tool_item(item_type));
        }
    }
}
