//! Private HTTP and stdio MCP bindings; no server/team credential inheritance.
use super::*;
use cowork_mcp::{Endpoint, Limits, Session};
#[path = "execution_mcp_interactions.rs"]
pub(super) mod interactions;
#[path = "execution_mcp_journal.rs"]
mod journal;
#[path = "execution_mcp_stdio.rs"]
mod stdio;
#[path = "execution_mcp_tasks.rs"]
mod tasks;
impl ExecutionEngine {
    pub fn mcp_journal_inspect(data: &Path, run: Uuid, source: Uuid) -> Result<Value> {
        journal::inspect(data, run, source)
    }
    pub fn mcp_journal_confirm_key(data: &Path) -> Result<Value> {
        journal::confirm(data)
    }
}

pub(super) fn validate(bindings: &[McpBinding]) -> Result<()> {
    let mut names = BTreeSet::new();
    for binding in bindings {
        ensure!(
            !binding.name.is_empty()
                && binding.name.len() <= 80
                && binding
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                && names.insert(&binding.name),
            "device_mcp_binding_name_invalid"
        );
        ensure!(
            binding.allowed_tools.len() <= 128
                && binding.allowed_tools.iter().collect::<BTreeSet<_>>().len()
                    == binding.allowed_tools.len(),
            "device_mcp_tool_allowlist_invalid"
        );
        if let Some(pin) = &binding.catalog_sha256 {
            ensure!(
                pin.len() == 64 && pin.bytes().all(|b| b.is_ascii_hexdigit()),
                "device_mcp_catalog_pin_invalid"
            );
        }
        match binding.transport {
            McpTransport::StreamableHttp => {
                Endpoint::new(&binding.endpoint, binding.bearer.as_deref())
                    .map_err(|_| anyhow::anyhow!("device_mcp_http_configuration_invalid"))?;
                ensure!(
                    binding.command.is_none()
                        && binding.arguments.is_empty()
                        && binding.environment.is_empty(),
                    "device_mcp_mixed_transports_invalid"
                );
            }
            McpTransport::Stdio => {
                let command = binding
                    .command
                    .as_ref()
                    .context("device_mcp_command_missing")?;
                ensure!(
                    command.is_absolute()
                        && command.is_file()
                        && command
                            .extension()
                            .is_none_or(|e| !e.eq_ignore_ascii_case("cmd")
                                && !e.eq_ignore_ascii_case("bat")),
                    "device_mcp_command_invalid"
                );
                ensure!(
                    binding.endpoint.is_empty()
                        && binding.bearer.is_none()
                        && binding.arguments.len() <= 128
                        && binding.arguments.iter().map(String::len).sum::<usize>() <= 32000
                        && !binding.arguments.iter().any(|a| a.contains('\0')),
                    "device_mcp_stdio_configuration_invalid"
                );
                ensure!(
                    binding.environment.len() <= 64
                        && binding.environment.iter().all(|(k, v)| !k.is_empty()
                            && !k.contains(['=', '\0'])
                            && !v.contains('\0'))
                        && binding
                            .environment
                            .iter()
                            .map(|(k, v)| k.len() + v.len())
                            .sum::<usize>()
                            <= 32000,
                    "device_mcp_environment_invalid"
                );
            }
        }
    }
    Ok(())
}

pub(super) async fn call(
    engine: &ExecutionEngine,
    control: &dyn DeviceControl,
    run: Uuid,
    lease: Uuid,
    source: Uuid,
    args: Value,
    stop: &CancellationToken,
) -> Result<Value> {
    let connection = args["connection"]
        .as_str()
        .context("device_mcp_connection_missing")?;
    let binding = engine
        .config
        .mcp_bindings
        .iter()
        .find(|b| b.name == connection)
        .context("device_mcp_connection_not_configured")?;
    let name = args["name"].as_str().context("device_mcp_tool_missing")?;
    ensure!(
        binding.allowed_tools.is_empty() || binding.allowed_tools.iter().any(|n| n == name),
        "device_mcp_tool_not_allowed"
    );
    let arguments = args["arguments"].clone();
    ensure!(arguments.is_object(), "device_mcp_arguments_invalid");
    match binding.transport {
        McpTransport::StreamableHttp => {
            http_call(
                engine, control, run, lease, source, binding, name, arguments, stop,
            )
            .await
        }
        McpTransport::Stdio => {
            stdio_call(
                engine, control, run, lease, source, binding, name, arguments, stop,
            )
            .await
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "HTTP MCP dispatch binds the selected connection and tool arguments to the run lease, original request and cancellation token."
)]
async fn http_call(
    engine: &ExecutionEngine,
    control: &dyn DeviceControl,
    run: Uuid,
    lease: Uuid,
    source: Uuid,
    binding: &McpBinding,
    name: &str,
    arguments: Value,
    stop: &CancellationToken,
) -> Result<Value> {
    let endpoint = Endpoint::new(&binding.endpoint, binding.bearer.as_deref())
        .map_err(|_| anyhow::anyhow!("device_mcp_configuration_invalid"))?;
    let connection = async {
        if interactions::sampling_available() {
            Session::connect_interactive(endpoint, Limits::default()).await
        } else {
            Session::connect_form_interactive(endpoint, Limits::default()).await
        }
    };
    let mut session = tokio::select! {_=stop.cancelled()=>bail!("device_run_canceled"),s=connection=>s.map_err(|_|anyhow::anyhow!("device_mcp_connection_failed"))?};
    session_call(
        engine,
        control,
        run,
        lease,
        source,
        binding,
        name,
        arguments,
        &mut session,
        stop,
    )
    .await
}
#[expect(
    clippy::too_many_arguments,
    reason = "MCP session dispatch checks the catalog and journals the original request under the explicit run, lease, session and cancellation authority."
)]
async fn session_call(
    engine: &ExecutionEngine,
    control: &dyn DeviceControl,
    run: Uuid,
    lease: Uuid,
    source: Uuid,
    binding: &McpBinding,
    name: &str,
    arguments: Value,
    session: &mut Session,
    stop: &CancellationToken,
) -> Result<Value> {
    let catalog = tokio::select! {_=stop.cancelled()=>bail!("device_run_canceled"),c=session.discover()=>c.map_err(|_|anyhow::anyhow!("device_mcp_catalog_failed"))?};
    let sha = catalog.sha256().to_owned();
    ensure!(
        binding
            .catalog_sha256
            .as_ref()
            .is_none_or(|pin| pin == &sha),
        "device_mcp_catalog_pin_changed"
    );
    let definition = catalog
        .tools()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| t.definition().clone())
        .context("device_mcp_tool_missing_in_catalog")?;
    let prepared = session
        .prepare(&sha, name, arguments.clone())
        .map_err(|_| anyhow::anyhow!("device_mcp_arguments_failed_schema"))?;
    let action = json!({"tool":"mcp_call","connection":binding.name,"endpoint":binding.endpoint,"command":binding.command,"name":name,"arguments":arguments,"definition":definition,"catalog_sha256":sha,"request_sha256":prepared.request_sha256(),"transport":match binding.transport{McpTransport::StreamableHttp=>"streamable_http",McpTransport::Stdio=>"stdio"},"native_owner_rights":matches!(binding.transport,McpTransport::Stdio)});
    if !engine
        .approval(control, run, lease, source, action.clone(), stop)
        .await?
    {
        return Ok(json!({"state":"rejected","performed":false}));
    }
    let journal = journal::Journal::open(engine, run, source, binding)?;
    journal.put("call/intent","task_response",&json!({"action":action,"connection_id":session.connection_id(),"epoch":session.epoch(),"protocol":session.protocol_version(),"rpc_id":prepared.rpc_request_id(),"request_sha256":prepared.request_sha256(),"lease_sha256":digest(lease.as_bytes())}))?;
    engine.admit(
        run,
        &source.to_string(),
        json!({"phase":"effect_dispatched","action":action}),
    )?;
    engine.checkpoint(control,run,lease,source,json!({"tool":"mcp_call","connection":binding.name,"request_sha256":prepared.request_sha256(),"catalog_sha256":sha})).await?;
    interactions::current(
        control,
        run,
        lease,
        chrono::Utc::now().timestamp_millis() + 120000,
        stop,
    )
    .await?;
    let result = tokio::select! {_=stop.cancelled()=>bail!("device_mcp_canceled_effect_outcome_unknown_no_replay"),r=session.call(prepared)=>r.map_err(|e|anyhow::anyhow!("device_mcp_call_failed_{}_uncertain_{}_no_replay",e.code,e.outcome_uncertain))?};
    journal.put("call/received","task_response",&json!({"response":result.response,"response_sha256":result.response_sha256,"task":result.task}))?;
    let result = if result.task.is_some() {
        tasks::await_task(
            engine, control, run, lease, source, binding, session, result, &journal, stop,
        )
        .await?
    } else {
        result
    };
    engine.admit(run,&format!("{source}/settled"),json!({"phase":"effect_settled","response_sha256":result.response_sha256,"catalog_changed":result.catalog_changed}))?;
    Ok(engine.redact_value(json!({"state":"completed","connection":binding.name,"name":name,"response":result.response,"response_sha256":result.response_sha256,"catalog_changed":result.catalog_changed})))
}

#[expect(
    clippy::too_many_arguments,
    reason = "Owned stdio MCP dispatch must bind process lifetime and tool arguments to the selected connection, run lease and original request."
)]
async fn stdio_call(
    engine: &ExecutionEngine,
    control: &dyn DeviceControl,
    run: Uuid,
    lease: Uuid,
    source: Uuid,
    binding: &McpBinding,
    name: &str,
    arguments: Value,
    stop: &CancellationToken,
) -> Result<Value> {
    let journal = journal::Journal::open(engine, run, source, binding)?;
    journal.put("stdio/process-intent","task_checkpoint",&json!({"command":binding.command,"arguments":binding.arguments,"connection":binding.name,"original_config_sha256":digest(&serde_json::to_vec(&json!({"command":binding.command,"arguments":binding.arguments,"environment":binding.environment}))?)}))?;
    let mut owned = stdio::Owned::open(engine, run, source, binding, stop).await?;
    let result=async{
        let endpoint=owned.endpoint.take().context("device_mcp_original_pipe_missing")?;
        let connection=async{if interactions::sampling_available(){Session::connect_interactive(endpoint,Limits::default()).await}else{Session::connect_form_interactive(endpoint,Limits::default()).await}};
        let mut session=tokio::select!{_=stop.cancelled()=>bail!("device_run_canceled"),r=connection=>r.map_err(|_|anyhow::anyhow!("device_mcp_original_stdio_initialize_failed"))?};
        session_call(engine,control,run,lease,source,binding,name,arguments,&mut session,stop).await
    }.await;
    let closed = owned.close(engine, run).await;
    if let Ok(receipt) = &closed {
        journal.put("stdio/source-closure", "task_checkpoint", receipt)?;
    }
    match (result, closed) {
        (Ok(result), Ok(_)) => Ok(result),
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(error),
    }
}
