use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use thiserror::Error;
use url::Url;

const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434";
const DEFAULT_MODEL: &str = "llama3.1:8b";
const DEFAULT_TIMEOUT_MS: u64 = 600_000;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaConfig {
    pub base_url: String,
    pub model: String,
    pub timeout_ms: u64,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: std::env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| DEFAULT_OLLAMA_BASE_URL.to_string()),
            model: std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
            timeout_ms: std::env::var("OLLAMA_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(DEFAULT_TIMEOUT_MS),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagResponse {
    models: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelEntry {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionResponse {
    version: String,
}

#[derive(Debug, Deserialize)]
struct GenerateResponse {
    response: String,
}

#[derive(Debug, Deserialize)]
struct GenerateStreamResponse {
    response: Option<String>,
    done: Option<bool>,
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatToolFunctionDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatToolDef {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ChatToolFunctionDef,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OllamaToolFunctionCall {
    pub name: String,
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OllamaToolCall {
    pub function: OllamaToolFunctionCall,
}

#[derive(Debug, Serialize)]
struct ChatApiMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatApiResponse {
    message: Option<ChatApiAssistantMessage>,
}

#[derive(Debug, Deserialize)]
struct ChatApiAssistantMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OllamaToolCall>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaHealthResponse {
    pub ok: bool,
    pub endpoint: String,
    pub model: String,
    pub latency_ms: u128,
    pub version: Option<String>,
    pub models: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanResponse {
    pub endpoint: String,
    pub model: String,
    pub raw_response: String,
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTurnResponse {
    pub endpoint: String,
    pub model: String,
    pub assistant_message: String,
    pub requires_approval: bool,
    pub proposed_plan: Vec<String>,
    pub tool_calls: Vec<OllamaToolCall>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStreamChunkPayload {
    pub stream_id: String,
    pub chunk: String,
}

#[derive(Debug, Error)]
pub enum OllamaError {
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("request failed: {0}")]
    RequestFailed(String),
    #[error("response parsing failed: {0}")]
    ParseFailed(String),
}

fn request_error(context: &str, config: &OllamaConfig, error: reqwest::Error) -> OllamaError {
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_decode() {
        "decode"
    } else if error.is_request() {
        "request"
    } else {
        "network"
    };
    OllamaError::RequestFailed(format!(
        "{context} failed ({kind}) for endpoint={} model={} timeoutMs={}: {}",
        config.base_url, config.model, config.timeout_ms, error
    ))
}

fn empty_response_error(context: &str, config: &OllamaConfig) -> OllamaError {
    OllamaError::RequestFailed(format!(
        "{context} returned an empty response for endpoint={} model={}. Try another model tag or increase the timeout.",
        config.base_url, config.model
    ))
}

fn normalize_config(config: Option<OllamaConfig>) -> Result<OllamaConfig, OllamaError> {
    let merged = config.unwrap_or_default();

    let endpoint = if merged.base_url.trim().is_empty() {
        DEFAULT_OLLAMA_BASE_URL.to_string()
    } else {
        merged.base_url.trim().to_string()
    };

    let parsed = Url::parse(&endpoint)
        .map_err(|error| OllamaError::InvalidEndpoint(format!("{endpoint} ({error})")))?;

    let model = if merged.model.trim().is_empty() {
        DEFAULT_MODEL.to_string()
    } else {
        merged.model.trim().to_string()
    };

    Ok(OllamaConfig {
        base_url: parsed.to_string().trim_end_matches('/').to_string(),
        model,
        timeout_ms: merged.timeout_ms.max(1_000),
    })
}

fn build_http_client(timeout_ms: u64) -> Result<Client, OllamaError> {
    Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .map_err(|error| OllamaError::RequestFailed(error.to_string()))
}

pub async fn check_health(
    config: Option<OllamaConfig>,
) -> Result<OllamaHealthResponse, OllamaError> {
    let config = normalize_config(config)?;
    let client = build_http_client(config.timeout_ms)?;

    let tags_url = format!("{}/api/tags", config.base_url);
    let version_url = format!("{}/api/version", config.base_url);

    let started = Instant::now();

    let tags_response = client
        .get(&tags_url)
        .send()
        .await
        .map_err(|error| request_error("/api/tags", &config, error))?;

    if !tags_response.status().is_success() {
        return Ok(OllamaHealthResponse {
            ok: false,
            endpoint: config.base_url,
            model: config.model,
            latency_ms: started.elapsed().as_millis(),
            version: None,
            models: vec![],
            error: Some(format!(
                "Ollama returned status {} for /api/tags",
                tags_response.status()
            )),
        });
    }

    let tag_payload: TagResponse = tags_response
        .json()
        .await
        .map_err(|error| OllamaError::ParseFailed(error.to_string()))?;

    let version_payload = match client.get(&version_url).send().await {
        Ok(response) if response.status().is_success() => response
            .json::<VersionResponse>()
            .await
            .ok()
            .map(|value| value.version),
        _ => None,
    };

    Ok(OllamaHealthResponse {
        ok: true,
        endpoint: config.base_url,
        model: config.model,
        latency_ms: started.elapsed().as_millis(),
        version: version_payload,
        models: tag_payload
            .models
            .into_iter()
            .map(|item| item.name)
            .collect(),
        error: None,
    })
}

pub async fn generate_plan(
    config: Option<OllamaConfig>,
    prompt: String,
) -> Result<PlanResponse, OllamaError> {
    let config = normalize_config(config)?;
    let client = build_http_client(config.timeout_ms)?;

    let generate_url = format!("{}/api/generate", config.base_url);

    let payload = serde_json::json!({
        "model": config.model,
        "prompt": format!(
            "Erzeuge eine kurze, umsetzbare Schrittfolge auf English. Gib nur nummerierte Schritte aus.\n\nTask:\n{}",
            prompt
        ),
        "stream": false,
        "options": {
            "temperature": 0.2
        }
    });

    let started = Instant::now();
    log::info!(
        "ollama generate_plan start endpoint={} model={} timeoutMs={}",
        config.base_url,
        config.model,
        config.timeout_ms
    );

    let response = client
        .post(&generate_url)
        .json(&payload)
        .send()
        .await
        .map_err(|error| request_error("/api/generate", &config, error))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(OllamaError::RequestFailed(format!(
            "Ollama returned status {} for /api/generate{}",
            status,
            if body.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", body.trim())
            }
        )));
    }

    let generate_payload: GenerateResponse = response
        .json()
        .await
        .map_err(|error| OllamaError::ParseFailed(error.to_string()))?;

    if generate_payload.response.trim().is_empty() {
        log::warn!(
            "ollama generate_plan empty endpoint={} model={} elapsedMs={}",
            config.base_url,
            config.model,
            started.elapsed().as_millis()
        );
        return Err(empty_response_error("/api/generate", &config));
    }

    log::info!(
        "ollama generate_plan success endpoint={} model={} elapsedMs={} chars={}",
        config.base_url,
        config.model,
        started.elapsed().as_millis(),
        generate_payload.response.len()
    );

    let steps = parse_steps(&generate_payload.response);

    Ok(PlanResponse {
        endpoint: config.base_url,
        model: config.model,
        raw_response: generate_payload.response,
        steps,
    })
}

pub async fn chat_turn(
    config: Option<OllamaConfig>,
    prompt: String,
    history: Vec<ChatMessage>,
    tools: Vec<ChatToolDef>,
) -> Result<ChatTurnResponse, OllamaError> {
    let config = normalize_config(config)?;

    if !tools.is_empty() {
        return chat_turn_with_tools(config, prompt, history, tools).await;
    }

    let client = build_http_client(config.timeout_ms)?;

    let generate_url = format!("{}/api/generate", config.base_url);
    let chat_prompt = build_chat_prompt(&prompt, &history);

    let payload = serde_json::json!({
        "model": config.model,
        "prompt": chat_prompt,
        "stream": false,
        "options": {
            "temperature": 0.25
        }
    });

    let started = Instant::now();
    log::info!(
        "ollama chat_turn start endpoint={} model={} timeoutMs={} historyItems={} promptChars={}",
        config.base_url,
        config.model,
        config.timeout_ms,
        history.len(),
        prompt.len()
    );

    let response = client
        .post(&generate_url)
        .json(&payload)
        .send()
        .await
        .map_err(|error| request_error("/api/generate", &config, error))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(OllamaError::RequestFailed(format!(
            "Ollama returned status {} for /api/generate{}",
            status,
            if body.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", body.trim())
            }
        )));
    }

    let generate_payload: GenerateResponse = response
        .json()
        .await
        .map_err(|error| OllamaError::ParseFailed(error.to_string()))?;

    if generate_payload.response.trim().is_empty() {
        log::warn!(
            "ollama chat_turn empty endpoint={} model={} elapsedMs={}",
            config.base_url,
            config.model,
            started.elapsed().as_millis()
        );
        return Err(empty_response_error("/api/generate", &config));
    }

    log::info!(
        "ollama chat_turn success endpoint={} model={} elapsedMs={} chars={}",
        config.base_url,
        config.model,
        started.elapsed().as_millis(),
        generate_payload.response.len()
    );

    Ok(build_chat_turn_response(
        config,
        prompt,
        generate_payload.response,
        vec![],
    ))
}

pub async fn chat_turn_stream<F>(
    stream_id: String,
    config: Option<OllamaConfig>,
    prompt: String,
    history: Vec<ChatMessage>,
    tools: Vec<ChatToolDef>,
    mut on_chunk: F,
    is_cancelled: impl Fn() -> bool,
) -> Result<ChatTurnResponse, OllamaError>
where
    F: FnMut(ChatStreamChunkPayload) -> Result<(), OllamaError>,
{
    if !tools.is_empty() {
        return chat_turn(config, prompt, history, tools).await;
    }

    let config = normalize_config(config)?;
    let client = build_http_client(config.timeout_ms)?;
    let generate_url = format!("{}/api/generate", config.base_url);
    let chat_prompt = build_chat_prompt(&prompt, &history);

    let payload = serde_json::json!({
        "model": config.model,
        "prompt": chat_prompt,
        "stream": true,
        "options": {
            "temperature": 0.25
        }
    });

    let started = Instant::now();
    log::info!(
        "ollama chat_turn_stream start endpoint={} model={} timeoutMs={} historyItems={} promptChars={}",
        config.base_url,
        config.model,
        config.timeout_ms,
        history.len(),
        prompt.len()
    );

    let response = client
        .post(&generate_url)
        .json(&payload)
        .send()
        .await
        .map_err(|error| request_error("/api/generate", &config, error))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(OllamaError::RequestFailed(format!(
            "Ollama returned status {} for /api/generate{}",
            status,
            if body.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", body.trim())
            }
        )));
    }

    let mut stream = response.bytes_stream();
    let mut line_buffer = String::new();
    let mut assistant_message = String::new();

    while let Some(next) = stream.next().await {
        if is_cancelled() {
            return Err(OllamaError::RequestFailed(
                "Chat-Generierung abgebrochen.".to_string(),
            ));
        }

        let bytes = match next {
            Ok(bytes) => bytes,
            Err(error) => {
                let stream_error = request_error("/api/generate stream", &config, error);
                log::warn!(
                    "ollama chat_turn_stream fallback endpoint={} model={} reason={}",
                    config.base_url,
                    config.model,
                    stream_error
                );
                return chat_turn(
                    Some(config.clone()),
                    prompt.clone(),
                    history.clone(),
                    vec![],
                )
                .await;
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        line_buffer.push_str(&text);

        while let Some(newline_idx) = line_buffer.find('\n') {
            let line = line_buffer[..newline_idx].trim().to_string();
            line_buffer = line_buffer[newline_idx + 1..].to_string();
            handle_stream_line(&stream_id, &line, &mut assistant_message, &mut on_chunk)?;
        }
    }

    let trailing = line_buffer.trim().to_string();
    if !trailing.is_empty() {
        handle_stream_line(&stream_id, &trailing, &mut assistant_message, &mut on_chunk)?;
    }

    if assistant_message.trim().is_empty() {
        log::warn!(
            "ollama chat_turn_stream empty endpoint={} model={} elapsedMs={}",
            config.base_url,
            config.model,
            started.elapsed().as_millis()
        );
        return chat_turn(Some(config.clone()), prompt, history, vec![]).await;
    }

    log::info!(
        "ollama chat_turn_stream success endpoint={} model={} elapsedMs={} chars={}",
        config.base_url,
        config.model,
        started.elapsed().as_millis(),
        assistant_message.len()
    );

    Ok(build_chat_turn_response(
        config,
        prompt,
        assistant_message,
        vec![],
    ))
}

async fn chat_turn_with_tools(
    config: OllamaConfig,
    prompt: String,
    history: Vec<ChatMessage>,
    tools: Vec<ChatToolDef>,
) -> Result<ChatTurnResponse, OllamaError> {
    let client = build_http_client(config.timeout_ms)?;
    let chat_url = format!("{}/api/chat", config.base_url);
    let messages = build_chat_messages(&prompt, &history);

    let payload = serde_json::json!({
        "model": config.model,
        "messages": messages,
        "stream": false,
        "tools": tools,
        "options": {
            "temperature": 0.25
        }
    });

    let started = Instant::now();
    log::info!(
        "ollama chat_turn tools start endpoint={} model={} timeoutMs={} historyItems={} toolCount={} promptChars={}",
        config.base_url,
        config.model,
        config.timeout_ms,
        history.len(),
        payload
            .get("tools")
            .and_then(|value| value.as_array())
            .map(|value| value.len())
            .unwrap_or(0),
        prompt.len()
    );

    let response = client
        .post(&chat_url)
        .json(&payload)
        .send()
        .await
        .map_err(|error| request_error("/api/chat", &config, error))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(OllamaError::RequestFailed(format!(
            "Ollama returned status {} for /api/chat{}",
            status,
            if body.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", body.trim())
            }
        )));
    }

    let chat_payload: ChatApiResponse = response
        .json()
        .await
        .map_err(|error| OllamaError::ParseFailed(error.to_string()))?;

    let assistant_message = chat_payload
        .message
        .as_ref()
        .and_then(|message| message.content.clone())
        .unwrap_or_default();
    let tool_calls = chat_payload
        .message
        .and_then(|message| message.tool_calls)
        .unwrap_or_default();

    if assistant_message.trim().is_empty() && tool_calls.is_empty() {
        log::warn!(
            "ollama chat_turn tools empty endpoint={} model={} elapsedMs={}",
            config.base_url,
            config.model,
            started.elapsed().as_millis()
        );
        return Err(empty_response_error("/api/chat", &config));
    }

    log::info!(
        "ollama chat_turn tools success endpoint={} model={} elapsedMs={} chars={} toolCalls={}",
        config.base_url,
        config.model,
        started.elapsed().as_millis(),
        assistant_message.len(),
        tool_calls.len()
    );

    Ok(build_chat_turn_response(
        config,
        prompt,
        assistant_message,
        tool_calls,
    ))
}

fn handle_stream_line<F>(
    stream_id: &str,
    line: &str,
    assistant_message: &mut String,
    on_chunk: &mut F,
) -> Result<(), OllamaError>
where
    F: FnMut(ChatStreamChunkPayload) -> Result<(), OllamaError>,
{
    let normalized = line.strip_prefix("data:").unwrap_or(line).trim();

    if normalized.is_empty() {
        return Ok(());
    }

    let payload: GenerateStreamResponse = match serde_json::from_str(normalized) {
        Ok(payload) => payload,
        Err(error) => {
            log::debug!(
                "ignoring non-json ollama stream line: {} ({})",
                normalized,
                error
            );
            return Ok(());
        }
    };

    if let Some(error) = payload.error {
        return Err(OllamaError::RequestFailed(error));
    }

    if let Some(chunk) = payload.response {
        if !chunk.is_empty() {
            assistant_message.push_str(&chunk);
            on_chunk(ChatStreamChunkPayload {
                stream_id: stream_id.to_string(),
                chunk,
            })?;
        }
    }

    let _done = payload.done.unwrap_or(false);
    Ok(())
}

fn build_chat_prompt(prompt: &str, history: &[ChatMessage]) -> String {
    let mut history_text = String::new();
    for msg in history.iter().rev().take(8).rev() {
        history_text.push_str(&format!("{}: {}\n", msg.role, msg.content));
    }

    format!(
        "You are Open_Cowork, a local assistant agent. Answer concisely, clearly, and in English.\n\
    If the task could contain risky or destructive actions, suggest a plan and mark it as requiring approval.\n\
    WICHTIGE REGELN:\n\
    - Never output placeholder or waiting answers such as 'I am analyzing', 'please wait', 'coming soon', or similar text.\n\
    - Always provide a final substantive answer directly in this message.\n\
    - Erfinde niemals Dokumentinhalte.\n\
    - If only a file path is available but no extracted document text is in the prompt, clearly state that the content is not available and ask for a documented text excerpt or enabled file analysis.\n\n\
    Context history:\n{}\nUser: {}\n\nAnswer:",
        history_text, prompt
    )
}

fn build_chat_messages(prompt: &str, history: &[ChatMessage]) -> Vec<ChatApiMessage> {
    let mut messages = history
        .iter()
        .rev()
        .take(8)
        .rev()
        .map(|msg| ChatApiMessage {
            role: msg.role.clone(),
            content: msg.content.clone(),
        })
        .collect::<Vec<_>>();

    messages.push(ChatApiMessage {
        role: "user".to_string(),
        content: prompt.to_string(),
    });

    messages
}

fn detect_risky_action(text: &str) -> bool {
    let normalized = text.to_lowercase();
    let dangerous_phrases = [
        "rm -rf",
        "format c:",
        "format d:",
        "drop table",
        "drop database",
        "truncate table",
        "delete from",
        "shutdown /s",
        "shutdown -h",
        "shutdown now",
        "kill -9",
        "taskkill /f",
        "remove-item -recurse",
        "del /s /q",
        "rmdir /s",
        "registry delete",
        "reg delete",
        "netsh advfirewall",
        "iptables -f",
        "chmod 777",
        "mkfs.",
        "dd if=",
    ];
    dangerous_phrases
        .iter()
        .any(|phrase| normalized.contains(phrase))
}

fn build_chat_turn_response(
    config: OllamaConfig,
    prompt: String,
    assistant_message: String,
    tool_calls: Vec<OllamaToolCall>,
) -> ChatTurnResponse {
    let requires_approval = detect_risky_action(&prompt) || detect_risky_action(&assistant_message);

    let proposed_plan = if requires_approval {
        let response_steps = parse_steps(&assistant_message);
        let steps: Vec<String> = response_steps.into_iter().take(6).collect();
        if steps.is_empty() || (steps.len() == 1 && steps[0] == assistant_message.trim()) {
            vec![
                assistant_message
                    .lines()
                    .next()
                    .unwrap_or("Aktion pruefen")
                    .trim()
                    .to_string(),
            ]
        } else {
            steps
        }
    } else {
        vec![]
    };

    ChatTurnResponse {
        endpoint: config.base_url,
        model: config.model,
        assistant_message,
        requires_approval,
        proposed_plan,
        tool_calls,
    }
}

fn parse_steps(raw: &str) -> Vec<String> {
    let mut parsed = vec![];

    for line in raw.lines() {
        let cleaned = line
            .trim()
            .trim_start_matches(|ch: char| {
                ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == ')'
            })
            .trim()
            .to_string();

        if !cleaned.is_empty() {
            parsed.push(cleaned);
        }
    }

    if parsed.is_empty() {
        vec![raw.trim().to_string()]
    } else {
        parsed
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OllamaConfig, OllamaToolCall, OllamaToolFunctionCall, build_chat_turn_response, parse_steps,
    };
    use serde_json::{Map, Value};

    #[test]
    fn parse_steps_extracts_numbered_lines() {
        let raw = "1. Initialize project\n2) Configure Ollama\n- Run tests";
        let parsed = parse_steps(raw);

        assert_eq!(
            parsed,
            vec![
                "Initialize project".to_string(),
                "Configure Ollama".to_string(),
            "Run tests".to_string()
            ]
        );
    }

    #[test]
    fn parse_steps_falls_back_to_raw_text() {
        let raw = "Free text without line breaks";
        let parsed = parse_steps(raw);

        assert_eq!(parsed, vec!["Free text without line breaks".to_string()]);
    }

    #[test]
    fn build_chat_turn_response_preserves_tool_calls() {
        let mut arguments = Map::new();
        arguments.insert(
            "file_path".to_string(),
            Value::String("C:\\workspace\\README.md".to_string()),
        );

        let response = build_chat_turn_response(
            OllamaConfig {
                base_url: "http://localhost:11434".to_string(),
                model: "qwen3.6:35b".to_string(),
                timeout_ms: 1_000,
            },
            "Lies README.md".to_string(),
            String::new(),
            vec![OllamaToolCall {
                function: OllamaToolFunctionCall {
                    name: "Read".to_string(),
                    arguments,
                },
            }],
        );

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].function.name, "Read");
        assert_eq!(response.assistant_message, "");
        assert!(!response.requires_approval);
    }
}
