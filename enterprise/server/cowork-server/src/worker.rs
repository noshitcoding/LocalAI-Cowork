use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
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
    crew::{
        apply_crew_agent_tool_policy, classify_crew_protocol_line, crew_protocol_run_event_kind,
        prepare_crew_request_with_agent_models, CrewModelConfig, CrewProtocolLine,
    },
    AgentRuntime, ModelConfig as AgentModelConfig, RuntimeHost, ToolDefinition, ToolInvocation,
    ToolOutput,
};
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    config::Config,
    db, desktop, governance,
    mcp_bindings::{self, ResolvedServerMcpBinding},
    providers, storage, workflow,
};

mod backend;
mod lifecycle;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug)]
struct ManagedDispatchDenied(crate::error::ApiError);
impl std::fmt::Display for ManagedDispatchDenied {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f,"{}",self.0)
    }
}
impl std::error::Error for ManagedDispatchDenied {}

#[derive(Clone)]
struct WorkerRuntime {
    http: Client,
    agent: Option<Arc<AgentRuntime>>,
    default_crew_model: Option<Arc<CrewModelConfig>>,
    runner: Option<RunnerConfig>,
    desktop_runner: Option<desktop::RunnerControl>,
    object_store: Option<storage::ObjectStore>,
    model_pricing: Option<ModelPricing>,
    server_capabilities: Vec<Capability>,
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

pub async fn run_until_shutdown<S>(pool: PgPool, config: Config, shutdown: S) -> Result<()>
where
    S: Future<Output = ()>,
{
    let object_store = config
        .object_store
        .as_ref()
        .map(storage::ObjectStore::from_config)
        .transpose()
        .context("invalid object-store configuration")?;
    let desktop_runner = config
        .runner_url
        .clone()
        .zip(config.runner_signing_key.clone())
        .map(|(url, key)| desktop::RunnerControl::new(url, key))
        .transpose()?;
    let http = match &desktop_runner {
        Some(runner) => runner.http_client(),
        None => Client::builder()
            .timeout(Duration::from_secs(24 * 60 * 60))
            .build()
            .context("invalid worker HTTP client configuration")?,
    };
    let runtime = WorkerRuntime {
        http,
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
        desktop_runner,
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
        server_capabilities: config.server_capabilities.clone(),
    };

    if runtime.agent.is_none() && runtime.runner.is_none() {
        tracing::warn!(
            "worker has neither a model endpoint nor a sandbox runner; server runs remain queued"
        );
    }
    tracing::info!(worker_id = %config.worker_id, "durable worker started");

    let settings = lifecycle::Settings {
        poll_interval: config.worker_poll_interval,
        max_concurrency: config.worker_max_concurrency,
        shutdown_grace: config.worker_shutdown_grace,
    };
    lifecycle::supervise(
        backend::ServerBackend {
            pool,
            config,
            runtime,
        },
        settings,
        shutdown,
    )
    .await
}

async fn execute_run(
    pool: &PgPool,
    worker_id: Uuid,
    lease_duration: Duration,
    runtime: &WorkerRuntime,
    lease: RunLease,
    claim_started: tokio::time::Instant,
) -> Result<()> {
    let initial_deadline = confirmed_lease_deadline(lease_duration, claim_started)?;
    let operation = execute_and_finalize_run(pool, worker_id, runtime, &lease);
    let renew = || async {
        let request_started = tokio::time::Instant::now();
        let renewed = db::renew_lease(
            pool,
            lease.run.spec.id,
            worker_id,
            lease.lease_token,
            lease_duration.as_secs() as i64,
        )
        .await
        .context("failed to renew worker lease")?;
        renewed
            .lease_expires_at
            .context("renewal did not return a lease expiry")?;
        confirmed_lease_deadline(lease_duration, request_started)
    };
    with_lease_heartbeat(lease_duration, initial_deadline, operation, renew).await
}

async fn execute_and_finalize_run(
    pool: &PgPool,
    worker_id: Uuid,
    runtime: &WorkerRuntime,
    lease: &RunLease,
) -> Result<()> {
    let outcome = if lease.run.spec.input.get("sandbox").is_some() {
        execute_sandbox_run(pool, worker_id, runtime, lease).await
    } else if lease
        .run
        .spec
        .input
        .get("task_runner")
        .and_then(Value::as_str)
        == Some("crew")
    {
        execute_crew_run(pool, worker_id, runtime, lease).await
    } else {
        execute_agent_run(pool, worker_id, runtime, lease).await
    };
    let mut desktop_stopped = true;
    if let Some(runner) = &runtime.desktop_runner {
        if let Err(error) = desktop::end_worker_leased_sessions(
            pool,
            runner,
            lease.run.spec.id,
            worker_id,
            lease.lease_token,
        )
        .await
        {
            desktop_stopped = false;
            tracing::warn!(?error, run_id = %lease.run.spec.id, "failed to clean up desktop sessions");
        }
    }

    match outcome {
        Ok(result) => {
            db::complete_server_leased_run(
                pool,
                lease.run.spec.id,
                worker_id,
                lease.lease_token,
                result,
                desktop_stopped,
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
            if safe_to_resume && desktop_stopped && error.is::<ManagedDispatchDenied>() {
                db::fail_server_dispatch_denied(
                    pool,lease.run.spec.id,worker_id,lease.lease_token,run_error,
                ).await?;
            } else if safe_to_resume {
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
    crate::organisation_mfa::execution_on(pool, lease.run.spec.id).await?;
    crate::managed_resources::execution::authorize(&lease.run.spec.input,lease.run.spec.creator_user_id)
        .await.map_err(ManagedDispatchDenied)?;
    let checkpoint =
        db::begin_server_agent_execution(pool, lease.run.spec.id, worker_id, lease.lease_token)
            .await?;
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
    let mut mcp_bindings = mcp_bindings::resolve_server_bindings_for_run(
        pool,
        runtime.object_store.as_ref(),
        lease.run.spec.project_id,
        &lease.run.spec.input,
    )
    .await?;
    mcp_bindings.extend(crate::managed_resources::execution::mcp(
        &lease.run.spec.input,lease.run.spec.creator_user_id,
    ).await?);
    let host = ServerRuntimeHost {
        pool,
        worker_id,
        runtime,
        lease,
        mcp_bindings,
    };
    let result = if let Some(checkpoint) = checkpoint {
        agent.resume(&lease.run.spec, &host, checkpoint).await?
    } else {
        agent.execute(&lease.run.spec, &host).await?
    };
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
    let mut profile_models = HashMap::new();
    let mut profile_secrets = Vec::new();
    for profile_id in providers::crew_profile_ids_for_server(&lease.run.spec.input)? {
        let profile = providers::resolve_server_provider(
            pool,
            runtime.object_store.as_ref(),
            lease.run.spec.creator_user_id,
            lease.run.spec.project_id,
            profile_id,
        )
        .await?;
        if let Some(secret) = profile.api_key.clone().filter(|value| !value.is_empty()) {
            profile_secrets.push(secret);
        }
        profile_models.insert(
            profile_id.to_string(),
            CrewModelConfig {
                base_url: profile.base_url,
                api_key: profile.api_key,
                model: profile.model,
                timeout: profile.timeout,
                verify_tls_certificates: profile.verify_tls_certificates,
            },
        );
    }
    let mcp_bindings = mcp_bindings::resolve_server_bindings_for_run(
        pool,
        runtime.object_store.as_ref(),
        lease.run.spec.project_id,
        &lease.run.spec.input,
    )
    .await?;
    let mut request = prepare_crew_request_with_agent_models(
        definition,
        &lease.run.spec,
        &model,
        &profile_models,
    )?;
    let mut secret_redactions = model.api_key.clone().into_iter().collect::<Vec<_>>();
    secret_redactions.extend(profile_secrets);
    secret_redactions.extend(inject_crew_mcp_context(
        &mut request,
        &mcp_bindings,
        lease.run.spec.creator_user_id,
        &runtime.server_capabilities,
    )?);
    let longest_model_timeout = profile_models
        .values()
        .map(|profile| profile.timeout)
        .max()
        .unwrap_or(model.timeout)
        .max(model.timeout);
    let timeout_seconds = longest_model_timeout
        .as_secs()
        .saturating_add(60)
        .clamp(60, 24 * 60 * 60);
    let spec = SandboxRunSpec {
        schema_version: cowork_contracts::SCHEMA_VERSION,
        run_id: lease.run.spec.id,
        workspace_id: crate::chat_workspaces::for_run(pool, lease.run.spec.id).await?,
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
    crate::organisation_mfa::execution_on(pool, lease.run.spec.id).await?;
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

    let result = send_runner_job(pool, worker_id, lease, runtime, &spec).await?;
    let stderr = redact_secrets(&result.stderr, &secret_redactions);
    if result.timed_out {
        bail!("Crew runtime exceeded its configured timeout");
    }
    if result.exit_code != Some(0) {
        bail!(
            "Crew runtime failed with exit code {:?} (stderr: {} bytes)",
            result.exit_code,
            stderr.len()
        );
    }

    let mut response = None;
    let mut emitted_events = 0_usize;
    let mut noise_lines = 0_usize;
    let mut noise_bytes = 0_usize;
    for line in result.stdout.lines().filter(|line| !line.trim().is_empty()) {
        match classify_crew_protocol_line(line.as_bytes()) {
            CrewProtocolLine::Event(mut value) => {
                redact_secret_values(&mut value, &secret_redactions);
                if emitted_events < 1_000 && serde_json::to_vec(&value)?.len() <= 1024 * 1024 {
                    let event_kind = crew_protocol_run_event_kind(&value);
                    db::append_leased_event(
                        pool,
                        lease.run.spec.id,
                        worker_id,
                        lease.lease_token,
                        None,
                        event_kind,
                        json!({"adapter":"crewai","crew_event":value}),
                    )
                    .await?;
                    emitted_events += 1;
                }
            }
            CrewProtocolLine::Response(mut value) => {
                redact_secret_values(&mut value, &secret_redactions);
                if response.replace(value).is_some() {
                    bail!(
                        "Crew runtime returned more than one response (ignored {noise_lines} console lines totaling {noise_bytes} bytes)"
                    );
                }
            }
            CrewProtocolLine::Noise => {
                noise_lines = noise_lines.saturating_add(1);
                noise_bytes = noise_bytes.saturating_add(line.len());
            }
            CrewProtocolLine::MalformedResponse => {
                bail!(
                    "Crew runtime returned a malformed response frame (ignored {noise_lines} console lines totaling {noise_bytes} bytes)"
                );
            }
        }
    }
    let response = response.with_context(|| {
        format!(
            "Crew runtime returned no response (ignored {noise_lines} console lines totaling {noise_bytes} bytes)"
        )
    })?;
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
    capabilities: &[Capability],
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
                .any(|capability| capability.0 == required)
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
    async fn authorize_dispatch(&self) -> Result<()> {
        crate::managed_resources::execution::authorize(&self.lease.run.spec.input,self.lease.run.spec.creator_user_id)
            .await.map_err(ManagedDispatchDenied)?;
        Ok(())
    }
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
            crate::organisation_mfa::execution_on(self.pool, self.lease.run.spec.id).await?;
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
        // A human approval may have waited while central grants were revoked.
        self.authorize_dispatch().await?;
        self.execute_sandbox_tool(&invocation).await
    }

    async fn checkpoint(&self, state: Value, safe_to_resume: bool) -> Result<()> {
        workflow::create_agent_worker_checkpoint(
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
                        self.lease.lease_token,
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
                    self.lease.lease_token,
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
            workspace_id: crate::chat_workspaces::for_run(self.pool, self.lease.run.spec.id)
                .await?,
            image,
            argv,
            environment: Default::default(),
            stdin_base64,
            network,
            limits,
        };
        let result =
            send_runner_job(self.pool, self.worker_id, self.lease, self.runtime, &spec).await?;
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
                        fetch_runner_file(self.pool, self.runtime, self.lease.run.spec.id, path)
                            .await?;
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
    pool: &PgPool,
    worker_id: Uuid,
    lease: &RunLease,
    runtime: &WorkerRuntime,
    spec: &SandboxRunSpec,
) -> Result<SandboxRunResult> {
    let runner = runtime
        .runner
        .as_ref()
        .context("the Docker sandbox runner is not configured")?;
    let attempt_id = Uuid::new_v4();
    let body = crate::runner_admission::execution_body(spec, lease.lease_token, attempt_id)?;
    db::prepare_runner_request(
        pool,
        lease.run.spec.id,
        worker_id,
        lease.lease_token,
        json!({"phase":"runner_dispatched","attempt_id":attempt_id}),
    )
    .await?;
    db::prepare_runner_attempt(
        pool,
        lease.run.spec.id,
        worker_id,
        lease.lease_token,
        attempt_id,
        spec.workspace_id,
        &body,
    )
    .await?;
    let path = crate::chat_workspaces::jobs_path(spec.workspace_id);
    crate::organisation_mfa::execution_on(pool, lease.run.spec.id).await?;
    let (timestamp, nonce, signature) = runner_signature(&runner.signing_key, "POST", path, &body)?;
    let response = runtime
        .http
        .post(format!("{}{path}", runner.url.trim_end_matches('/')))
        .timeout(Duration::from_secs(24 * 60 * 60))
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
    pool: &PgPool,
    runtime: &WorkerRuntime,
    run_id: Uuid,
    workspace_path: &str,
) -> Result<Vec<u8>> {
    let runner = runtime
        .runner
        .as_ref()
        .context("the Docker sandbox runner is not configured")?;
    let encoded_path = utf8_percent_encode(workspace_path, NON_ALPHANUMERIC).to_string();
    let workspace = crate::chat_workspaces::for_run(pool, run_id).await?;
    let path = crate::chat_workspaces::run_path(
        run_id,
        "file",
        &format!("path={encoded_path}"),
        workspace,
    );
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
    // Ignore any workspace supplied in untrusted raw sandbox input.
    spec.workspace_id = crate::chat_workspaces::for_run(pool, lease.run.spec.id).await?;
    spec.schema_version = cowork_contracts::SCHEMA_VERSION;
    let path = crate::chat_workspaces::jobs_path(spec.workspace_id);
    let mut first_attempt = true;
    let result: SandboxRunResult = loop {
        // The serialized spec and workspace binding remain frozen across waits.
        // Every attempt has a fresh nonce; a previous rejection cannot authorize it.
        let attempt_id = Uuid::new_v4();
        let body = crate::runner_admission::execution_body(&spec, lease.lease_token, attempt_id)?;
        let (timestamp, nonce, signature) =
            runner_signature(&runner.signing_key, "POST", path, &body)?;
        let mut attempt =
            crate::runner_admission::RequestAttempt::new("POST", path, &timestamp, &nonce, &body);
        db::prepare_runner_request(
            pool,
            lease.run.spec.id,
            worker_id,
            lease.lease_token,
            json!({
                "phase": "sandbox_dispatched", "image": spec.image,
                "argv": spec.argv, "network": spec.network,
            }),
        )
        .await?;
        db::prepare_runner_attempt(
            pool,
            lease.run.spec.id,
            worker_id,
            lease.lease_token,
            attempt_id,
            spec.workspace_id,
            &body,
        )
        .await?;
        if first_attempt {
            db::append_leased_event(
                pool,
                lease.run.spec.id,
                worker_id,
                lease.lease_token,
                None,
                RunEventKind::ToolStarted,
                json!({"tool":"sandbox","image":spec.image,"network":spec.network}),
            )
            .await?;
            first_attempt = false;
        }
        // Includes cancellation, current owner/token and live database expiry.
        db::check_runner_capacity_waiting_lease(
            pool,
            lease.run.spec.id,
            worker_id,
            lease.lease_token,
        )
        .await?;
        let response = runtime
            .http
            .post(format!("{}{path}", runner.url.trim_end_matches('/')))
            .timeout(Duration::from_secs(24 * 60 * 60))
            .header("x-cowork-timestamp", &timestamp)
            .header("x-cowork-nonce", &nonce)
            .header("x-cowork-signature", signature)
            .header("content-type", "application/json")
            .body(body.clone())
            .send()
            .await
            .context("runner request failed")?;
        let status = response.status();
        let headers = response.headers().clone();
        let response_body = response.text().await?;
        if status.is_success() {
            break serde_json::from_str(&response_body)
                .context("invalid sandbox runner response")?;
        }
        if !attempt.verify_capacity_no_start(
            &runner.signing_key,
            status.as_u16(),
            &headers,
            response_body.as_bytes(),
        ) {
            return Err(anyhow!(
                "sandbox runner returned {status}: {}",
                truncate(&response_body, 2_000)
            ));
        }
        db::mark_runner_capacity_waiting(pool, lease.run.spec.id, worker_id, lease.lease_token)
            .await?;
        // This future remains inside the existing lease heartbeat and supervisor.
        // No requeue, central reservation release or agent/tool replay occurs.
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_millis(750)).await;
            db::check_runner_capacity_waiting_lease(
                pool,
                lease.run.spec.id,
                worker_id,
                lease.lease_token,
            )
            .await?;
        }
    };
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

/// Claims and renewals mint expiry with database time after this request starts.
/// Conservatively stop at request start plus the granted duration: response
/// latency and any constant database/application clock offset add no authority.
/// Absolute database timestamps remain the database's fencing proof, never a
/// duration computed against the application's independent wall clock.
fn confirmed_lease_deadline(
    lease_duration: Duration,
    request_started: tokio::time::Instant,
) -> Result<tokio::time::Instant> {
    let now = tokio::time::Instant::now();
    let deadline = request_started + lease_duration;
    if deadline <= now {
        bail!("last confirmed worker lease has expired");
    }
    Ok(deadline)
}

/// Renewal lives in the execution future, so cancellation cannot leave a
/// detached heartbeat. A stalled renewal cancels the operation at the last
/// confirmed deadline, including while the database response is still pending.
async fn with_lease_heartbeat<F, R, RF, T>(
    lease_duration: Duration,
    initial_deadline: tokio::time::Instant,
    operation: F,
    mut renew: R,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
    R: FnMut() -> RF,
    RF: Future<Output = Result<tokio::time::Instant>>,
{
    tokio::pin!(operation);
    let heartbeat = async {
        let every = (lease_duration / 3).max(Duration::from_millis(1));
        let mut deadline = initial_deadline;
        loop {
            if deadline <= tokio::time::Instant::now() {
                bail!("last confirmed worker lease has expired");
            }
            let renewal = async {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                tokio::time::sleep(every.min(remaining / 3)).await;
                renew().await
            };
            let next_deadline = tokio::select! {
                biased;
                _ = tokio::time::sleep_until(deadline) => {
                    bail!("worker lease renewal did not finish before the last confirmed lease expired");
                }
                result = tokio::time::timeout_at(deadline, renewal) => {
                    result.context("worker lease renewal did not finish before the last confirmed lease expired")??
                }
            };
            if next_deadline <= tokio::time::Instant::now() {
                bail!("renewal returned an expired worker lease");
            }
            deadline = next_deadline;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {
        biased;
        renewal = heartbeat => {
            renewal?;
            Err(anyhow!("worker lease heartbeat stopped unexpectedly"))
        }
        outcome = &mut operation => outcome,
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn heartbeat_cancellation_drops_operation_and_stops_renewals() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        struct DropMarker(Arc<AtomicBool>);
        impl Drop for DropMarker {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let renewals = Arc::new(AtomicUsize::new(0));
        let operation_dropped = dropped.clone();
        let task_renewals = renewals.clone();
        let worker = tokio::spawn(async move {
            with_lease_heartbeat(
                Duration::from_millis(90),
                tokio::time::Instant::now() + Duration::from_millis(90),
                async move {
                    let _marker = DropMarker(operation_dropped);
                    std::future::pending::<Result<()>>().await
                },
                || async {
                    task_renewals.fetch_add(1, Ordering::SeqCst);
                    Ok(tokio::time::Instant::now() + Duration::from_millis(90))
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while renewals.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        assert!(dropped.load(Ordering::SeqCst));
        let stopped_at = renewals.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(renewals.load(Ordering::SeqCst), stopped_at);
    }

    #[tokio::test]
    async fn lost_lease_cancels_operation_without_waiting_for_it() {
        let outcome = tokio::time::timeout(
            Duration::from_secs(1),
            with_lease_heartbeat(
                Duration::from_millis(90),
                tokio::time::Instant::now() + Duration::from_millis(90),
                std::future::pending::<Result<()>>(),
                || async { Err(anyhow!("lease ownership lost")) },
            ),
        )
        .await
        .unwrap();
        assert_eq!(outcome.unwrap_err().to_string(), "lease ownership lost");
    }

    #[tokio::test]
    async fn blocked_renewal_does_not_block_execution_completion() {
        tokio::time::timeout(
            Duration::from_secs(1),
            with_lease_heartbeat(
                Duration::from_millis(300),
                tokio::time::Instant::now() + Duration::from_millis(300),
                async {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    Ok(())
                },
                std::future::pending::<Result<tokio::time::Instant>>,
            ),
        )
        .await
        .unwrap()
        .unwrap();
    }

    #[tokio::test]
    async fn stalled_renewal_cancels_execution_at_last_confirmed_deadline() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let continued = Arc::new(AtomicBool::new(false));
        let operation_continued = continued.clone();
        let outcome = tokio::time::timeout(
            Duration::from_secs(1),
            with_lease_heartbeat(
                Duration::from_millis(90),
                tokio::time::Instant::now() + Duration::from_millis(90),
                async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    operation_continued.store(true, Ordering::SeqCst);
                    Ok(())
                },
                std::future::pending::<Result<tokio::time::Instant>>,
            ),
        )
        .await
        .unwrap();
        assert!(outcome
            .unwrap_err()
            .to_string()
            .contains("last confirmed lease expired"));
        assert!(!continued.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn successful_renewal_extends_execution_authority() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let renewals = AtomicUsize::new(0);
        tokio::time::timeout(
            Duration::from_secs(2),
            with_lease_heartbeat(
                Duration::from_millis(120),
                tokio::time::Instant::now() + Duration::from_millis(120),
                async {
                    tokio::time::sleep(Duration::from_millis(350)).await;
                    Ok(())
                },
                || async {
                    renewals.fetch_add(1, Ordering::SeqCst);
                    Ok(tokio::time::Instant::now() + Duration::from_millis(120))
                },
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(renewals.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn expired_authority_takes_priority_over_ready_execution() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dispatched = AtomicBool::new(false);
        let outcome = with_lease_heartbeat(
            Duration::from_millis(90),
            tokio::time::Instant::now(),
            async {
                dispatched.store(true, Ordering::SeqCst);
                Ok(())
            },
            std::future::pending::<Result<tokio::time::Instant>>,
        )
        .await;
        assert!(outcome.is_err());
        assert!(!dispatched.load(Ordering::SeqCst));
    }

    #[test]
    fn lease_response_latency_never_adds_execution_authority() {
        let requested = tokio::time::Instant::now() - Duration::from_millis(100);
        assert!(confirmed_lease_deadline(Duration::from_millis(30), requested).is_err());
        let duration = Duration::from_secs(30);
        assert_eq!(
            confirmed_lease_deadline(duration, requested).unwrap(),
            requested + duration,
            "a delayed claim must retain its original request deadline"
        );
    }

    #[tokio::test]
    async fn delayed_initial_claim_cancels_execution_without_a_successful_renewal() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let continued = AtomicBool::new(false);
        let duration = Duration::from_millis(120);
        let requested = tokio::time::Instant::now() - Duration::from_millis(80);
        let initial_deadline = confirmed_lease_deadline(duration, requested).unwrap();
        assert_eq!(initial_deadline, requested + duration);
        let outcome = tokio::time::timeout(
            Duration::from_secs(1),
            with_lease_heartbeat(
                duration,
                initial_deadline,
                async {
                    // This would fit a fresh duration measured from delivery,
                    // but cannot fit the remainder of the original claim.
                    tokio::time::sleep(Duration::from_millis(90)).await;
                    continued.store(true, Ordering::SeqCst);
                    Ok(())
                },
                std::future::pending::<Result<tokio::time::Instant>>,
            ),
        )
        .await
        .unwrap();
        assert!(outcome.is_err());
        assert!(!continued.load(Ordering::SeqCst));
    }

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
                {"id":"researcher","tools":["read_file","office_workflow"],"mcpServerNames":["Docs"]},
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

        let secrets = inject_crew_mcp_context(
            &mut request,
            &bindings,
            creator_user_id,
            &[Capability::from("files"), Capability::from("office.ooxml")],
        )
        .unwrap();

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
            json!(["read_file", "office_workflow"])
        );
        assert_eq!(request["agents"][0]["allowDelegation"], false);
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
