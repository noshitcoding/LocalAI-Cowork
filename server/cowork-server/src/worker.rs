use std::{
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

use crate::{config::Config, db, desktop, governance, storage, workflow};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct WorkerRuntime {
    http: Client,
    agent: Option<Arc<AgentRuntime>>,
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
    let mut reap_counter = 0_u8;
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
        if reap_counter % 60 == 0 {
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
        if runtime.agent.is_none() && runtime.runner.is_none() {
            continue;
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
    let agent = runtime
        .agent
        .as_ref()
        .context("the server model provider is not configured")?;
    let host = ServerRuntimeHost {
        pool,
        worker_id,
        runtime,
        lease,
    };
    let result = agent.execute(&lease.run.spec, &host).await?;
    Ok(serde_json::to_value(result)?)
}

struct ServerRuntimeHost<'a> {
    pool: &'a PgPool,
    worker_id: Uuid,
    runtime: &'a WorkerRuntime,
    lease: &'a RunLease,
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
        let failed = result.timed_out || result.exit_code != Some(0);
        let content = if failed {
            format!("stdout:\n{}\nstderr:\n{}", result.stdout, result.stderr)
        } else if result.stderr.is_empty() {
            result.stdout.clone()
        } else {
            format!("{}\n[stderr]\n{}", result.stdout, result.stderr)
        };
        // Browser and Office adapters emit structured diagnostics even when an
        // operation fails. Persisting those artifacts is essential for review
        // and must not depend on a zero exit code.
        let structured =
            if invocation.name.starts_with("Browser") || invocation.name.starts_with("Office") {
                serde_json::from_str::<Value>(&result.stdout).unwrap_or(Value::Null)
            } else {
                Value::Null
            };
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
}
