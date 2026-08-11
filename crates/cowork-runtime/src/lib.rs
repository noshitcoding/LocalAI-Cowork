use std::{collections::HashMap, time::Duration};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use cowork_contracts::{Capability, RunEventKind, RunSpec};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub mod crew;

const MAX_MODEL_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub timeout: Duration,
    pub max_steps: usize,
    pub verify_tls_certificates: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub required_capability: Option<Capability>,
    #[serde(default)]
    pub mutating: bool,
}

#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub safe_to_resume: bool,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub content: String,
    pub steps: usize,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[async_trait]
pub trait RuntimeHost: Send + Sync {
    fn tools(&self) -> Vec<ToolDefinition>;

    async fn emit(&self, kind: RunEventKind, payload: Value) -> Result<()>;

    async fn execute_tool(&self, invocation: ToolInvocation) -> Result<ToolOutput>;

    async fn checkpoint(&self, state: Value, safe_to_resume: bool) -> Result<()>;
}

pub struct AgentRuntime {
    http: Client,
    config: ModelConfig,
}

impl AgentRuntime {
    pub fn new(config: ModelConfig) -> Result<Self> {
        if config.model.trim().is_empty() {
            bail!("model name cannot be empty");
        }
        if config.max_steps == 0 || config.max_steps > 512 {
            bail!("max_steps must be between 1 and 512");
        }
        let http = Client::builder()
            .timeout(config.timeout)
            .danger_accept_invalid_certs(!config.verify_tls_certificates)
            .build()?;
        Ok(Self { http, config })
    }

    pub async fn execute<H: RuntimeHost>(&self, run: &RunSpec, host: &H) -> Result<AgentResult> {
        let tools = host.tools();
        validate_tools(&tools)?;
        let advertised: HashMap<&str, &ToolDefinition> = tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool))
            .collect();
        let mut messages = initial_messages(run);
        let mut prompt_tokens = 0_u64;
        let mut completion_tokens = 0_u64;

        for step in 1..=self.config.max_steps {
            host.emit(
                RunEventKind::ModelStarted,
                json!({"model": self.config.model, "step": step}),
            )
            .await?;
            let response = self.complete(&messages, &tools).await?;
            prompt_tokens = prompt_tokens.saturating_add(response.usage.prompt_tokens);
            completion_tokens = completion_tokens.saturating_add(response.usage.completion_tokens);
            let choice = response
                .choices
                .into_iter()
                .next()
                .context("model response did not contain a choice")?;
            let content = choice.message.text_content();
            let tool_calls = choice.message.tool_calls.clone();
            host.emit(
                RunEventKind::ModelCompleted,
                json!({
                    "model": self.config.model,
                    "step": step,
                    "content": content,
                    "tool_calls": tool_calls,
                    "finish_reason": choice.finish_reason,
                    "usage": response.usage,
                }),
            )
            .await?;
            messages.push(ModelMessage::assistant(content.clone(), tool_calls.clone()));

            if tool_calls.is_empty() {
                return Ok(AgentResult {
                    content,
                    steps: step,
                    prompt_tokens,
                    completion_tokens,
                });
            }

            for call in tool_calls {
                let Some(definition) = advertised.get(call.function.name.as_str()) else {
                    let output = ToolOutput {
                        content: format!("Unknown tool: {}", call.function.name),
                        is_error: true,
                        safe_to_resume: true,
                        metadata: Value::Null,
                    };
                    messages.push(ModelMessage::tool(call.id, output.content.clone()));
                    continue;
                };
                let arguments = match serde_json::from_str::<Value>(&call.function.arguments) {
                    Ok(Value::Object(arguments)) => Value::Object(arguments),
                    Ok(_) => json!({"_invalid_arguments": "tool arguments must be an object"}),
                    Err(error) => json!({"_invalid_arguments": error.to_string()}),
                };
                host.emit(
                    RunEventKind::ToolStarted,
                    json!({
                        "tool_call_id": call.id,
                        "tool": definition.name,
                        "arguments": arguments,
                        "mutating": definition.mutating,
                    }),
                )
                .await?;
                let invocation = ToolInvocation {
                    id: call.id.clone(),
                    name: call.function.name.clone(),
                    arguments,
                };
                let output = match host.execute_tool(invocation).await {
                    Ok(output) => output,
                    Err(error) => ToolOutput {
                        content: format!("Tool execution error: {error:#}"),
                        is_error: true,
                        safe_to_resume: false,
                        metadata: Value::Null,
                    },
                };
                host.emit(
                    if output.is_error {
                        RunEventKind::ToolFailed
                    } else {
                        RunEventKind::ToolCompleted
                    },
                    json!({
                        "tool_call_id": call.id,
                        "tool": definition.name,
                        "content": output.content,
                        "metadata": output.metadata,
                        "safe_to_resume": output.safe_to_resume,
                    }),
                )
                .await?;
                messages.push(ModelMessage::tool(call.id, output.content));
                host.checkpoint(
                    json!({"step": step, "messages": messages}),
                    output.safe_to_resume,
                )
                .await?;
                if !output.safe_to_resume {
                    bail!(
                        "tool {} ended without a safe resume point; manual review is required",
                        definition.name
                    );
                }
            }
        }
        bail!(
            "agent exceeded the configured maximum of {} model steps",
            self.config.max_steps
        )
    }

    async fn complete(
        &self,
        messages: &[ModelMessage],
        tools: &[ToolDefinition],
    ) -> Result<ChatCompletionResponse> {
        let request = ChatCompletionRequest {
            model: &self.config.model,
            messages,
            tools: tools.iter().map(OpenAiTool::from).collect(),
            tool_choice: if tools.is_empty() { None } else { Some("auto") },
        };
        let endpoint = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut builder = self.http.post(endpoint).json(&request);
        if let Some(api_key) = &self.config.api_key {
            builder = builder.bearer_auth(api_key);
        }
        let response = builder.send().await.context("model request failed")?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_MODEL_RESPONSE_BYTES)
        {
            bail!("model response exceeds {MAX_MODEL_RESPONSE_BYTES} bytes");
        }
        let bytes = response
            .bytes()
            .await
            .context("failed to read model response")?;
        if bytes.len() as u64 > MAX_MODEL_RESPONSE_BYTES {
            bail!("model response exceeds {MAX_MODEL_RESPONSE_BYTES} bytes");
        }
        if !status.is_success() {
            bail!(
                "model endpoint returned {status}: {}",
                String::from_utf8_lossy(&bytes)
                    .chars()
                    .take(2_000)
                    .collect::<String>()
            );
        }
        serde_json::from_slice(&bytes).context("invalid chat completion response")
    }
}

fn initial_messages(run: &RunSpec) -> Vec<ModelMessage> {
    let base_system = run
        .input
        .get("system_prompt")
        .and_then(Value::as_str)
        .unwrap_or(
            "You are Open Cowork. Complete the user's task using the available tools. Keep all file operations inside the assigned workspace. Inspect results before finishing.",
        );
    let project_instructions = run
        .input
        .get("current_project_instructions")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let system = project_instructions
        .map(|instructions| {
            format!("{base_system}\n\nCurrent project instructions:\n{instructions}")
        })
        .unwrap_or_else(|| base_system.to_owned());
    let run_prompt = run
        .input
        .get("prompt")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| run.input.to_string());
    let prompt = run
        .input
        .get("task_instructions")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|instructions| {
            if run_prompt.trim() == instructions {
                run_prompt.clone()
            } else {
                format!("{instructions}\n\nRun input:\n{run_prompt}")
            }
        })
        .unwrap_or(run_prompt);
    let mut messages = vec![ModelMessage::system(&system)];
    if let Some(history) = run.input.get("messages").and_then(Value::as_array) {
        for item in history {
            let Some(role) = item.get("role").and_then(Value::as_str) else {
                continue;
            };
            let Some(content) = item.get("content").and_then(Value::as_str) else {
                continue;
            };
            if content.trim().is_empty() {
                continue;
            }
            match role {
                "system" => messages.push(ModelMessage::system(content)),
                "assistant" => {
                    messages.push(ModelMessage::assistant(content.to_owned(), Vec::new()))
                }
                "user" => messages.push(ModelMessage::user(content.to_owned())),
                _ => {}
            }
        }
    }
    messages.push(ModelMessage::user(prompt));
    messages
}

fn validate_tools(tools: &[ToolDefinition]) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for tool in tools {
        if tool.name.is_empty()
            || tool.name.len() > 128
            || !tool
                .name
                .chars()
                .all(|character| character == '_' || character.is_ascii_alphanumeric())
        {
            bail!("invalid tool name {}", tool.name);
        }
        if !names.insert(tool.name.as_str()) {
            bail!("duplicate tool name {}", tool.name);
        }
        if !tool.input_schema.is_object() {
            bail!("tool {} input_schema must be an object", tool.name);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: &'a [ModelMessage],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiTool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    tool_calls: Vec<ModelToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

impl ModelMessage {
    fn system(content: &str) -> Self {
        Self::plain("system", content.to_owned())
    }

    fn user(content: String) -> Self {
        Self::plain("user", content)
    }

    fn assistant(content: String, tool_calls: Vec<ModelToolCall>) -> Self {
        Self {
            role: "assistant",
            content: (!content.is_empty()).then_some(Value::String(content)),
            tool_calls,
            tool_call_id: None,
        }
    }

    fn tool(tool_call_id: String, content: String) -> Self {
        Self {
            role: "tool",
            content: Some(Value::String(content)),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id),
        }
    }

    fn plain(role: &'static str, content: String) -> Self {
        Self {
            role,
            content: Some(Value::String(content)),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct OpenAiTool<'a> {
    r#type: &'static str,
    function: OpenAiFunction<'a>,
}

#[derive(Debug, Clone, Serialize)]
struct OpenAiFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a Value,
}

impl<'a> From<&'a ToolDefinition> for OpenAiTool<'a> {
    fn from(tool: &'a ToolDefinition) -> Self {
        Self {
            r#type: "function",
            function: OpenAiFunction {
                name: &tool.name,
                description: &tool.description,
                parameters: &tool.input_schema,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Usage,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ModelResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelResponseMessage {
    content: Option<Value>,
    #[serde(default)]
    tool_calls: Vec<ModelToolCall>,
}

impl ModelResponseMessage {
    fn text_content(&self) -> String {
        match &self.content {
            Some(Value::String(content)) => content.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelToolCall {
    id: String,
    #[serde(default = "function_type")]
    r#type: String,
    function: ModelFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelFunctionCall {
    name: String,
    arguments: String,
}

fn function_type() -> String {
    "function".to_owned()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use axum::{extract::State, routing::post, Json, Router};
    use chrono::Utc;
    use cowork_contracts::{
        ExecutorTarget, FrozenReference, ProjectPrivacy, RunSpec, SCHEMA_VERSION,
    };
    use uuid::Uuid;

    #[test]
    fn reads_text_parts_from_openai_compatible_content() {
        let message = ModelResponseMessage {
            content: Some(json!([
                {"type": "text", "text": "hello "},
                {"type": "text", "text": "world"}
            ])),
            tool_calls: Vec::new(),
        };
        assert_eq!(message.text_content(), "hello world");
    }

    #[test]
    fn rejects_duplicate_tool_names() {
        let tool = ToolDefinition {
            name: "Read".to_owned(),
            description: "read".to_owned(),
            input_schema: json!({"type": "object"}),
            required_capability: None,
            mutating: false,
        };
        assert!(validate_tools(&[tool.clone(), tool]).is_err());
    }

    #[test]
    fn includes_frozen_conversation_history_before_the_current_prompt() {
        let id = Uuid::new_v4();
        let run = RunSpec {
            schema_version: SCHEMA_VERSION,
            id,
            thread_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            project: FrozenReference { id, revision: 1 },
            project_privacy: ProjectPrivacy::PrivateLocal,
            task: None,
            creator_user_id: Uuid::new_v4(),
            executor_target: ExecutorTarget::PersonalDevice {
                device_id: Uuid::new_v4(),
            },
            required_capabilities: Vec::new(),
            input: json!({
                "system_prompt": "system override",
                "current_project_instructions": "latest project rules",
                "messages": [
                    {"role": "user", "content": "earlier question"},
                    {"role": "assistant", "content": "earlier answer"}
                ],
                "task_instructions": "frozen task instructions",
                "prompt": "current question"
            }),
            model_profile_id: None,
            snapshot_id: None,
            idempotency_key: "history-test".to_owned(),
            created_at: Utc::now(),
        };
        let messages = initial_messages(&run);
        assert_eq!(messages.len(), 4);
        assert_eq!(
            messages[0].content,
            Some(Value::String(
                "system override\n\nCurrent project instructions:\nlatest project rules".to_owned()
            ))
        );
        assert_eq!(
            messages[1].content,
            Some(Value::String("earlier question".to_owned()))
        );
        assert_eq!(
            messages[2].content,
            Some(Value::String("earlier answer".to_owned()))
        );
        assert_eq!(
            messages[3].content,
            Some(Value::String(
                "frozen task instructions\n\nRun input:\ncurrent question".to_owned()
            ))
        );
    }

    struct UnsafeHost {
        checkpoints: Arc<Mutex<Vec<bool>>>,
    }

    #[async_trait]
    impl RuntimeHost for UnsafeHost {
        fn tools(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "Unsafe".to_owned(),
                description: "uncertain operation".to_owned(),
                input_schema: json!({"type":"object"}),
                required_capability: None,
                mutating: true,
            }]
        }

        async fn emit(&self, _kind: RunEventKind, _payload: Value) -> Result<()> {
            Ok(())
        }

        async fn execute_tool(&self, _invocation: ToolInvocation) -> Result<ToolOutput> {
            Ok(ToolOutput {
                content: "connection lost after dispatch".to_owned(),
                is_error: true,
                safe_to_resume: false,
                metadata: Value::Null,
            })
        }

        async fn checkpoint(&self, _state: Value, safe_to_resume: bool) -> Result<()> {
            self.checkpoints.lock().unwrap().push(safe_to_resume);
            Ok(())
        }
    }

    #[tokio::test]
    async fn stops_after_a_tool_without_a_safe_resume_point() {
        async fn completion(State(calls): State<Arc<Mutex<usize>>>) -> Json<Value> {
            *calls.lock().unwrap() += 1;
            Json(json!({
                "choices": [{
                    "message": {"content": null, "tool_calls": [{
                        "id": "unsafe-1", "type": "function",
                        "function": {"name": "Unsafe", "arguments": "{}"}
                    }]},
                    "finish_reason": "tool_calls"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1}
            }))
        }

        let calls = Arc::new(Mutex::new(0_usize));
        let app = Router::new()
            .route("/v1/chat/completions", post(completion))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let runtime = AgentRuntime::new(ModelConfig {
            base_url: format!("http://{address}/v1"),
            api_key: None,
            model: "test-model".to_owned(),
            timeout: Duration::from_secs(5),
            max_steps: 3,
            verify_tls_certificates: true,
        })
        .unwrap();
        let id = Uuid::new_v4();
        let run = RunSpec {
            schema_version: SCHEMA_VERSION,
            id,
            thread_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            project: FrozenReference { id, revision: 1 },
            project_privacy: ProjectPrivacy::PrivateLocal,
            task: None,
            creator_user_id: Uuid::new_v4(),
            executor_target: ExecutorTarget::ServerLinux { pool_id: None },
            required_capabilities: Vec::new(),
            input: json!({"prompt":"test"}),
            model_profile_id: None,
            snapshot_id: None,
            idempotency_key: "unsafe-test".to_owned(),
            created_at: Utc::now(),
        };
        let checkpoints = Arc::new(Mutex::new(Vec::new()));
        let host = UnsafeHost {
            checkpoints: checkpoints.clone(),
        };
        let error = runtime.execute(&run, &host).await.unwrap_err();
        assert!(error.to_string().contains("manual review"));
        assert_eq!(*calls.lock().unwrap(), 1);
        assert_eq!(*checkpoints.lock().unwrap(), vec![false]);
    }
}
