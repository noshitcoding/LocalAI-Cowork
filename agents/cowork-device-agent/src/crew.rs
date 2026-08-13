use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use cowork_contracts::{ExecutorKind, RunEvent, RunEventKind, RunLease, SCHEMA_VERSION};
use cowork_runtime::crew::{
    apply_crew_agent_tool_policy, crew_protocol_run_event_kind, prepare_crew_request,
    CrewModelConfig,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

use super::{
    inventory_workspace, materialize_run_workspace, publish_result_snapshot,
    redact_executor_secret_value, redact_executor_secrets, selected_executor_mcp_names,
    workspace_diff_summary, Config, ControlPlaneClient, ExecutorMcpBinding, LeaseExecution,
    ManagedMcpProcessJob,
};

pub(crate) const EXPECTED_CREWAI_VERSION: &str = "1.15.8";
const EXPECTED_RUNTIME_SCHEMA_VERSION: u64 = 2;
const MAX_CREW_LINE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CREW_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_CREW_STDERR_BYTES: usize = 8 * 1024 * 1024;
const MAX_CREW_EVENTS: usize = 1_000;
const MAX_CREW_EVENT_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct CrewRuntimeConfig {
    python: PathBuf,
    script: PathBuf,
}

pub(crate) fn runtime_from_env(
    kind: ExecutorKind,
    advertises_crew: bool,
    has_model_endpoint: bool,
) -> Result<Option<CrewRuntimeConfig>> {
    if kind != ExecutorKind::ManagedWindows || !advertises_crew {
        return Ok(None);
    }
    if !has_model_endpoint {
        bail!("managed Windows crew.python execution requires COWORK_MODEL_BASE_URL");
    }
    let python = required_runtime_path("COWORK_CREW_PYTHON")?;
    let script = required_runtime_path("COWORK_CREW_SCRIPT")?;
    Ok(Some(CrewRuntimeConfig { python, script }))
}

fn required_runtime_path(name: &str) -> Result<PathBuf> {
    let path = env::var_os(name)
        .map(PathBuf::from)
        .with_context(|| format!("managed Windows crew.python requires {name}"))?;
    if !path.is_absolute() {
        bail!("{name} must use an absolute path");
    }
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("failed to inspect {name} at {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("{name} must reference a regular non-symlink file");
    }
    Ok(path)
}

pub(crate) async fn verify_runtime(runtime: &CrewRuntimeConfig) -> Result<()> {
    let mut command = tokio::process::Command::new(&runtime.python);
    command
        .arg(&runtime.script)
        .arg("status")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_minimal_windows_environment(&mut command);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().with_context(|| {
        format!(
            "failed to start Crew runtime status check {}",
            runtime.python.display()
        )
    })?;
    let process_job = match ManagedMcpProcessJob::attach(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error).context("failed to isolate the Crew runtime status process tree");
        }
    };
    let stdout = child
        .stdout
        .take()
        .context("Crew runtime status stdout is missing")?;
    let stderr = child
        .stderr
        .take()
        .context("Crew runtime status stderr is missing")?;
    let stdout_task = tokio::spawn(read_bounded_bytes(stdout, MAX_CREW_EVENT_BYTES));
    let stderr_task = tokio::spawn(read_bounded_bytes(stderr, MAX_CREW_EVENT_BYTES));
    let status = match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            bail!("Crew runtime status check timed out");
        }
    };
    process_job.close();
    let (stdout, stdout_truncated) = stdout_task
        .await
        .context("Crew status stdout reader failed")??;
    let (stderr, stderr_truncated) = stderr_task
        .await
        .context("Crew status stderr reader failed")??;
    if stdout_truncated || stderr_truncated {
        bail!("Crew runtime status output exceeds 1 MiB");
    }
    if !status.success() {
        bail!(
            "Crew runtime status check failed with {}: {}",
            status,
            String::from_utf8_lossy(&stderr).trim()
        );
    }
    let status: Value = serde_json::from_slice(&stdout)
        .context("Crew runtime status check returned invalid JSON")?;
    if status.get("runtimeCompatible").and_then(Value::as_bool) != Some(true)
        || status.get("crewaiVersion").and_then(Value::as_str) != Some(EXPECTED_CREWAI_VERSION)
        || status.get("runtimeSchemaVersion").and_then(Value::as_u64)
            != Some(EXPECTED_RUNTIME_SCHEMA_VERSION)
    {
        bail!(
            "Crew runtime is not the required compatible CrewAI {EXPECTED_CREWAI_VERSION} schema {EXPECTED_RUNTIME_SCHEMA_VERSION} build"
        );
    }
    Ok(())
}

pub(crate) async fn execute_managed_run(
    client: &ControlPlaneClient,
    config: &Config,
    lease: &RunLease,
) -> Result<LeaseExecution> {
    let runtime = config
        .crew_runtime
        .as_ref()
        .context("this managed Windows executor does not have a verified Crew runtime")?;
    verify_runtime(runtime).await?;
    let base_url = config
        .model_base_url
        .clone()
        .context("managed Windows Crew execution requires a model endpoint")?;
    let model = CrewModelConfig {
        base_url,
        api_key: config.model_api_key.clone(),
        model: config.model_name.clone(),
        timeout: Duration::from_secs(20 * 60),
        verify_tls_certificates: true,
    };
    let definition = lease
        .run
        .spec
        .input
        .get("crew_definition")
        .cloned()
        .context("the managed Windows Crew run has no frozen crew_definition")?;
    let selected = selected_executor_mcp_names(&lease.run.spec.input)?;
    let bindings = config
        .mcp_bindings
        .iter()
        .filter(|binding| selected.binary_search(&binding.name).is_ok())
        .collect::<Vec<_>>();
    if bindings.len() != selected.len() {
        bail!("this managed Windows executor does not have every Crew MCP binding");
    }

    let workspace = if lease.run.spec.snapshot_id.is_some() {
        materialize_run_workspace(client, config, lease).await?
    } else {
        let path = config.workspace_root.join(lease.run.spec.id.to_string());
        if path.parent() != Some(config.workspace_root.as_path()) {
            bail!("refusing a managed Crew workspace outside the configured root");
        }
        tokio::fs::create_dir_all(&path).await?;
        path
    };
    let before = inventory_workspace(&workspace).await?;
    let mut request = prepare_crew_request(definition, &lease.run.spec, &model)?;
    request
        .as_object_mut()
        .context("the prepared Crew request must be an object")?
        .insert(
            "cwd".to_owned(),
            Value::String(workspace.to_string_lossy().into_owned()),
        );
    let mut secrets = config
        .model_api_key
        .clone()
        .filter(|secret| !secret.is_empty())
        .into_iter()
        .collect::<Vec<_>>();
    secrets.extend(inject_executor_mcp_context(
        &mut request,
        &bindings,
        lease.run.spec.creator_user_id,
        &config.capabilities,
    )?);

    client
        .create_checkpoint(
            lease,
            Uuid::new_v4(),
            false,
            json!({
                "phase":"managed_windows_crew_dispatched",
                "adapter":"crewai",
                "crew_id":lease.run.spec.input.get("crew_id"),
                "crew_revision":lease.run.spec.input.get("crew_revision"),
            }),
        )
        .await?;
    append_event(
        client,
        lease,
        RunEventKind::ModelStarted,
        json!({"adapter":"crewai","runtime":"managed_windows","model":config.model_name}),
        &secrets,
    )
    .await?;

    let mut command = tokio::process::Command::new(&runtime.python);
    command
        .arg(&runtime.script)
        .arg("execute")
        .current_dir(&workspace)
        .env_clear()
        .env("LITELLM_LOCAL_MODEL_COST_MAP", "True")
        .env(
            "COWORK_MCP_TOOL_COMMAND_JSON",
            serde_json::to_string(&vec![
                env::current_exe()?.to_string_lossy().into_owned(),
                "executor-mcp-tool".to_owned(),
            ])?,
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if config
        .capabilities
        .iter()
        .any(|capability| capability.name.0 == "office.microsoft")
    {
        command.env(
            "COWORK_WINDOWS_OFFICE_COMMAND_JSON",
            serde_json::to_string(&vec![
                env::current_exe()?.to_string_lossy().into_owned(),
                "executor-windows-office".to_owned(),
            ])?,
        );
    }
    configure_minimal_windows_environment(&mut command);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().with_context(|| {
        format!(
            "failed to start managed Windows Crew runtime {}",
            runtime.python.display()
        )
    })?;
    let process_job = match ManagedMcpProcessJob::attach(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error).context("failed to isolate the managed Windows Crew process tree");
        }
    };
    let mut stdin = child
        .stdin
        .take()
        .context("Crew runtime stdin is missing")?;
    stdin.write_all(&serde_json::to_vec(&request)?).await?;
    stdin.shutdown().await?;
    drop(stdin);
    let stdout = child
        .stdout
        .take()
        .context("Crew runtime stdout is missing")?;
    let stderr = child
        .stderr
        .take()
        .context("Crew runtime stderr is missing")?;
    let stderr_task = tokio::spawn(read_bounded_stream(stderr, MAX_CREW_STDERR_BYTES));
    let mut reader = BufReader::new(stdout);
    let mut response = None;
    let mut emitted_events = 0_usize;
    let mut output_bytes = 0_usize;
    let timeout = model.timeout.saturating_add(Duration::from_secs(60));
    let started = Instant::now();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            line = read_bounded_line(&mut reader, MAX_CREW_LINE_BYTES) => {
                let Some(line) = line? else { break; };
                output_bytes = output_bytes.saturating_add(line.len());
                if output_bytes > MAX_CREW_OUTPUT_BYTES {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    bail!("Crew runtime output exceeds 32 MiB");
                }
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let mut value: Value = serde_json::from_slice(&line)
                    .context("Crew runtime returned invalid JSON")?;
                redact_executor_secret_value(&mut value, &secrets);
                if value.get("localAiCoworkEvent").is_some() {
                    if emitted_events < MAX_CREW_EVENTS
                        && serde_json::to_vec(&value)?.len() <= MAX_CREW_EVENT_BYTES
                    {
                        let event_kind = crew_protocol_run_event_kind(&value);
                        append_event(
                            client,
                            lease,
                            event_kind,
                            json!({"adapter":"crewai","crew_event":value}),
                            &secrets,
                        )
                        .await?;
                        emitted_events += 1;
                    }
                } else if response.replace(value).is_some() {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    bail!("Crew runtime returned more than one response");
                }
            }
            _ = &mut deadline => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                bail!("Crew runtime exceeded its configured timeout of {} seconds", timeout.as_secs());
            }
        }
    }
    let remaining = timeout.saturating_sub(started.elapsed());
    let status = match tokio::time::timeout(remaining, child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            bail!(
                "Crew runtime exceeded its configured timeout of {} seconds",
                timeout.as_secs()
            );
        }
    };
    process_job.close();
    let stderr = stderr_task.await.context("Crew stderr reader failed")??;
    let stderr = redact_executor_secrets(&stderr, &secrets);
    if !status.success() {
        bail!(
            "Crew runtime failed with {status}: {}",
            stderr.chars().take(8_000).collect::<String>()
        );
    }
    let mut response = response.context("Crew runtime returned no response")?;
    redact_executor_secret_value(&mut response, &secrets);
    let status_name = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("failed");
    if status_name != "completed" {
        bail!(
            "Crew runtime ended in state {status_name}: {}",
            response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown CrewAI error")
        );
    }
    let content = response
        .get("taskResults")
        .or_else(|| response.get("task_results"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("output").and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    append_event(
        client,
        lease,
        RunEventKind::ModelCompleted,
        json!({
            "adapter":"crewai",
            "content":content,
            "task_count":response.get("taskResults").and_then(Value::as_array).map_or(0, Vec::len),
            "event_count":emitted_events,
            "usage":response.get("usage").cloned().unwrap_or(Value::Null),
        }),
        &secrets,
    )
    .await?;
    client
        .create_checkpoint(
            lease,
            Uuid::new_v4(),
            true,
            json!({"phase":"managed_windows_crew_completed","adapter":"crewai"}),
        )
        .await?;

    let after = inventory_workspace(&workspace).await?;
    let diff_summary = workspace_diff_summary(Some(&before), &after);
    let result_snapshot_manifest_id = if before.fingerprints == after.fingerprints {
        None
    } else {
        Some(publish_result_snapshot(client, lease, &after).await?)
    };
    Ok(LeaseExecution {
        result: json!({"content":content,"crew_response":response}),
        result_snapshot_manifest_id,
        result_diff_summary: diff_summary,
    })
}

fn configure_minimal_windows_environment(command: &mut tokio::process::Command) {
    for name in [
        "SystemRoot",
        "WINDIR",
        "PATH",
        "PATHEXT",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
    ] {
        if let Some(value) = env::var_os(name) {
            command.env(name, value);
        }
    }
}

fn inject_executor_mcp_context(
    request: &mut Value,
    bindings: &[&ExecutorMcpBinding],
    creator_user_id: Uuid,
    capabilities: &[cowork_contracts::CapabilityDescriptor],
) -> Result<Vec<String>> {
    let agents = request
        .get_mut("agents")
        .and_then(Value::as_array_mut)
        .context("the prepared Crew request must contain agents")?;
    let mut requested_names = BTreeSet::new();
    let mut agent_access = Vec::with_capacity(agents.len());
    for agent in agents {
        let agent = agent
            .as_object_mut()
            .context("prepared Crew agents must be objects")?;
        let agent_id = agent
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("prepared Crew agents require an id")?
            .to_owned();
        let allowed_tools = apply_crew_agent_tool_policy(agent, |required| {
            capabilities
                .iter()
                .any(|capability| capability.name.0 == required)
        })?;
        let mut allowed_names = BTreeSet::new();
        if let Some(values) = agent.get("mcpServerNames") {
            for value in values
                .as_array()
                .context("prepared Crew agent mcpServerNames must be an array")?
            {
                let name = value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .context("prepared Crew agent MCP names must be strings")?;
                allowed_names.insert(name.to_owned());
                requested_names.insert(name.to_owned());
            }
        }
        agent_access.push(json!({
            "agentId":agent_id,
            "allowedTools":allowed_tools,
            "blockedTools":[],
            "allowedMcpServerNames":allowed_names,
            "blockedMcpServerNames":[],
            "delegationAllowed":false,
            "gatewayHints":[],
        }));
    }

    let by_name = bindings
        .iter()
        .map(|binding| (binding.name.as_str(), *binding))
        .collect::<BTreeMap<_, _>>();
    let resolved_names = by_name
        .keys()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    if requested_names != resolved_names {
        bail!("executor Crew MCP bindings do not match the prepared agent allowlists");
    }
    let mut executor_bindings = Vec::with_capacity(requested_names.len());
    let mut secrets = Vec::new();
    for name in &requested_names {
        let binding = by_name
            .get(name.as_str())
            .with_context(|| format!("Crew MCP binding {name:?} is unavailable"))?;
        secrets.extend(binding.secret_values().cloned());
        executor_bindings.push(serde_json::to_value(binding)?);
    }
    let request = request
        .as_object_mut()
        .context("the prepared Crew request must be an object")?;
    request.insert(
        "executorMcpBindings".to_owned(),
        Value::Array(executor_bindings),
    );
    request.insert(
        "governance".to_owned(),
        json!({
            "subject":format!("managed-windows-run:{creator_user_id}"),
            "subjectRoles":["runner"],
            "policyStrict":true,
            "denyRules":[],
            "pendingApprovalTypes":[],
            "agentAccess":agent_access,
        }),
    );
    Ok(secrets)
}

async fn append_event(
    client: &ControlPlaneClient,
    lease: &RunLease,
    kind: RunEventKind,
    mut payload: Value,
    secrets: &[String],
) -> Result<()> {
    redact_executor_secret_value(&mut payload, secrets);
    client
        .append_event(
            lease,
            &RunEvent {
                schema_version: SCHEMA_VERSION,
                run_id: lease.run.spec.id,
                sequence: 0,
                event_id: Uuid::new_v4(),
                kind,
                payload,
                created_at: chrono::Utc::now(),
            },
        )
        .await
}

async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(take) > limit {
            bail!("Crew runtime output line exceeds {limit} bytes");
        }
        let complete = available[take - 1] == b'\n';
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if complete {
            break;
        }
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    Ok(Some(line))
}

async fn read_bounded_stream<R: AsyncRead + Unpin>(mut reader: R, limit: usize) -> Result<String> {
    let (stored, truncated) = read_bounded_bytes(&mut reader, limit).await?;
    let mut value = String::from_utf8_lossy(&stored).into_owned();
    if truncated {
        value.push_str("\n[stderr truncated]");
    }
    Ok(value)
}

async fn read_bounded_bytes<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> Result<(Vec<u8>, bool)> {
    let mut stored = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(stored.len());
        stored.extend_from_slice(&buffer[..read.min(remaining)]);
        truncated |= read > remaining;
    }
    Ok((stored, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExecutorMcpTransport;
    use std::collections::BTreeMap;

    #[test]
    fn executor_crew_mcp_context_is_exact_agent_scoped_and_secret_bearing() {
        let mut request = json!({
            "agents":[
                {"id":"researcher","tools":["read_file","office_workflow"],"mcpServerNames":["Docs"]},
                {"id":"reviewer","mcpServerNames":[]}
            ]
        });
        let binding = ExecutorMcpBinding {
            name: "Docs".to_owned(),
            transport: ExecutorMcpTransport::Stdio,
            command: r"C:\MCP\docs.exe".to_owned(),
            args: vec!["--stdio".to_owned()],
            environment: BTreeMap::from([(
                "DOCS_TOKEN".to_owned(),
                "executor-crew-secret".to_owned(),
            )]),
            url: String::new(),
            headers: BTreeMap::new(),
        };
        let capabilities = [
            cowork_contracts::CapabilityDescriptor {
                schema_version: SCHEMA_VERSION,
                name: cowork_contracts::Capability::from("files"),
                version: "test".to_owned(),
                attributes: BTreeMap::new(),
            },
            cowork_contracts::CapabilityDescriptor {
                schema_version: SCHEMA_VERSION,
                name: cowork_contracts::Capability::from("office.ooxml"),
                version: "test".to_owned(),
                attributes: BTreeMap::new(),
            },
        ];
        let secrets =
            inject_executor_mcp_context(&mut request, &[&binding], Uuid::nil(), &capabilities)
                .unwrap();
        assert_eq!(secrets, vec!["executor-crew-secret"]);
        assert_eq!(
            request["governance"]["agentAccess"][0]["allowedMcpServerNames"],
            json!(["Docs"])
        );
        assert_eq!(
            request["governance"]["agentAccess"][1]["allowedMcpServerNames"],
            json!([])
        );
        assert_eq!(request["executorMcpBindings"][0]["name"], "Docs");
        assert_eq!(
            request["governance"]["agentAccess"][0]["allowedTools"],
            json!(["read_file", "office_workflow"])
        );
        assert_eq!(request["agents"][0]["allowDelegation"], false);

        let mut mismatched = json!({"agents":[{"id":"researcher","mcpServerNames":["CRM"]}]});
        assert!(inject_executor_mcp_context(
            &mut mismatched,
            &[&binding],
            Uuid::nil(),
            &capabilities,
        )
        .is_err());
    }

    #[tokio::test]
    async fn crew_output_reader_rejects_an_oversized_line() {
        let input = vec![b'x'; 17];
        let mut reader = BufReader::with_capacity(4, input.as_slice());
        let error = read_bounded_line(&mut reader, 16).await.unwrap_err();
        assert!(error.to_string().contains("exceeds 16 bytes"));
    }
}
