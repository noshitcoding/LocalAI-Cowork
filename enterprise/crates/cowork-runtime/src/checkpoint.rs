//! Versioned execution cursor. Only a host-proven safe record may be resumed.
use anyhow::{bail, ensure, Context, Result};
use cowork_contracts::RunSpec;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    initial_messages, AgentResult, ModelConfig, ModelMessage, ModelToolCall, ToolDefinition,
};

pub const MAX_CHECKPOINT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelBinding {
    pub model: String,
    pub endpoint_sha256: String,
    pub max_steps: usize,
    pub verify_tls_certificates: bool,
}

impl ModelBinding {
    pub fn new(config: &ModelConfig) -> Result<Self> {
        let endpoint = reqwest::Url::parse(&config.base_url).context("invalid model endpoint")?;
        let normalized = endpoint.as_str().trim_end_matches('/');
        Ok(Self {
            model: config.model.clone(),
            endpoint_sha256: format!("{:x}", Sha256::digest(normalized.as_bytes())),
            max_steps: config.max_steps,
            verify_tls_certificates: config.verify_tls_certificates,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingBatch {
    pub step: usize,
    pub calls: Vec<ModelToolCall>,
    pub next_call: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StepUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExecutionCheckpoint {
    pub version: u8,
    pub run_id: uuid::Uuid,
    pub run_spec: Value,
    pub model: ModelBinding,
    pub tools: Value,
    pub initial_messages: Vec<ModelMessage>,
    pub messages: Vec<ModelMessage>,
    pub next_step: usize,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub usage: Vec<StepUsage>,
    pub pending: Option<PendingBatch>,
    pub finished: Option<AgentResult>,
    pub safe_to_resume: bool,
}

impl ExecutionCheckpoint {
    pub fn initial(run: &RunSpec, config: &ModelConfig, tools: &[ToolDefinition]) -> Result<Self> {
        let messages = initial_messages(run);
        let value = Self {
            version: 1,
            run_id: run.id,
            run_spec: serde_json::to_value(run)?,
            model: ModelBinding::new(config)?,
            tools: serde_json::to_value(tools)?,
            initial_messages: messages.clone(),
            messages,
            next_step: 1,
            prompt_tokens: 0,
            completion_tokens: 0,
            usage: Vec::new(),
            pending: None,
            finished: None,
            safe_to_resume: true,
        };
        value.value()?;
        Ok(value)
    }

    pub fn value(&self) -> Result<Value> {
        let bytes = serde_json::to_vec(self)?;
        ensure!(
            bytes.len() <= MAX_CHECKPOINT_BYTES,
            "checkpoint context exceeds 16 MiB"
        );
        if self.safe_to_resume {
            self.validate_run(&serde_json::from_value(self.run_spec.clone())?)?;
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn parse(run: &RunSpec, value: &Value) -> Result<Self> {
        ensure!(
            serde_json::to_vec(value)?.len() <= MAX_CHECKPOINT_BYTES,
            "checkpoint context exceeds 16 MiB"
        );
        let state: Self =
            serde_json::from_value(value.clone()).context("invalid execution checkpoint")?;
        state.validate_run(run)?;
        Ok(state)
    }

    fn validate_run(&self, run: &RunSpec) -> Result<()> {
        ensure!(
            self.version == 1 && self.safe_to_resume,
            "checkpoint is not a supported safe execution cursor"
        );
        ensure!(
            self.run_id == run.id && self.run_spec == serde_json::to_value(run)?,
            "checkpoint run binding changed"
        );
        ensure!(
            self.initial_messages == initial_messages(run)
                && self.messages.starts_with(&self.initial_messages),
            "checkpoint initial conversation changed"
        );
        ensure!(
            (1..=512).contains(&self.model.max_steps)
                && (1..=self.model.max_steps + 1).contains(&self.next_step),
            "checkpoint step budget is invalid"
        );
        ensure!(
            self.model.endpoint_sha256.len() == 64
                && self
                    .model
                    .endpoint_sha256
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "checkpoint model endpoint binding is invalid"
        );
        let definitions: Vec<ToolDefinition> = serde_json::from_value(self.tools.clone())?;
        crate::validate_tools(&definitions)?;
        let tail = &self.messages[self.initial_messages.len()..];
        let mut position = 0;
        let mut steps = 0;
        let mut final_seen = false;
        while position < tail.len() {
            let assistant = &tail[position];
            ensure!(
                assistant.role == "assistant" && assistant.tool_call_id.is_none(),
                "checkpoint conversation cursor is invalid"
            );
            steps += 1;
            position += 1;
            let mut ids = std::collections::HashSet::new();
            for call in &assistant.tool_calls {
                ensure!(
                    !call.id.is_empty()
                        && call.id.len() <= 256
                        && call.r#type == "function"
                        && ids.insert(&call.id),
                    "checkpoint tool batch identity is invalid"
                );
            }
            let mut completed = 0;
            while completed < assistant.tool_calls.len()
                && position < tail.len()
                && tail[position].role == "tool"
            {
                let output = &tail[position];
                ensure!(
                    output.tool_call_id.as_ref() == Some(&assistant.tool_calls[completed].id)
                        && output.tool_calls.is_empty()
                        && output.content.as_ref().is_some_and(Value::is_string),
                    "checkpoint completed tool cursor is invalid"
                );
                completed += 1;
                position += 1;
            }
            if completed < assistant.tool_calls.len() {
                let pending = self
                    .pending
                    .as_ref()
                    .context("checkpoint has unresolved calls without a cursor")?;
                ensure!(
                    position == tail.len()
                        && pending.step == steps
                        && pending.next_call == completed
                        && serde_json::to_value(&pending.calls)?
                            == serde_json::to_value(&assistant.tool_calls)?,
                    "checkpoint pending tool cursor is invalid"
                );
            } else {
                ensure!(
                    self.pending
                        .as_ref()
                        .is_none_or(|pending| pending.step != steps),
                    "checkpoint repeats completed tool calls"
                );
            }
            if assistant.tool_calls.is_empty() {
                final_seen = true;
                let result = self
                    .finished
                    .as_ref()
                    .context("checkpoint final response is missing")?;
                ensure!(
                    position == tail.len()
                        && result.steps == steps
                        && result.content
                            == assistant
                                .content
                                .as_ref()
                                .and_then(Value::as_str)
                                .unwrap_or("")
                        && result.prompt_tokens == self.prompt_tokens
                        && result.completion_tokens == self.completion_tokens,
                    "checkpoint final response binding changed"
                );
            }
        }
        ensure!(
            self.next_step == steps + 1,
            "checkpoint model step cursor changed"
        );
        ensure!(
            self.usage.len() == steps
                && self
                    .usage
                    .iter()
                    .fold(0_u64, |sum, usage| sum.saturating_add(usage.prompt_tokens))
                    == self.prompt_tokens
                && self.usage.iter().fold(0_u64, |sum, usage| sum
                    .saturating_add(usage.completion_tokens))
                    == self.completion_tokens,
            "checkpoint token accounting changed"
        );
        ensure!(
            final_seen == self.finished.is_some(),
            "checkpoint final cursor changed"
        );
        ensure!(
            self.pending
                .as_ref()
                .is_none_or(|batch| batch.step == steps && batch.next_call < batch.calls.len()),
            "checkpoint pending batch is invalid"
        );
        ensure!(
            self.finished.is_none() || (self.pending.is_none() && steps > 0),
            "checkpoint completion cursor is invalid"
        );
        Ok(())
    }

    pub fn validate_runtime(&self, config: &ModelConfig, tools: &[ToolDefinition]) -> Result<()> {
        ensure!(
            self.model == ModelBinding::new(config)?,
            "checkpoint model configuration changed"
        );
        ensure!(
            self.tools == serde_json::to_value(tools)?,
            "checkpoint advertised tools changed"
        );
        if self.finished.is_none() && self.pending.is_none() && self.next_step > config.max_steps {
            bail!(
                "agent exceeded the configured maximum of {} model steps",
                config.max_steps
            );
        }
        Ok(())
    }
}

/// Structural/run validation for a trusted server provenance gate. Current
/// model/tool configuration is additionally verified by AgentRuntime::resume.
pub fn validate_for_run(run: &RunSpec, value: &Value) -> Result<()> {
    ExecutionCheckpoint::parse(run, value).map(|_| ())
}

pub fn validate_record_for_run(run: &RunSpec, value: &Value, safe: bool) -> Result<()> {
    ensure!(
        serde_json::to_vec(value)?.len() <= MAX_CHECKPOINT_BYTES,
        "checkpoint context exceeds 16 MiB"
    );
    let mut state: ExecutionCheckpoint = serde_json::from_value(value.clone())?;
    ensure!(
        state.safe_to_resume == safe,
        "checkpoint safety annotation changed"
    );
    state.safe_to_resume = true;
    state.validate_run(run)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentRuntime, RuntimeHost, ToolInvocation, ToolOutput};
    use async_trait::async_trait;
    use axum::{extract::State, routing::post, Json, Router};
    use std::sync::{Arc, Mutex};

    fn run() -> RunSpec {
        let id = uuid::Uuid::new_v4();
        RunSpec {
            schema_version: cowork_contracts::SCHEMA_VERSION,
            id,
            thread_id: uuid::Uuid::new_v4(),
            project_id: uuid::Uuid::new_v4(),
            project: cowork_contracts::FrozenReference { id, revision: 1 },
            project_privacy: cowork_contracts::ProjectPrivacy::TeamManaged,
            task: None,
            creator_user_id: uuid::Uuid::new_v4(),
            executor_target: cowork_contracts::ExecutorTarget::ServerLinux { pool_id: None },
            required_capabilities: Vec::new(),
            input: serde_json::json!({"prompt":"complete A then B"}),
            model_profile_id: None,
            snapshot_id: None,
            idempotency_key: "resume-test".into(),
            created_at: chrono::Utc::now(),
        }
    }

    struct Host {
        invocations: Arc<Mutex<Vec<String>>>,
        checkpoints: Arc<Mutex<Vec<Value>>>,
        break_after_a: bool,
    }
    #[async_trait]
    impl RuntimeHost for Host {
        fn tools(&self) -> Vec<ToolDefinition> {
            ["A", "B"]
                .into_iter()
                .map(|name| ToolDefinition {
                    name: name.into(),
                    description: name.into(),
                    input_schema: serde_json::json!({"type":"object"}),
                    required_capability: None,
                    mutating: true,
                })
                .collect()
        }
        async fn emit(&self, _: cowork_contracts::RunEventKind, _: Value) -> Result<()> {
            Ok(())
        }
        async fn execute_tool(&self, call: ToolInvocation) -> Result<ToolOutput> {
            self.invocations.lock().unwrap().push(call.name.clone());
            Ok(ToolOutput {
                content: format!("{} done", call.name),
                is_error: false,
                safe_to_resume: true,
                metadata: Value::Null,
            })
        }
        async fn checkpoint(&self, value: Value, safe: bool) -> Result<()> {
            self.checkpoints.lock().unwrap().push(value.clone());
            if self.break_after_a && safe && value["pending"]["next_call"] == 1 {
                bail!("simulated worker interruption after durable A");
            }
            Ok(())
        }
    }

    async fn model(calls: Arc<Mutex<Vec<Value>>>) -> (ModelConfig, tokio::task::JoinHandle<()>) {
        async fn complete(
            State(calls): State<Arc<Mutex<Vec<Value>>>>,
            Json(input): Json<Value>,
        ) -> Json<Value> {
            let mut calls = calls.lock().unwrap();
            calls.push(input);
            if calls.len() == 1 {
                Json(
                    serde_json::json!({"choices":[{"message":{"content":null,"tool_calls":[
                    {"id":"a-1","type":"function","function":{"name":"A","arguments":"{}"}},
                    {"id":"b-1","type":"function","function":{"name":"B","arguments":"{}"}}
                ]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":7,"completion_tokens":3}}),
                )
            } else {
                Json(
                    serde_json::json!({"choices":[{"message":{"content":"finished"},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":11,"completion_tokens":5}}),
                )
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/v1/chat/completions", post(complete))
            .with_state(calls);
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (
            ModelConfig {
                base_url: url,
                api_key: Some("private-provider-key".into()),
                model: "fixture".into(),
                timeout: std::time::Duration::from_secs(5),
                max_steps: 2,
                verify_tls_certificates: true,
            },
            task,
        )
    }

    #[tokio::test]
    async fn resumes_pending_b_without_repeating_completed_a_and_preserves_usage() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (config, server) = model(calls.clone()).await;
        let runtime = AgentRuntime::new(config).unwrap();
        let run = run();
        let invocations = Arc::new(Mutex::new(Vec::new()));
        let checkpoints = Arc::new(Mutex::new(Vec::new()));
        let interrupted = Host {
            invocations: invocations.clone(),
            checkpoints: checkpoints.clone(),
            break_after_a: true,
        };
        assert!(runtime.execute(&run, &interrupted).await.is_err());
        let checkpoint = checkpoints.lock().unwrap().last().unwrap().clone();
        assert_eq!(checkpoint["pending"]["next_call"], 1);
        assert!(!checkpoint.to_string().contains("private-provider-key"));
        let continued = Host {
            break_after_a: false,
            ..interrupted
        };
        let result = runtime.resume(&run, &continued, checkpoint).await.unwrap();
        assert_eq!(*invocations.lock().unwrap(), ["A", "B"]);
        assert_eq!(
            (result.steps, result.prompt_tokens, result.completion_tokens),
            (2, 18, 8)
        );
        let finished = checkpoints.lock().unwrap().last().unwrap().clone();
        let repeated = runtime.resume(&run, &continued, finished).await.unwrap();
        assert_eq!(
            (
                repeated.content,
                repeated.steps,
                repeated.prompt_tokens,
                repeated.completion_tokens
            ),
            (result.content, 2, 18, 8)
        );
        assert_eq!(*invocations.lock().unwrap(), ["A", "B"]);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        let messages = calls[1]["messages"].as_array().unwrap();
        assert!(messages
            .iter()
            .any(|message| message["tool_call_id"] == "a-1" && message["content"] == "A done"));
        assert!(messages
            .iter()
            .any(|message| message["tool_call_id"] == "b-1" && message["content"] == "B done"));
        server.abort();
    }

    #[tokio::test]
    async fn pending_batch_uses_existing_step_then_exhausted_budget_cannot_call_model_again() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (mut config, server) = model(calls.clone()).await;
        config.max_steps = 1;
        let runtime = AgentRuntime::new(config).unwrap();
        let run = run();
        let invocations = Arc::new(Mutex::new(Vec::new()));
        let checkpoints = Arc::new(Mutex::new(Vec::new()));
        let host = Host {
            invocations: invocations.clone(),
            checkpoints: checkpoints.clone(),
            break_after_a: true,
        };
        assert!(runtime.execute(&run, &host).await.is_err());
        let checkpoint = checkpoints.lock().unwrap().last().unwrap().clone();
        let host = Host {
            break_after_a: false,
            ..host
        };
        let error = runtime.resume(&run, &host, checkpoint).await.unwrap_err();
        assert!(error.to_string().contains("maximum of 1 model steps"));
        assert_eq!(*invocations.lock().unwrap(), ["A", "B"]);
        assert_eq!(calls.lock().unwrap().len(), 1);
        let value = checkpoints.lock().unwrap().last().unwrap().clone();
        assert_eq!(value["prompt_tokens"], 7);
        assert_eq!(value["completion_tokens"], 3);
        assert!(runtime.resume(&run, &host, value).await.is_err());
        assert_eq!(calls.lock().unwrap().len(), 1);
        server.abort();
    }

    #[tokio::test]
    async fn changed_bindings_unsafe_state_and_forged_cursor_are_rejected_before_effects() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (config, server) = model(calls.clone()).await;
        let runtime = AgentRuntime::new(config.clone()).unwrap();
        let run = run();
        let host = Host {
            invocations: Arc::new(Mutex::new(Vec::new())),
            checkpoints: Arc::new(Mutex::new(Vec::new())),
            break_after_a: true,
        };
        assert!(runtime.execute(&run, &host).await.is_err());
        let value = host.checkpoints.lock().unwrap().last().unwrap().clone();
        for (field, replacement) in [
            ("version", serde_json::json!(2)),
            ("run_id", serde_json::json!(uuid::Uuid::new_v4())),
            ("safe_to_resume", serde_json::json!(false)),
            ("next_step", serde_json::json!(1)),
            ("prompt_tokens", serde_json::json!(0)),
            ("completion_tokens", serde_json::json!(99)),
            ("usage", serde_json::json!([])),
        ] {
            let mut invalid = value.clone();
            invalid[field] = replacement;
            assert!(runtime.resume(&run, &host, invalid).await.is_err());
        }
        let mut invalid = value.clone();
        invalid["pending"]["next_call"] = serde_json::json!(0);
        assert!(runtime.resume(&run, &host, invalid).await.is_err());
        let mut invalid = value.clone();
        invalid["initial_messages"][0]["content"] = serde_json::json!("changed");
        assert!(runtime.resume(&run, &host, invalid).await.is_err());
        let mut changed = config;
        changed.max_steps = 3;
        assert!(AgentRuntime::new(changed)
            .unwrap()
            .resume(&run, &host, value.clone())
            .await
            .is_err());
        let mut invalid = value;
        invalid["tools"][0]["mutating"] = serde_json::json!(false);
        assert!(runtime.resume(&run, &host, invalid).await.is_err());
        assert_eq!(*host.invocations.lock().unwrap(), ["A"]);
        assert_eq!(calls.lock().unwrap().len(), 1);
        server.abort();
    }

    #[test]
    fn endpoint_credentials_are_digest_only_and_context_is_bounded() {
        let run = run();
        let config = ModelConfig {
            base_url:
                "https://private-user:private-url-password@provider.invalid/v1?secret=private-query"
                    .into(),
            api_key: Some("private-api-key".into()),
            model: "fixture".into(),
            timeout: std::time::Duration::from_secs(1),
            max_steps: 2,
            verify_tls_certificates: true,
        };
        let state = ExecutionCheckpoint::initial(&run, &config, &[]).unwrap();
        let value = state.value().unwrap();
        let serialized = value.to_string();
        for secret in [
            "private-user",
            "private-url-password",
            "private-query",
            "private-api-key",
            "provider.invalid",
        ] {
            assert!(!serialized.contains(secret));
        }
        let mut rotated = config.clone();
        rotated.api_key = Some("rotated-provider-secret".into());
        assert!(state.validate_runtime(&rotated, &[]).is_ok());
        rotated.base_url.push_str("-changed");
        assert!(state.validate_runtime(&rotated, &[]).is_err());
        let mut oversized = value;
        oversized["messages"][0]["content"] = serde_json::json!("a".repeat(MAX_CHECKPOINT_BYTES));
        assert!(ExecutionCheckpoint::parse(&run, &oversized)
            .unwrap_err()
            .to_string()
            .contains("16 MiB"));
    }
}
