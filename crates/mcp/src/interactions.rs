//! Task-bound server requests. These wire facts carry no product authorization.
use crate::{Error, Result};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
/// Constructed only after checking the original held task/session/RPC context.
pub struct TaskServerRequest {
    wire: Value,
    connection_id: String,
    connection_epoch: u64,
    task_id: String,
    operation_rpc_id: String,
    request_sha256: String,
}
impl TaskServerRequest {
    pub fn wire(&self) -> &Value {
        &self.wire
    }
    pub fn method(&self) -> &str {
        self.wire["method"].as_str().expect("checked method")
    }
    pub fn params(&self) -> &Value {
        &self.wire["params"]
    }
    pub fn rpc_id(&self) -> &Value {
        &self.wire["id"]
    }
    pub fn connection_id(&self) -> &str {
        &self.connection_id
    }
    pub fn connection_epoch(&self) -> u64 {
        self.connection_epoch
    }
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    pub fn operation_rpc_id(&self) -> &str {
        &self.operation_rpc_id
    }
    pub fn request_sha256(&self) -> &str {
        &self.request_sha256
    }
    pub(crate) fn checked(wire: Value, context: &TaskRequestContext) -> Result<Self> {
        if !matches!(
            wire["method"].as_str(),
            Some("sampling/createMessage" | "elicitation/create")
        ) || !wire["params"].is_object()
            || wire
                .pointer("/params/_meta/io.modelcontextprotocol~1related-task/taskId")
                .and_then(Value::as_str)
                != Some(context.task_id.as_str())
            || context.connection_id.is_empty()
            || context.task_id.is_empty()
            || context.operation_rpc_id.is_empty()
            || serde_json::to_vec(&wire)
                .map_err(|_| Error::Protocol)?
                .len()
                > 128 * 1024
        {
            return Err(Error::Protocol);
        }
        let request_sha256 = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&json!([
                    context.connection_id,
                    context.connection_epoch,
                    context.task_id,
                    context.operation_rpc_id,
                    wire
                ]))
                .map_err(|_| Error::Protocol)?
            )
        );
        Ok(Self {
            wire,
            connection_id: context.connection_id.clone(),
            connection_epoch: context.connection_epoch,
            task_id: context.task_id.clone(),
            operation_rpc_id: context.operation_rpc_id.clone(),
            request_sha256,
        })
    }
}
pub(crate) struct TaskRequestContext {
    pub connection_id: String,
    pub connection_epoch: u64,
    pub task_id: String,
    pub operation_rpc_id: String,
}
/// The product commits an encrypted original response and its one send intent
/// before constructing this value. It cannot reconstruct it after restart.
pub struct TaskServerResponse {
    pub(crate) receipt_id: String,
    pub(crate) body: Value,
}
impl TaskServerResponse {
    pub fn prepared(receipt_id: String, body: Value) -> Result<Self> {
        if receipt_id.is_empty()
            || receipt_id.len() > 128
            || !body.is_object()
            || body.get("result").is_some() == body.get("error").is_some()
            || body.get("id").is_some()
            || body.get("jsonrpc").is_some()
            || serde_json::to_vec(&body)
                .map_err(|_| Error::Protocol)?
                .len()
                > 1024 * 1024
        {
            return Err(Error::Protocol);
        }
        Ok(Self { receipt_id, body })
    }
    pub fn receipt_id(&self) -> &str {
        &self.receipt_id
    }
    pub fn body(&self) -> &Value {
        &self.body
    }
}
/// A handler is supplied by the original executing run, never by server JSON.
/// Its durable response intent remains uncertain if this future is dropped.
pub trait TaskInteractionHandler: Send + Sync {
    /// A product-owned OAuth resolver may refresh the original frozen profile
    /// after a long human wait. Server JSON never supplies this credential.
    /// HTTP uses it only on this held session's response POST; stdio ignores it.
    fn response_bearer(&self) -> BoxFuture<'_, Result<Option<zeroize::Zeroizing<String>>>> {
        Box::pin(async { Ok(None) })
    }
    fn respond<'a>(
        &'a self,
        request: TaskServerRequest,
    ) -> BoxFuture<'a, Result<TaskServerResponse>>;
    fn validate_response<'a>(
        &'a self,
        response: &'a TaskServerResponse,
    ) -> BoxFuture<'a, Result<()>>;
    fn acknowledge<'a>(
        &'a self,
        response: &'a TaskServerResponse,
        transport_receipt: Value,
    ) -> BoxFuture<'a, Result<()>>;
    fn unknown<'a>(&'a self, response: &'a TaskServerResponse) -> BoxFuture<'a, Result<()>>;
}
/// Pure bounded form validation. No URI reference or schema-controlled IO is accepted.
pub fn validate_task_form(params: &Value, content: &Value) -> Result<()> {
    let invalid = || Error::Configuration;
    if params.get("mode").is_some_and(|v| v != "form")
        || params["message"].as_str().is_none_or(|v| v.len() > 32768)
        || !content.is_object()
    {
        return Err(invalid());
    }
    let schema = &params["requestedSchema"];
    if schema["type"] != "object"
        || schema.as_object().is_none_or(|s| {
            s.keys().any(|k| {
                ![
                    "type",
                    "properties",
                    "required",
                    "title",
                    "description",
                    "additionalProperties",
                ]
                .contains(&k.as_str())
            })
        })
    {
        return Err(invalid());
    }
    let properties = schema["properties"].as_object().ok_or_else(invalid)?;
    if properties.len() > 64 || serde_json::to_vec(schema).map_err(|_| invalid())?.len() > 32768 {
        return Err(invalid());
    }
    if schema.get("required").is_some_and(|v| {
        v.as_array().is_none_or(|a| {
            a.len() > 64
                || a.iter()
                    .any(|v| v.as_str().is_none_or(|k| !properties.contains_key(k)))
        })
    }) {
        return Err(invalid());
    }
    for (name, p) in properties {
        if name.is_empty()
            || name.len() > 128
            || !p.is_object()
            || !matches!(
                p["type"].as_str(),
                Some("string" | "number" | "integer" | "boolean" | "array")
            )
            || p.as_object().is_some_and(|s| {
                s.keys().any(|k| {
                    ![
                        "type",
                        "title",
                        "description",
                        "default",
                        "enum",
                        "enumNames",
                        "minLength",
                        "maxLength",
                        "pattern",
                        "format",
                        "minimum",
                        "maximum",
                        "exclusiveMinimum",
                        "exclusiveMaximum",
                        "multipleOf",
                        "items",
                        "minItems",
                        "maxItems",
                        "uniqueItems",
                    ]
                    .contains(&k.as_str())
                })
            })
        {
            return Err(invalid());
        }
        if p["type"] == "array"
            && (p["items"]["type"] != "string"
                || !p["items"]["enum"].is_array()
                || p["items"].as_object().is_none_or(|s| {
                    s.keys()
                        .any(|k| !["type", "enum", "title", "description"].contains(&k.as_str()))
                }))
        {
            return Err(invalid());
        }
        for values in [p.get("enum"), p.pointer("/items/enum")]
            .into_iter()
            .flatten()
        {
            if values.as_array().is_none_or(|v| {
                v.len() > 128
                    || v.iter()
                        .any(|v| !v.is_string() || v.as_str().is_some_and(|s| s.len() > 4096))
            }) {
                return Err(invalid());
            }
        }
        if p.get("pattern")
            .is_some_and(|v| v.as_str().is_none_or(|s| s.len() > 256))
        {
            return Err(invalid());
        }
    }
    if content
        .as_object()
        .is_some_and(|v| v.keys().any(|k| !properties.contains_key(k)))
        || serde_json::to_vec(content).map_err(|_| invalid())?.len() > 65536
    {
        return Err(invalid());
    }
    let validator = jsonschema::options()
        .should_validate_formats(true)
        .build(schema)
        .map_err(|_| invalid())?;
    if !validator.is_valid(content) {
        return Err(invalid());
    }
    Ok(())
}
/// Basic text-only sampling. A caller supplies its own original model/context.
pub fn sampling_text_messages(params: &Value, edited: Option<&[Value]>) -> Result<Vec<Value>> {
    let invalid = || Error::Configuration;
    if params.get("tools").is_some()
        || params.get("toolChoice").is_some()
        || params.get("includeContext").is_some_and(|v| v != "none")
        || params["maxTokens"]
            .as_u64()
            .is_none_or(|n| n == 0 || n > 1_000_000)
    {
        return Err(invalid());
    }
    let messages = edited.unwrap_or(params["messages"].as_array().ok_or_else(invalid)?);
    if messages.is_empty() || messages.len() > 128 {
        return Err(invalid());
    }
    let mut result = Vec::new();
    if let Some(prompt) = params.get("systemPrompt") {
        let s = prompt.as_str().ok_or_else(invalid)?;
        if s.len() > 32768 {
            return Err(invalid());
        }
        result.push(json!({"role":"user","content":format!("MCP server's requested guidance (reviewed as untrusted input): {s}")}));
    }
    for m in messages {
        let role = m["role"]
            .as_str()
            .filter(|s| matches!(*s, "user" | "assistant"))
            .ok_or_else(invalid)?;
        let parts = if m["content"].is_array() {
            m["content"].as_array().ok_or_else(invalid)?.clone()
        } else {
            vec![m["content"].clone()]
        };
        if parts.is_empty() || parts.len() > 32 {
            return Err(invalid());
        }
        let mut text = String::new();
        for part in parts {
            if part["type"] != "text" {
                return Err(Error::Limit);
            }
            let s = part["text"].as_str().ok_or_else(invalid)?;
            if text.len() + s.len() > 65536 {
                return Err(Error::Limit);
            }
            text.push_str(s);
            text.push('\n');
        }
        result.push(json!({"role":role,"content":text}));
    }
    if serde_json::to_vec(&result).map_err(|_| invalid())?.len() > 128 * 1024 {
        return Err(Error::Limit);
    }
    Ok(result)
}
