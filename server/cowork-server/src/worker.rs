use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use cowork_contracts::{
    Capability, RunError, RunEventKind, RunLease, SandboxImage, SandboxLimits, SandboxNetwork,
    SandboxRunResult, SandboxRunSpec,
};
use cowork_runtime::{
    crew::{prepare_crew_request, CrewModelConfig},
    AgentRuntime, ModelConfig as AgentModelConfig, RuntimeHost, ToolDefinition, ToolInvocation,
    ToolOutput,
};
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::{Client, Response};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

use crate::{
    config::Config,
    db, desktop, governance,
    mcp_bindings::{self, ResolvedServerMcpBinding},
    providers, storage, workflow,
};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct WorkerRuntime {
    http: Client,
    agent: Option<Arc<AgentRuntime>>,
    default_crew_model: Option<Arc<CrewModelConfig>>,
    runner: Option<RunnerConfig>,
    desktop_runner: Option<desktop::RunnerControl>,
    object_store: Option<storage::ObjectStore>,
    model_pricing: Option<ModelPricing>,
}

#[derive(Clone, Copy)]
struct ModelPricing {
    input_micros_per_million: u64,
    output_micros_per_million: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CrewUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

#[derive(Clone)]
struct RunnerConfig {
    url: String,
    signing_key: Vec<u8>,
}

pub async fn run(pool: PgPool, config: Config) -> Result<()> {
    let object_store = config
        .object_store
        .as_ref()
        .map(storage::ObjectStore::from_config)
        .transpose()
        .context("invalid object-store configuration")?;
    let runtime = WorkerRuntime {
        http: Client::builder()
            .timeout(Duration::from_secs(24 * 60 * 60))
            .build()
            .expect("reqwest client configuration is valid"),
        agent: config
            .model_base_url
            .clone()
            .map(|base_url| {
                AgentRuntime::new(AgentModelConfig {
                    base_url,
                    api_key: config.model_api_key.clone(),
                    model: config.model_name.clone(),
                    timeout: Duration::from_secs(24 * 60 * 60),
                    max_steps: 64,
                    verify_tls_certificates: true,
                })
            })
            .transpose()?
            .map(Arc::new),
        default_crew_model: config.model_base_url.clone().map(|base_url| {
            Arc::new(CrewModelConfig {
                base_url,
                api_key: config.model_api_key.clone(),
                model: config.model_name.clone(),
                timeout: Duration::from_secs(24 * 60 * 60),
                verify_tls_certificates: true,
            })
        }),
        runner: config
            .runner_url
            .clone()
            .zip(config.runner_signing_key.clone())
            .map(|(url, signing_key)| RunnerConfig {
                url,
                signing_key: signing_key.into_bytes(),
            }),
        desktop_runner: config
            .runner_url
            .clone()
            .zip(config.runner_signing_key.clone())
            .map(|(url, key)| desktop::RunnerControl::new(url, key)),
        object_store: object_store.clone(),
        model_pricing: config
            .model_input_cost_micros_per_million
            .zip(config.model_output_cost_micros_per_million)
            .map(
                |(input_micros_per_million, output_micros_per_million)| ModelPricing {
                    input_micros_per_million,
                    output_micros_per_million,
                },
            ),
    };

    if runtime.agent.is_none() && runtime.runner.is_none() {
        tracing::warn!(
            "worker has neither a model endpoint nor a sandbox runner; server runs remain queued"
        );
    }
    tracing::info!(worker_id = %config.worker_id, "durable worker started");

    let mut interval = tokio::time::interval(config.worker_poll_interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut reap_counter = 0_u64;
    loop {
        interval.tick().await;
        reap_counter = reap_counter.wrapping_add(1);
        match workflow::trigger_due_schedules(
            &pool,
            &config.server_capabilities,
            chrono::Utc::now(),
            25,
        )
        .await
        {
            Ok(count) if count > 0 => tracing::info!(count, "triggered due schedules"),
            Err(error) => tracing::error!(?error, "failed to trigger due schedules"),
            _ => {}
        }
        if reap_counter % 10 == 0 {
            match db::interrupt_expired_leases(&pool).await {
                Ok(count) if count > 0 => tracing::warn!(count, "interrupted expired run leases"),
                Err(error) => tracing::error!(?error, "failed to reap expired run leases"),
                _ => {}
            }
            match desktop::reap_terminal_sessions(&pool, runtime.desktop_runner.as_ref()).await {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "cleaned up terminal run desktop sessions")
                }
                Err(error) => tracing::error!(?error, "failed to reap terminal desktop sessions"),
                _ => {}
            }
            match workflow::expire_pending_workflows(&pool, chrono::Utc::now()).await {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "expired pending approvals or inputs")
                }
                Err(error) => tracing::error!(?error, "failed to expire approvals or inputs"),
                _ => {}
            }
        }
        if reap_counter % config.maintenance_every_polls == 0 {
            match db::enforce_auth_session_retention(&pool, chrono::Utc::now(), 10_000).await {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "removed authentication sessions past retention")
                }
                Err(error) => tracing::error!(?error, "authentication retention failed"),
                _ => {}
            }
            match db::enforce_run_event_retention(&pool, chrono::Utc::now(), 10_000).await {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "removed run events past the 90-day retention window")
                }
                Err(error) => tracing::error!(?error, "run-event retention failed"),
                _ => {}
            }
            if let Some(store) = &object_store {
                match storage::garbage_collect(&pool, store, 100).await {
                    Ok(count) if count > 0 => {
                        tracing::info!(count, "garbage-collected snapshot chunks")
                    }
                    Err(error) => tracing::error!(?error, "snapshot garbage collection failed"),
                    _ => {}
                }
            }
        }
        match db::claim_server_run(
            &pool,
            config.worker_id,
            config.lease_duration.as_secs() as i64,
        )
        .await
        {
            Ok(Some(lease)) => {
                if let Err(error) = execute_run(
                    &pool,
                    config.worker_id,
                    config.lease_duration,
                    &runtime,
                    lease,
                )
                .await
                {
                    // A database/lease error is intentionally not retried here. The lease
                    // reaper will move the run to interrupted if ownership was lost.
                    tracing::error!(?error, "server run execution failed");
                }
            }
            Ok(None) => {}
            Err(error) => tracing::error!(?error, "failed to claim server run"),
        }
    }
}

async fn execute_run(
    pool: &PgPool,
    worker_id: Uuid,
    lease_duration: Duration,
    runtime: &WorkerRuntime,
    lease: RunLease,
) -> Result<()> {
    let heartbeat_pool = pool.clone();
    let heartbeat_lease = lease.clone();
    let heartbeat = tokio::spawn(async move {
        let every = (lease_duration / 3).max(Duration::from_secs(5));
        let mut interval = tokio::time::interval(every);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(error) = db::renew_lease(
                &heartbeat_pool,
                heartbeat_lease.run.spec.id,
                worker_id,
                heartbeat_lease.lease_token,
                lease_duration.as_secs() as i64,
            )
            .await
            {
                tracing::warn!(?error, run_id = %heartbeat_lease.run.spec.id, "worker lease heartbeat stopped");
                break;
            }
        }
    });
    let outcome = if lease.run.spec.input.get("sandbox").is_some() {
        execute_sandbox_run(pool, worker_id, lease_duration, runtime, &lease).await
    } else if lease
        .run
        .spec
        .input
        .get("task_runner")
        .and_then(Value::as_str)
        == Some("crew")
    {
        execute_crew_run(pool, worker_id, runtime, &lease).await
    } else {
        execute_agent_run(pool, worker_id, runtime, &lease).await
    };
    heartbeat.abort();
    if let Some(runner) = &runtime.desktop_runner {
        if let Err(error) = desktop::end_worker_sessions(pool, runner, lease.run.spec.id).await {
            tracing::warn!(?error, run_id = %lease.run.spec.id, "failed to clean up desktop sessions");
        }
    }

    match outcome {
        Ok(result) => {
            db::complete_leased_run(
                pool,
                lease.run.spec.id,
                worker_id,
                lease.lease_token,
                result,
                None,
                Value::Null,
            )
            .await?;
        }
        Err(error) => {
            let safe_to_resume = sqlx::query_scalar::<_, bool>(
                "SELECT safe_to_resume FROM run_checkpoints WHERE run_id = $1 ORDER BY sequence DESC LIMIT 1",
            )
            .bind(lease.run.spec.id)
            .fetch_optional(pool)
            .await?
            .unwrap_or(true);
            let run_error = RunError {
                code: if safe_to_resume {
                    "executor_operation_failed"
                } else {
                    "unsafe_tool_interrupted"
                }
                .to_owned(),
                message: error.to_string(),
                retryable: false,
                details: json!({"safe_to_resume": safe_to_resume}),
            };
            if safe_to_resume {
                db::fail_leased_run(
                    pool,
                    lease.run.spec.id,
                    worker_id,
                    lease.lease_token,
                    run_error,
                )
                .await?;
            } else {
                db::interrupt_leased_run(
                    pool,
                    lease.run.spec.id,
                    worker_id,
                    lease.lease_token,
                    run_error,
                )
                .await?;
            }
        }
    }
    Ok(())
}

async fn execute_agent_run(
    pool: &PgPool,
    worker_id: Uuid,
    runtime: &WorkerRuntime,
    lease: &RunLease,
) -> Result<Value> {
    let selected_agent = if let Some(profile_id) = lease.run.spec.model_profile_id {
        let profile = providers::resolve_server_provider(
            pool,
            runtime.object_store.as_ref(),
            lease.run.spec.creator_user_id,
            lease.run.spec.project_id,
            profile_id,
        )
        .await?;
        Some(AgentRuntime::new(AgentModelConfig {
            base_url: profile.base_url,
            api_key: profile.api_key,
            model: profile.model,
            timeout: profile.timeout,
            max_steps: profile.max_steps,
            verify_tls_certificates: profile.verify_tls_certificates,
        })?)
    } else {
        None
    };
    let agent = selected_agent
        .as_ref()
        .or(runtime.agent.as_deref())
        .context("the server model provider is not configured")?;
    let mcp_bindings = mcp_bindings::resolve_server_bindings_for_run(
        pool,
        runtime.object_store.as_ref(),
        lease.run.spec.project_id,
        &lease.run.spec.input,
    )
    .await?;
    let host = ServerRuntimeHost {
        pool,
        worker_id,
        runtime,
        lease,
        mcp_bindings,
    };
    let result = agent.execute(&lease.run.spec, &host).await?;
    Ok(serde_json::to_value(result)?)
}

async fn execute_crew_run(
    pool: &PgPool,
    worker_id: Uuid,
    runtime: &WorkerRuntime,
    lease: &RunLease,
) -> Result<Value> {
    let model = if let Some(profile_id) = lease.run.spec.model_profile_id {
        let profile = providers::resolve_server_provider(
            pool,
            runtime.object_store.as_ref(),
            lease.run.spec.creator_user_id,
            lease.run.spec.project_id,
            profile_id,
        )
        .await?;
        CrewModelConfig {
            base_url: profile.base_url,
            api_key: profile.api_key,
            model: profile.model,
            timeout: profile.timeout,
            verify_tls_certificates: profile.verify_tls_certificates,
        }
    } else {
        runtime
            .default_crew_model
            .as_deref()
            .context("the server Crew model provider is not configured")?
            .clone()
    };
    let definition = lease
        .run
        .spec
        .input
        .get("crew_definition")
        .cloned()
        .context("the Crew run has no frozen crew_definition")?;
    let mcp_bindings = mcp_bindings::resolve_server_bindings_for_run(
        pool,
        runtime.object_store.as_ref(),
        lease.run.spec.project_id,
        &lease.run.spec.input,
    )
    .await?;
    let mut request = prepare_crew_request(definition, &lease.run.spec, &model)?;
    let mut secret_redactions = model.api_key.clone().into_iter().collect::<Vec<_>>();
    secret_redactions.extend(inject_crew_mcp_context(
        &mut request,
        &mcp_bindings,
        lease.run.spec.creator_user_id,
    )?);
    let timeout_seconds = model
        .timeout
        .as_secs()
        .saturating_add(60)
        .clamp(60, 24 * 60 * 60);
    let spec = SandboxRunSpec {
        schema_version: cowork_contracts::SCHEMA_VERSION,
        run_id: lease.run.spec.id,
        image: SandboxImage::Crew,
        argv: vec![
            "python3".to_owned(),
            "/opt/cowork/crew-runtime/main.py".to_owned(),
            "execute".to_owned(),
        ],
        environment: Default::default(),
        stdin_base64: Some(STANDARD.encode(serde_json::to_vec(&request)?)),
        network: SandboxNetwork::FilteredEgress,
        limits: SandboxLimits {
            memory_bytes: 4 * 1024 * 1024 * 1024,
            cpu_nanos: 2_000_000_000,
            pids: 512,
            timeout_seconds,
            tmpfs_bytes: 1024 * 1024 * 1024,
            output_bytes: 32 * 1024 * 1024,
        },
    };
    governance::ensure_model_quota_for_run(pool, lease.run.spec.id).await?;
    workflow::create_worker_checkpoint(
        pool,
        worker_id,
        lease.run.spec.id,
        lease.lease_token,
        false,
        json!({
            "phase":"crew_dispatched",
            "adapter":"crewai",
            "crew_id":lease.run.spec.input.get("crew_id"),
            "crew_revision":lease.run.spec.input.get("crew_revision"),
        }),
    )
    .await?;
    db::append_leased_event(
        pool,
        lease.run.spec.id,
        worker_id,
        lease.lease_token,
        None,
        RunEventKind::ModelStarted,
        json!({"adapter":"crewai","runtime":"sandbox","model":model.model}),
    )
    .await?;

    let result = send_runner_job(runtime, &spec).await?;
    let stderr = redact_secrets(&result.stderr, &secret_redactions);
    if result.timed_out {
        bail!("Crew runtime exceeded its configured timeout");
    }
    if result.exit_code != Some(0) {
        bail!(
            "Crew runtime failed with exit code {:?}: {}",
            result.exit_code,
            truncate(stderr.trim(), 8_000)
        );
    }

    let mut response = None;
    let mut emitted_events = 0_usize;
    for line in result.stdout.lines().filter(|line| !line.trim().is_empty()) {
        let mut value: Value =
            serde_json::from_str(line).context("Crew runtime returned invalid JSON")?;
        redact_secret_values(&mut value, &secret_redactions);
        if value.get("localAiCoworkEvent").is_some() {
            if emitted_events < 1_000 && serde_json::to_vec(&value)?.len() <= 1024 * 1024 {
                db::append_leased_event(
                    pool,
                    lease.run.spec.id,
                    worker_id,
                    lease.lease_token,
                    None,
                    RunEventKind::ModelDelta,
                    json!({"adapter":"crewai","crew_event":value}),
                )
                .await?;
                emitted_events += 1;
            }
        } else {
            if response.replace(value).is_some() {
                bail!("Crew runtime returned more than one response");
            }
        }
    }
    let response = response.context("Crew runtime returned no response")?;
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("failed");
    let usage = crew_usage(&response);
    if let Some(usage) = usage {
        let cost_micros = runtime
            .model_pricing
            .map(|pricing| model_cost_micros(pricing, usage.prompt_tokens, usage.completion_tokens))
            .unwrap_or(0);
        governance::record_model_usage_for_run(
            pool,
            lease.run.spec.id,
            usage.total_tokens,
            cost_micros,
        )
        .await?;
    }
    if status != "completed" {
        if usage.is_some() {
            db::append_leased_event(
                pool,
                lease.run.spec.id,
                worker_id,
                lease.lease_token,
                None,
                RunEventKind::Warning,
                json!({
                    "code":"crew_usage_recorded_after_failure",
                    "adapter":"crewai",
                    "status":status,
                    "usage":response.get("usage").cloned().unwrap_or(Value::Null),
                }),
            )
            .await?;
        }
        bail!(
            "Crew runtime ended in state {status}: {}",
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
    db::append_leased_event(
        pool,
        lease.run.spec.id,
        worker_id,
        lease.lease_token,
        None,
        RunEventKind::ModelCompleted,
        json!({
            "adapter":"crewai",
            "content":content,
            "task_count":response.get("taskResults").and_then(Value::as_array).map_or(0, Vec::len),
            "event_count":emitted_events,
            "usage":response.get("usage").cloned().unwrap_or(Value::Null),
        }),
    )
    .await?;
    Ok(json!({"content":content,"crew_response":response}))
}

fn redact_secrets(value: &str, secrets: &[String]) -> String {
    let mut secrets = secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .collect::<Vec<_>>();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets
        .into_iter()
        .fold(value.to_owned(), |redacted, secret| {
            redacted.replace(secret.as_str(), "[REDACTED]")
        })
}

fn crew_usage(response: &Value) -> Option<CrewUsage> {
    let usage = response.get("usage")?.as_object()?;
    let prompt_tokens = usage.get("prompt_tokens").and_then(Value::as_u64);
    let completion_tokens = usage.get("completion_tokens").and_then(Value::as_u64);
    let total_tokens = usage.get("total_tokens").and_then(Value::as_u64);
    if prompt_tokens.is_none() && completion_tokens.is_none() && total_tokens.is_none() {
        return None;
    }
    let prompt_tokens = prompt_tokens.unwrap_or(0);
    let completion_tokens = completion_tokens.unwrap_or(0);
    let reported_total = total_tokens.unwrap_or(0);
    Some(CrewUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens: reported_total.max(prompt_tokens.saturating_add(completion_tokens)),
    })
}

fn redact_secret_values(value: &mut Value, secrets: &[String]) {
    match value {
        Value::String(text) => *text = redact_secrets(text, secrets),
        Value::Array(values) => {
            for value in values {
                redact_secret_values(value, secrets);
            }
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                redact_secret_values(value, secrets);
            }
        }
        _ => {}
    }
}

fn inject_crew_mcp_context(
    request: &mut Value,
    bindings: &[ResolvedServerMcpBinding],
    creator_user_id: Uuid,
) -> Result<Vec<String>> {
    let agents = request
        .get("agents")
        .and_then(Value::as_array)
        .context("the prepared Crew request must contain agents")?;
    let mut requested_names = BTreeSet::new();
    let mut agent_access = Vec::with_capacity(agents.len());
    for agent in agents {
        let agent = agent
            .as_object()
            .context("prepared Crew agents must be objects")?;
        let agent_id = agent
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("prepared Crew agents require an id")?;
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
            "allowedTools":[],
            "blockedTools":[],
            "allowedMcpServerNames":allowed_names,
            "blockedMcpServerNames":[],
            "delegationAllowed":false,
            "gatewayHints":[],
        }));
    }

    let by_name = bindings
        .iter()
        .map(|binding| (binding.name.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    let resolved_names = by_name
        .keys()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    if requested_names != resolved_names {
        bail!("resolved Crew MCP bindings do not match the prepared agent allowlists");
    }

    let mut executor_bindings = Vec::with_capacity(requested_names.len());
    let mut secrets = Vec::new();
    for name in &requested_names {
        let binding = by_name
            .get(name.as_str())
            .with_context(|| format!("Crew MCP binding {name:?} was not resolved"))?;
        secrets.extend(binding.secret_values());
        executor_bindings.push(binding.sandbox_value());
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
            "subject":format!("server-run:{creator_user_id}"),
            "subjectRoles":["runner"],
            "policyStrict":true,
            "denyRules":[],
            "pendingApprovalTypes":[],
            "agentAccess":agent_access,
        }),
    );
    Ok(secrets)
}

struct ServerRuntimeHost<'a> {
    pool: &'a PgPool,
    worker_id: Uuid,
    runtime: &'a WorkerRuntime,
    lease: &'a RunLease,
    mcp_bindings: Vec<ResolvedServerMcpBinding>,
}

#[async_trait]
impl RuntimeHost for ServerRuntimeHost<'_> {
    fn tools(&self) -> Vec<ToolDefinition> {
        let mut tools = vec![
            tool_definition(
                "Think",
                "Record private reasoning or a short plan without changing the workspace.",
                json!({"type":"object","properties":{"thought":{"type":"string"}},"required":["thought"],"additionalProperties":false}),
                None,
                false,
            ),
            tool_definition(
                "AskUser",
                "Pause the durable run and ask the user a structured question.",
                json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"],"additionalProperties":true}),
                None,
                false,
            ),
        ];
        if self.runtime.runner.is_some() {
            tools.extend([
                tool_definition("Read", "Read a UTF-8 text file from the run workspace.", json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1,"maximum":20000}},"required":["path"],"additionalProperties":false}), Some("files"), false),
                tool_definition("Write", "Create or replace a file in the run workspace.", json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}), Some("files"), true),
                tool_definition("Edit", "Replace an exact string in a UTF-8 workspace file.", json!({"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old_string","new_string"],"additionalProperties":false}), Some("files"), true),
                tool_definition("ListDir", "List directory entries and basic metadata.", json!({"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}), Some("files"), false),
                tool_definition("Glob", "Find workspace paths matching a glob pattern.", json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"],"additionalProperties":false}), Some("files"), false),
                tool_definition("Grep", "Search workspace text with ripgrep.", json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"},"case_sensitive":{"type":"boolean"}},"required":["pattern"],"additionalProperties":false}), Some("files"), false),
                tool_definition("Bash", "Run a shell command inside the isolated persistent run workspace.", json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1,"maximum":3600}},"required":["command"],"additionalProperties":false}), Some("shell"), true),
                tool_definition("WebFetch", "Fetch an HTTP(S) URL through the sandbox filtered-egress network.", json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"],"additionalProperties":false}), Some("web.fetch"), false),
                tool_definition("BrowserNavigate", "Navigate Chromium to an HTTP(S) URL. The profile, cookies and last URL persist for this run.", json!({"type":"object","properties":{"url":{"type":"string"},"visible":{"type":"boolean"},"wait_until":{"type":"string","enum":["load","domcontentloaded","networkidle","commit"]},"timeout_ms":{"type":"integer","minimum":1,"maximum":120000},"record_video":{"type":"boolean"}},"required":["url"],"additionalProperties":false}), Some("browser.headless"), false),
                tool_definition("BrowserClick", "Click the first element matching a Playwright selector; optionally capture a download.", json!({"type":"object","properties":{"selector":{"type":"string"},"expect_download":{"type":"boolean"},"download_path":{"type":"string"},"visible":{"type":"boolean"},"timeout_ms":{"type":"integer","minimum":1,"maximum":120000},"wait_ms":{"type":"integer","minimum":0,"maximum":30000},"record_video":{"type":"boolean"}},"required":["selector"],"additionalProperties":false}), Some("browser.headless"), true),
                tool_definition("BrowserFill", "Fill the first input matching a Playwright selector.", json!({"type":"object","properties":{"selector":{"type":"string"},"value":{"type":"string"},"visible":{"type":"boolean"},"timeout_ms":{"type":"integer","minimum":1,"maximum":120000},"record_video":{"type":"boolean"}},"required":["selector","value"],"additionalProperties":false}), Some("browser.headless"), true),
                tool_definition("BrowserUpload", "Attach one or more run-workspace files to a file input.", json!({"type":"object","properties":{"selector":{"type":"string"},"path":{"type":"string"},"paths":{"type":"array","items":{"type":"string"},"maxItems":50},"visible":{"type":"boolean"},"timeout_ms":{"type":"integer","minimum":1,"maximum":120000},"record_video":{"type":"boolean"}},"required":["selector"],"additionalProperties":false}), Some("browser.headless"), true),
                tool_definition("BrowserScreenshot", "Capture the current Chromium page as a PNG artifact.", json!({"type":"object","properties":{"path":{"type":"string"},"full_page":{"type":"boolean"},"visible":{"type":"boolean"},"record_video":{"type":"boolean"}},"additionalProperties":false}), Some("browser.headless"), false),
                tool_definition("BrowserInspect", "Return page title, visible body text and links from the current Chromium page.", json!({"type":"object","properties":{"max_chars":{"type":"integer","minimum":1,"maximum":200000},"visible":{"type":"boolean"},"timeout_ms":{"type":"integer","minimum":1,"maximum":120000},"record_video":{"type":"boolean"}},"additionalProperties":false}), Some("browser.headless"), false),
                tool_definition("BrowserTabs", "List the Chromium pages active during this browser operation.", json!({"type":"object","properties":{"visible":{"type":"boolean"},"record_video":{"type":"boolean"}},"additionalProperties":false}), Some("browser.headless"), false),
                tool_definition("OfficeInspect", "Inspect DOC/DOCX, XLS/XLSX, PPT/PPTX or PDF structure and text without executing active content.", json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}), Some("office.ooxml"), false),
                tool_definition("OfficeReplaceText", "Deterministically replace text in DOCX, XLSX or PPTX. Macro-enabled formats are rejected.", json!({"type":"object","properties":{"path":{"type":"string"},"output_path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old_text","new_text"],"additionalProperties":false}), Some("office.ooxml"), true),
                tool_definition("OfficeExportPdf", "Render an Office document to a PDF artifact with LibreOffice in safe mode.", json!({"type":"object","properties":{"path":{"type":"string"},"output_path":{"type":"string"}},"required":["path"],"additionalProperties":false}), Some("office.libreoffice"), false),
                tool_definition("OfficePreview", "Render the first page or all pages of an Office document/PDF to PNG review artifacts.", json!({"type":"object","properties":{"path":{"type":"string"},"all_pages":{"type":"boolean"},"dpi":{"type":"integer","minimum":72,"maximum":200}},"required":["path"],"additionalProperties":false}), Some("office.libreoffice"), false),
                tool_definition("DesktopOpenOffice", "Open an Office document in the persistent visible LibreOffice desktop for observation or manual takeover.", json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}), Some("desktop.linux"), false),
            ]);
            if !self.mcp_bindings.is_empty() {
                let names = self
                    .mcp_bindings
                    .iter()
                    .map(|binding| binding.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                tools.push(tool_definition(
                    "MCPTool",
                    &format!(
                        "Call a tool on an encrypted project-bound Linux MCP server. Available servers: {names}"
                    ),
                    json!({"type":"object","properties":{"server_name":{"type":"string"},"tool_name":{"type":"string"},"arguments":{"type":"object"}},"required":["server_name","tool_name"],"additionalProperties":false}),
                    Some("tool.mcp.invoke"),
                    true,
                ));
            }
        }
        tools
    }

    async fn emit(&self, kind: RunEventKind, payload: Value) -> Result<()> {
        if kind == RunEventKind::ModelStarted {
            governance::ensure_model_quota_for_run(self.pool, self.lease.run.spec.id).await?;
        }
        let usage = if kind == RunEventKind::ModelCompleted {
            payload.get("usage").map(|usage| {
                (
                    usage
                        .get("prompt_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    usage
                        .get("completion_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                )
            })
        } else {
            None
        };
        db::append_leased_event(
            self.pool,
            self.lease.run.spec.id,
            self.worker_id,
            self.lease.lease_token,
            None,
            kind,
            payload,
        )
        .await?;
        if let Some((prompt_tokens, completion_tokens)) = usage {
            let cost_micros = self
                .runtime
                .model_pricing
                .map(|pricing| model_cost_micros(pricing, prompt_tokens, completion_tokens))
                .unwrap_or(0);
            governance::record_model_usage_for_run(
                self.pool,
                self.lease.run.spec.id,
                prompt_tokens.saturating_add(completion_tokens),
                cost_micros,
            )
            .await?;
        }
        Ok(())
    }

    async fn execute_tool(&self, invocation: ToolInvocation) -> Result<ToolOutput> {
        if invocation.name == "Think" {
            return Ok(ToolOutput {
                content: invocation
                    .arguments
                    .get("thought")
                    .and_then(Value::as_str)
                    .unwrap_or("Thought recorded.")
                    .to_owned(),
                is_error: false,
                safe_to_resume: true,
                metadata: Value::Null,
            });
        }
        if invocation.name == "AskUser" {
            let response = workflow::await_worker_input(
                self.pool,
                self.worker_id,
                self.lease.run.spec.id,
                self.lease.lease_token,
                invocation.arguments.clone(),
            )
            .await?;
            let is_error = response.is_none();
            return Ok(ToolOutput {
                content: response
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "The input request expired.".to_owned()),
                is_error,
                safe_to_resume: true,
                metadata: Value::Null,
            });
        }

        let mutating = matches!(
            invocation.name.as_str(),
            "Write"
                | "Edit"
                | "Bash"
                | "OfficeReplaceText"
                | "BrowserClick"
                | "BrowserFill"
                | "BrowserUpload"
                | "MCPTool"
        );
        let policy = self.tool_policy().await?;
        if mutating && policy == "read_only" {
            return Ok(ToolOutput {
                content: "Denied by the project's read-only tool policy.".to_owned(),
                is_error: true,
                safe_to_resume: true,
                metadata: json!({"policy": policy}),
            });
        }
        if mutating && policy == "confirm_mutations" {
            let approved = workflow::await_worker_approval(
                self.pool,
                self.worker_id,
                self.lease.run.spec.id,
                self.lease.lease_token,
                json!({
                    "tool": invocation.name,
                    "arguments": invocation.arguments,
                    "tool_call_id": invocation.id,
                }),
            )
            .await?;
            if !approved {
                return Ok(ToolOutput {
                    content: "The user rejected or did not answer the approval request.".to_owned(),
                    is_error: true,
                    safe_to_resume: true,
                    metadata: json!({"approval": "rejected"}),
                });
            }
        }
        if mutating {
            workflow::create_worker_checkpoint(
                self.pool,
                self.worker_id,
                self.lease.run.spec.id,
                self.lease.lease_token,
                false,
                json!({
                    "phase": "tool_dispatched",
                    "tool_call_id": invocation.id,
                    "tool": invocation.name,
                    "arguments": invocation.arguments,
                }),
            )
            .await?;
        }
        self.execute_sandbox_tool(&invocation).await
    }

    async fn checkpoint(&self, state: Value, safe_to_resume: bool) -> Result<()> {
        workflow::create_worker_checkpoint(
            self.pool,
            self.worker_id,
            self.lease.run.spec.id,
            self.lease.lease_token,
            safe_to_resume,
            state,
        )
        .await?;
        Ok(())
    }
}

fn model_cost_micros(pricing: ModelPricing, prompt_tokens: u64, completion_tokens: u64) -> u64 {
    let input = u128::from(prompt_tokens) * u128::from(pricing.input_micros_per_million);
    let output = u128::from(completion_tokens) * u128::from(pricing.output_micros_per_million);
    u64::try_from((input + output).div_ceil(1_000_000)).unwrap_or(u64::MAX)
}

impl ServerRuntimeHost<'_> {
    async fn tool_policy(&self) -> Result<String> {
        if let Some(policy) = self
            .lease
            .run
            .spec
            .input
            .get("tool_policy")
            .and_then(Value::as_str)
        {
            return Ok(policy.to_owned());
        }
        let policy = sqlx::query_scalar::<_, Value>(
            "SELECT policy FROM projects WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(self.lease.run.spec.project_id)
        .fetch_optional(self.pool)
        .await?
        .unwrap_or(Value::Null);
        Ok(policy
            .get("tool_policy")
            .and_then(Value::as_str)
            .unwrap_or("autonomous")
            .to_owned())
    }

    async fn execute_sandbox_tool(&self, invocation: &ToolInvocation) -> Result<ToolOutput> {
        let argument = |name: &str| -> Result<&str> {
            invocation
                .arguments
                .get(name)
                .and_then(Value::as_str)
                .with_context(|| format!("{} requires string argument {name}", invocation.name))
        };
        let mut stdin_base64 = None;
        let mut network = SandboxNetwork::None;
        let mut limits = SandboxLimits::default();
        let mut image = SandboxImage::Core;
        let mut secret_redactions = Vec::new();
        let argv = match invocation.name.as_str() {
            "Read" => {
                let path = validated_workspace_path(argument("path")?)?;
                let offset = invocation
                    .arguments
                    .get("offset")
                    .and_then(Value::as_u64)
                    .unwrap_or(1);
                let limit = invocation
                    .arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(2000)
                    .min(20_000);
                vec!["python3".to_owned(), "-c".to_owned(), "import pathlib,sys; p=pathlib.Path(sys.argv[1]); lines=p.read_text(encoding='utf-8').splitlines(True); start=max(int(sys.argv[2])-1,0); sys.stdout.write(''.join(lines[start:start+int(sys.argv[3])]))".to_owned(), path, offset.to_string(), limit.to_string()]
            }
            "Write" => {
                let path = validated_workspace_path(argument("path")?)?;
                let content = argument("content")?.as_bytes();
                stdin_base64 = Some(STANDARD.encode(content));
                vec!["python3".to_owned(), "-c".to_owned(), "import pathlib,sys; p=pathlib.Path(sys.argv[1]); p.parent.mkdir(parents=True,exist_ok=True); p.write_bytes(sys.stdin.buffer.read()); print(p)".to_owned(), path]
            }
            "Edit" => {
                let path = validated_workspace_path(argument("path")?)?;
                let payload = json!({
                    "old": argument("old_string")?,
                    "new": argument("new_string")?,
                    "all": invocation.arguments.get("replace_all").and_then(Value::as_bool).unwrap_or(false)
                });
                stdin_base64 = Some(STANDARD.encode(serde_json::to_vec(&payload)?));
                vec!["python3".to_owned(), "-c".to_owned(), "import json,pathlib,sys; p=pathlib.Path(sys.argv[1]); s=p.read_text(encoding='utf-8'); q=json.load(sys.stdin); n=s.count(q['old']); assert n>0,'old_string not found'; assert q['all'] or n==1,f'old_string occurs {n} times'; p.write_text(s.replace(q['old'],q['new']) if q['all'] else s.replace(q['old'],q['new'],1),encoding='utf-8'); print(f'replacements={n if q[\"all\"] else 1}')".to_owned(), path]
            }
            "ListDir" => {
                let path = validated_workspace_path(
                    invocation
                        .arguments
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("."),
                )?;
                vec!["python3".to_owned(), "-c".to_owned(), "import json,pathlib,sys; p=pathlib.Path(sys.argv[1]); print(json.dumps([{'name':x.name,'type':'dir' if x.is_dir() else 'file','size':x.stat().st_size if x.is_file() else None} for x in sorted(p.iterdir())]))".to_owned(), path]
            }
            "Glob" => {
                let path = validated_workspace_path(
                    invocation
                        .arguments
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("."),
                )?;
                let pattern = validated_glob_pattern(argument("pattern")?)?;
                vec!["python3".to_owned(), "-c".to_owned(), "import pathlib,sys; p=pathlib.Path(sys.argv[1]); print('\\n'.join(str(x) for x in sorted(p.glob(sys.argv[2])))[:4000000])".to_owned(), path, pattern]
            }
            "Grep" => {
                let path = validated_workspace_path(
                    invocation
                        .arguments
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("."),
                )?;
                let mut argv = vec![
                    "rg".to_owned(),
                    "--line-number".to_owned(),
                    "--no-heading".to_owned(),
                    "--color=never".to_owned(),
                ];
                if !invocation
                    .arguments
                    .get("case_sensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
                {
                    argv.push("--ignore-case".to_owned());
                }
                argv.extend(["--".to_owned(), argument("pattern")?.to_owned(), path]);
                argv
            }
            "Bash" => {
                limits.timeout_seconds = invocation
                    .arguments
                    .get("timeout_seconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(900)
                    .clamp(1, 3600);
                vec![
                    "/bin/bash".to_owned(),
                    "-lc".to_owned(),
                    argument("command")?.to_owned(),
                ]
            }
            "WebFetch" => {
                network = SandboxNetwork::FilteredEgress;
                limits.timeout_seconds = 120;
                let url = argument("url")?;
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    bail!("WebFetch only accepts HTTP(S) URLs");
                }
                vec![
                    "curl".to_owned(),
                    "--fail-with-body".to_owned(),
                    "--location".to_owned(),
                    "--max-redirs".to_owned(),
                    "5".to_owned(),
                    "--max-time".to_owned(),
                    "120".to_owned(),
                    "--silent".to_owned(),
                    "--show-error".to_owned(),
                    url.to_owned(),
                ]
            }
            "MCPTool" => {
                let server_name = argument("server_name")?;
                let tool_name = argument("tool_name")?;
                if tool_name.trim().is_empty()
                    || tool_name.len() > 1024
                    || tool_name.chars().any(char::is_control)
                {
                    bail!("MCP tool name is missing or invalid");
                }
                let binding = self
                    .mcp_bindings
                    .iter()
                    .find(|binding| binding.name == server_name)
                    .with_context(|| {
                        format!("MCP server {server_name:?} is not bound to this Linux executor")
                    })?;
                let arguments = invocation
                    .arguments
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if !arguments.is_object() {
                    bail!("MCPTool arguments must be an object");
                }
                let payload = json!({
                    "server": binding.sandbox_value(),
                    "tool_name": tool_name,
                    "arguments": arguments,
                    "timeout_seconds": 120,
                });
                secret_redactions.extend(binding.secret_values());
                stdin_base64 = Some(STANDARD.encode(serde_json::to_vec(&payload)?));
                network = SandboxNetwork::FilteredEgress;
                limits.timeout_seconds = 150;
                limits.output_bytes = 8 * 1024 * 1024;
                vec!["python3".to_owned(), "/opt/cowork/mcp-tool.py".to_owned()]
            }
            name if name.starts_with("Browser") => {
                let action = match name {
                    "BrowserNavigate" => "navigate",
                    "BrowserClick" => "click",
                    "BrowserFill" => "fill",
                    "BrowserUpload" => "upload",
                    "BrowserScreenshot" => "screenshot",
                    "BrowserInspect" => "inspect",
                    "BrowserTabs" => "tabs",
                    _ => return Err(anyhow!("unsupported browser tool {name}")),
                };
                let mut payload = invocation.arguments.clone();
                let object = payload
                    .as_object_mut()
                    .context("browser tool arguments must be an object")?;
                object.insert("action".to_owned(), Value::String(action.to_owned()));
                if !object.contains_key("visible")
                    && self
                        .lease
                        .run
                        .spec
                        .required_capabilities
                        .iter()
                        .any(|capability| capability.0 == "browser.visible")
                {
                    object.insert("visible".to_owned(), Value::Bool(true));
                }
                let visible = object
                    .get("visible")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if visible {
                    let runner = self
                        .runtime
                        .desktop_runner
                        .as_ref()
                        .context("the desktop runner is not configured")?;
                    desktop::ensure_worker_session(
                        self.pool,
                        runner,
                        self.lease.run.spec.id,
                        self.worker_id,
                    )
                    .await?;
                }
                limits.timeout_seconds = object
                    .get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(30_000)
                    .div_ceil(1_000)
                    .saturating_add(30)
                    .clamp(30, 300);
                stdin_base64 = Some(STANDARD.encode(serde_json::to_vec(&payload)?));
                network = SandboxNetwork::FilteredEgress;
                image = SandboxImage::Gui;
                vec!["node".to_owned(), "/opt/cowork/browser-tool.mjs".to_owned()]
            }
            name if name.starts_with("Office") => {
                let action = match name {
                    "OfficeInspect" => "inspect",
                    "OfficeReplaceText" => "replace_text",
                    "OfficeExportPdf" => "export_pdf",
                    "OfficePreview" => "preview",
                    _ => return Err(anyhow!("unsupported Office tool {name}")),
                };
                let mut payload = invocation.arguments.clone();
                payload
                    .as_object_mut()
                    .context("Office tool arguments must be an object")?
                    .insert("action".to_owned(), Value::String(action.to_owned()));
                stdin_base64 = Some(STANDARD.encode(serde_json::to_vec(&payload)?));
                limits.timeout_seconds = 300;
                image = SandboxImage::Gui;
                vec![
                    "/opt/cowork/office-venv/bin/python".to_owned(),
                    "/opt/cowork/office-tool.py".to_owned(),
                ]
            }
            "DesktopOpenOffice" => {
                let path = validated_workspace_path(argument("path")?)?;
                let runner = self
                    .runtime
                    .desktop_runner
                    .as_ref()
                    .context("the desktop runner is not configured")?;
                desktop::ensure_worker_session(
                    self.pool,
                    runner,
                    self.lease.run.spec.id,
                    self.worker_id,
                )
                .await?;
                limits.timeout_seconds = 30;
                image = SandboxImage::Gui;
                vec![
                    "/opt/cowork/office-venv/bin/python".to_owned(),
                    "-c".to_owned(),
                    "import subprocess,sys; subprocess.Popen(['libreoffice','--nologo','--nodefault','--nofirststartwizard',sys.argv[1]],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True); print(sys.argv[1])".to_owned(),
                    path,
                ]
            }
            other => return Err(anyhow!("unsupported server tool {other}")),
        };
        let spec = SandboxRunSpec {
            schema_version: cowork_contracts::SCHEMA_VERSION,
            run_id: self.lease.run.spec.id,
            image,
            argv,
            environment: Default::default(),
            stdin_base64,
            network,
            limits,
        };
        let result = send_runner_job(self.runtime, &spec).await?;
        let stdout = redact_secrets(&result.stdout, &secret_redactions);
        let stderr = redact_secrets(&result.stderr, &secret_redactions);
        let structured = if invocation.name.starts_with("Browser")
            || invocation.name.starts_with("Office")
            || invocation.name == "MCPTool"
        {
            serde_json::from_str::<Value>(&stdout).unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let failed = result.timed_out
            || result.exit_code != Some(0)
            || (invocation.name == "MCPTool"
                && structured.get("success").and_then(Value::as_bool) != Some(true));
        let content = if failed {
            format!("stdout:\n{stdout}\nstderr:\n{stderr}")
        } else if invocation.name == "MCPTool" {
            serde_json::to_string_pretty(structured.get("result").unwrap_or(&structured))?
        } else if stderr.is_empty() {
            stdout.clone()
        } else {
            format!("{stdout}\n[stderr]\n{stderr}")
        };
        // Browser and Office adapters emit structured diagnostics even when an
        // operation fails. Persisting those artifacts is essential for review
        // and must not depend on a zero exit code.
        if let Some(artifacts) = structured.get("artifacts").and_then(Value::as_array) {
            for path in artifacts.iter().filter_map(Value::as_str) {
                let payload = if let Some(store) = &self.runtime.object_store {
                    let bytes =
                        fetch_runner_file(self.runtime, self.lease.run.spec.id, path).await?;
                    storage::persist_run_artifact(
                        self.pool,
                        store,
                        self.lease.run.spec.id,
                        self.lease.run.spec.project_id,
                        self.lease.run.spec.creator_user_id,
                        path,
                        &invocation.name,
                        None,
                        &bytes,
                    )
                    .await?
                } else {
                    json!({
                        "path": path,
                        "source": invocation.name,
                        "storage": "run_workspace",
                    })
                };
                self.emit(RunEventKind::ArtifactCreated, payload).await?;
            }
        }
        Ok(ToolOutput {
            content,
            is_error: failed,
            safe_to_resume: !failed,
            metadata: json!({"sandbox": result, "output": structured}),
        })
    }
}

fn tool_definition(
    name: &str,
    description: &str,
    input_schema: Value,
    capability: Option<&str>,
    mutating: bool,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema,
        required_capability: capability.map(Capability::from),
        mutating,
    }
}

fn validated_workspace_path(path: &str) -> Result<String> {
    if path.is_empty()
        || path.len() > 4096
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "..")
    {
        bail!("workspace path must be a normalized relative POSIX path");
    }
    Ok(path.to_owned())
}

fn validated_glob_pattern(pattern: &str) -> Result<String> {
    if pattern.is_empty()
        || pattern.len() > 4096
        || pattern.starts_with('/')
        || pattern.contains('\\')
        || pattern.split('/').any(|component| component == "..")
    {
        bail!("glob pattern must stay inside the run workspace");
    }
    Ok(pattern.to_owned())
}

async fn send_runner_job(
    runtime: &WorkerRuntime,
    spec: &SandboxRunSpec,
) -> Result<SandboxRunResult> {
    let runner = runtime
        .runner
        .as_ref()
        .context("the Docker sandbox runner is not configured")?;
    let body = serde_json::to_vec(spec)?;
    let path = "/v1/jobs";
    let (timestamp, nonce, signature) = runner_signature(&runner.signing_key, "POST", path, &body)?;
    let response = runtime
        .http
        .post(format!("{}{path}", runner.url.trim_end_matches('/')))
        .header("x-cowork-timestamp", timestamp)
        .header("x-cowork-nonce", nonce)
        .header("x-cowork-signature", signature)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .context("runner request failed")?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!(
            "sandbox runner returned {status}: {}",
            truncate(&body, 2_000)
        );
    }
    serde_json::from_str(&body).context("invalid sandbox runner response")
}

async fn fetch_runner_file(
    runtime: &WorkerRuntime,
    run_id: Uuid,
    workspace_path: &str,
) -> Result<Vec<u8>> {
    let runner = runtime
        .runner
        .as_ref()
        .context("the Docker sandbox runner is not configured")?;
    let encoded_path = utf8_percent_encode(workspace_path, NON_ALPHANUMERIC).to_string();
    let path = format!("/v1/runs/{run_id}/file?path={encoded_path}");
    let (timestamp, nonce, signature) = runner_signature(&runner.signing_key, "GET", &path, &[])?;
    let response = runtime
        .http
        .get(format!("{}{}", runner.url.trim_end_matches('/'), path))
        .header("x-cowork-timestamp", timestamp)
        .header("x-cowork-nonce", nonce)
        .header("x-cowork-signature", signature)
        .send()
        .await
        .context("runner artifact request failed")?;
    let status = response.status();
    if let Some(size) = response.content_length() {
        if size > 64 * 1024 * 1024 {
            bail!("runner artifact exceeds the 64 MiB transfer limit");
        }
    }
    let bytes = response.bytes().await?;
    if !status.is_success() {
        bail!(
            "sandbox runner returned {status} for artifact: {}",
            truncate(&String::from_utf8_lossy(&bytes), 2_000)
        );
    }
    if bytes.len() > 64 * 1024 * 1024 {
        bail!("runner artifact exceeds the 64 MiB transfer limit");
    }
    Ok(bytes.to_vec())
}

async fn execute_sandbox_run(
    pool: &PgPool,
    worker_id: Uuid,
    lease_duration: Duration,
    runtime: &WorkerRuntime,
    lease: &RunLease,
) -> Result<Value> {
    let runner = runtime
        .runner
        .as_ref()
        .context("the Docker sandbox runner is not configured")?;
    let mut spec: SandboxRunSpec = serde_json::from_value(
        lease
            .run
            .spec
            .input
            .get("sandbox")
            .cloned()
            .context("sandbox input is missing")?,
    )
    .context("invalid sandbox input")?;
    spec.run_id = lease.run.spec.id;
    spec.schema_version = cowork_contracts::SCHEMA_VERSION;
    let body = serde_json::to_vec(&spec)?;
    let path = "/v1/jobs";
    let (timestamp, nonce, signature) = runner_signature(&runner.signing_key, "POST", path, &body)?;

    workflow::create_worker_checkpoint(
        pool,
        worker_id,
        lease.run.spec.id,
        lease.lease_token,
        false,
        json!({
            "phase": "sandbox_dispatched",
            "image": spec.image,
            "argv": spec.argv,
            "network": spec.network,
        }),
    )
    .await?;
    db::append_leased_event(
        pool,
        lease.run.spec.id,
        worker_id,
        lease.lease_token,
        None,
        RunEventKind::ToolStarted,
        json!({"tool": "sandbox", "image": spec.image, "network": spec.network}),
    )
    .await?;
    let response = await_with_lease(
        pool,
        worker_id,
        lease,
        lease_duration,
        runtime
            .http
            .post(format!("{}{path}", runner.url.trim_end_matches('/')))
            .header("x-cowork-timestamp", &timestamp)
            .header("x-cowork-nonce", nonce)
            .header("x-cowork-signature", signature)
            .header("content-type", "application/json")
            .body(body)
            .send(),
    )
    .await
    .context("runner request failed")?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(anyhow!(
            "sandbox runner returned {status}: {}",
            truncate(&body, 2_000)
        ));
    }
    let result: SandboxRunResult =
        serde_json::from_str(&body).context("invalid sandbox runner response")?;
    let payload = serde_json::to_value(&result)?;
    if result.timed_out || result.exit_code != Some(0) {
        db::append_leased_event(
            pool,
            lease.run.spec.id,
            worker_id,
            lease.lease_token,
            None,
            RunEventKind::ToolFailed,
            payload.clone(),
        )
        .await?;
        return Err(anyhow!(
            "sandbox command failed with exit code {:?}{}",
            result.exit_code,
            if result.timed_out {
                " after timeout"
            } else {
                ""
            }
        ));
    }
    db::append_leased_event(
        pool,
        lease.run.spec.id,
        worker_id,
        lease.lease_token,
        None,
        RunEventKind::ToolCompleted,
        payload.clone(),
    )
    .await?;
    workflow::create_worker_checkpoint(
        pool,
        worker_id,
        lease.run.spec.id,
        lease.lease_token,
        true,
        json!({"phase": "sandbox_completed", "result": payload}),
    )
    .await?;
    Ok(json!({"sandbox": payload}))
}

fn runner_signature(
    signing_key: &[u8],
    method: &str,
    path_and_query: &str,
    body: &[u8],
) -> Result<(String, String, String)> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs()
        .to_string();
    let nonce = Uuid::new_v4().to_string();
    let mut mac = HmacSha256::new_from_slice(signing_key)
        .map_err(|_| anyhow!("invalid runner signing key"))?;
    mac.update(timestamp.as_bytes());
    mac.update(b"\n");
    mac.update(nonce.as_bytes());
    mac.update(b"\n");
    mac.update(method.as_bytes());
    mac.update(b"\n");
    mac.update(path_and_query.as_bytes());
    mac.update(b"\n");
    mac.update(body);
    Ok((timestamp, nonce, hex::encode(mac.finalize().into_bytes())))
}

async fn await_with_lease<F>(
    pool: &PgPool,
    worker_id: Uuid,
    lease: &RunLease,
    lease_duration: Duration,
    future: F,
) -> Result<Response>
where
    F: Future<Output = Result<Response, reqwest::Error>>,
{
    tokio::pin!(future);
    let heartbeat_every = (lease_duration / 3).max(Duration::from_secs(5));
    let mut heartbeat = tokio::time::interval(heartbeat_every);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    heartbeat.tick().await;
    loop {
        tokio::select! {
            response = &mut future => return response.map_err(Into::into),
            _ = heartbeat.tick() => {
                db::renew_lease(
                    pool,
                    lease.run.spec.id,
                    worker_id,
                    lease.lease_token,
                    lease_duration.as_secs() as i64,
                ).await.context("failed to renew worker lease")?;
            }
        }
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_cost_rounds_up_to_one_micro() {
        let pricing = ModelPricing {
            input_micros_per_million: 2_000_000,
            output_micros_per_million: 8_000_000,
        };
        assert_eq!(model_cost_micros(pricing, 500_000, 250_000), 3_000_000);
        assert_eq!(model_cost_micros(pricing, 1, 0), 2);
    }

    #[test]
    fn crew_protocol_values_are_recursively_redacted() {
        let mut value = json!({
            "message":"provider rejected secret-value",
            "nested":[{"detail":"secret-value"}],
        });
        redact_secret_values(&mut value, &["secret-value".to_owned()]);
        assert_eq!(value["message"], "provider rejected [REDACTED]");
        assert_eq!(value["nested"][0]["detail"], "[REDACTED]");
        assert!(!value.to_string().contains("secret-value"));
    }

    #[test]
    fn crew_mcp_context_is_agent_scoped_and_returns_secret_redactions() {
        let creator_user_id = Uuid::new_v4();
        let mut request = json!({
            "agents":[
                {"id":"researcher","mcpServerNames":["Docs"]},
                {"id":"reviewer","mcpServerNames":[]}
            ]
        });
        let bindings = vec![ResolvedServerMcpBinding {
            name: "Docs".to_owned(),
            transport: "stdio".to_owned(),
            command: "/opt/cowork/bin/docs-mcp".to_owned(),
            args: vec!["--stdio".to_owned()],
            environment: BTreeMap::from([("DOCS_TOKEN".to_owned(), "crew-secret".to_owned())]),
            url: String::new(),
            headers: BTreeMap::new(),
        }];

        let secrets = inject_crew_mcp_context(&mut request, &bindings, creator_user_id).unwrap();

        assert_eq!(secrets, vec!["crew-secret"]);
        assert_eq!(
            request["executorMcpBindings"][0]["command"],
            "/opt/cowork/bin/docs-mcp"
        );
        assert_eq!(request["executorMcpBindings"][0]["transport"], "stdio");
        assert_eq!(
            request["governance"]["subject"],
            format!("server-run:{creator_user_id}")
        );
        assert_eq!(
            request["governance"]["agentAccess"][0]["allowedMcpServerNames"],
            json!(["Docs"])
        );
        assert_eq!(
            request["governance"]["agentAccess"][1]["allowedMcpServerNames"],
            json!([])
        );
        assert_eq!(
            request["governance"]["agentAccess"][0]["allowedTools"],
            json!([])
        );
    }

    #[test]
    fn mcp_environment_values_are_redacted_longest_first() {
        assert_eq!(
            redact_secrets(
                "token=secret-value fallback=secret",
                &["secret".to_owned(), "secret-value".to_owned()],
            ),
            "token=[REDACTED] fallback=[REDACTED]"
        );
    }

    #[test]
    fn crew_usage_uses_the_reported_total_and_billable_token_split() {
        assert_eq!(
            crew_usage(&json!({"usage":{
                "prompt_tokens":20,
                "completion_tokens":10,
                "total_tokens":37,
                "reasoning_tokens":7,
            }})),
            Some(CrewUsage {
                prompt_tokens: 20,
                completion_tokens: 10,
                total_tokens: 37,
            })
        );
    }

    #[test]
    fn crew_usage_never_understates_the_reported_billable_split() {
        assert_eq!(
            crew_usage(&json!({"usage":{
                "prompt_tokens":20,
                "completion_tokens":10,
                "total_tokens":1,
            }})),
            Some(CrewUsage {
                prompt_tokens: 20,
                completion_tokens: 10,
                total_tokens: 30,
            })
        );
        assert_eq!(crew_usage(&json!({"usage":{"total_tokens":"30"}})), None);
        assert_eq!(crew_usage(&json!({})), None);
    }
}
