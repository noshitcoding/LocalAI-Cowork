use cowork_contracts::*;
use cowork_domain::*;
use cowork_model_accounting::{
    registry::{RawWireMode, RegisteredModel},
    tokenizer::TokenCount,
};
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;
#[path = "codex_gateway.rs"]
pub mod codex_gateway;

#[derive(Debug)]
pub struct Reply {
    pub provider_continuation: Option<Value>,
    pub received_cost_metadata: Option<Value>,
    pub content: String,
    pub tools: Vec<ToolCall>,
    pub usage: Usage,
    pub received: Option<cowork_storage::model_money::CompletedModelReceipt>,
}

// Preserve provider decimal lexemes before Value can round a JSON fraction.
// Only these bounded received facts are projected as provider-reported costs.
fn raw_cost_metadata(raw: &str, ollama: bool) -> serde_json::Map<String, Value> {
    let Ok(envelope) = serde_json::from_str::<BTreeMap<String, &serde_json::value::RawValue>>(raw)
    else {
        return Default::default();
    };
    let usage = if ollama {
        envelope
    } else {
        let Some(value) = envelope.get("usage") else {
            return Default::default();
        };
        let Ok(usage) =
            serde_json::from_str::<BTreeMap<String, &serde_json::value::RawValue>>(value.get())
        else {
            return Default::default();
        };
        usage
    };
    let mut result = serde_json::Map::new();
    for name in ["cost", "response_cost", "currency"] {
        let Some(raw) = usage.get(name) else { continue };
        if raw.get().len() > 256 {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<String>(raw.get()) {
            if value.len() <= 128 && !value.chars().any(char::is_control) {
                result.insert(name.into(), Value::String(value));
            }
        } else if name != "currency"
            && raw.get().len() <= 128
            && serde_json::from_str::<serde_json::Number>(raw.get()).is_ok()
        {
            result.insert(name.into(), Value::String(raw.get().into()));
        }
    }
    result
}

/// Actual receive evidence retained independently of a provider future. The
/// Runtime can journal uncertainty after EOF, timeout or cancellation, including
/// when the future is dropped. This grants no answer or second send authority.
#[derive(Clone, Default)]
pub struct ProviderReceiveObserver(Arc<Mutex<ObservedReceive>>);
#[derive(Default)]
struct ObservedReceive {
    scope: Option<cowork_storage::model_money::ModelDispatchScope>,
    bytes: Vec<u8>,
    usage: Vec<Value>,
    ollama: bool,
    evidence_only: bool,
    eof: bool,
    completed_stream: bool,
    usage_truncated: bool,
    completed_usage: Option<Value>,
}
impl ProviderReceiveObserver {
    pub fn new(scope: Option<cowork_storage::model_money::ModelDispatchScope>) -> Self {
        Self(Arc::new(Mutex::new(ObservedReceive {
            scope,
            ..Default::default()
        })))
    }
    /// Record actual receive facts for a caller with its own durable intent.
    /// This observer has no monetary dispatch scope or send authority.
    pub fn evidence_only() -> Self {
        Self(Arc::new(Mutex::new(ObservedReceive {
            evidence_only: true,
            ..Default::default()
        })))
    }
    fn observe_chunk(&self, bytes: &[u8], ollama: bool) {
        let mut observed = self.0.lock().expect("receive observer poisoned");
        if observed.scope.is_some() || observed.evidence_only {
            observed.bytes.extend_from_slice(bytes);
            observed.ollama = ollama;
        }
    }
    fn observe_usage(&self, value: &Value, ollama: bool) {
        let object = if ollama {
            Some(value)
        } else {
            value.get("usage")
        };
        if let Some(object) = object.filter(|v| !v.is_null()) {
            let mut observed = self.0.lock().expect("receive observer poisoned");
            if observed.scope.is_some() || observed.evidence_only {
                if observed.scope.is_some() || observed.usage.len() < 256 {
                    observed.usage.push(object.clone());
                } else {
                    observed.usage_truncated = true;
                }
            }
        }
    }
    pub fn received_evidence(&self) -> Result<Option<Value>> {
        let observed = self.0.lock().expect("receive observer poisoned");
        if observed.bytes.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            json!({"format":1,"raw_response_sha256":hash(&observed.bytes),"raw_scope":if observed.completed_stream{"completed_wire_stream"}else{"observed_prefix"},"received_bytes":observed.bytes.len(),"eof":observed.eof,"completed_stream":observed.completed_stream,"usage_objects":observed.usage,"usage_truncated":observed.usage_truncated,"completed_usage":observed.completed_usage,"provider_kind":if observed.ollama{"ollama"}else{"openai_compatible"},"evidence_only":observed.scope.is_none()}),
        ))
    }
    pub fn interrupted_receipt(
        &self,
    ) -> Result<Option<cowork_storage::model_money::InterruptedModelReceipt>> {
        let observed = self.0.lock().expect("receive observer poisoned");
        let Some(scope) = observed
            .scope
            .clone()
            .filter(|_| !observed.bytes.is_empty())
        else {
            return Ok(None);
        };
        cowork_storage::model_money::InterruptedModelReceipt::from_interrupted_stream(
            scope,
            &hash(&observed.bytes),
            observed.usage.clone(),
            observed.ollama,
        )
        .map(Some)
        .map_err(|error| Error::Storage(Box::new(error)))
    }
}

/// A provider cannot turn replayed tool-call IDs into a second action.
pub fn validate_tool_ids(messages: &[Value], calls: &[ToolCall]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for message in messages {
        if let Some(prior) = message.get("tool_calls").and_then(Value::as_array) {
            for call in prior {
                if let Some(id) = call["id"].as_str() {
                    seen.insert(id.to_owned());
                }
            }
        }
    }
    for call in calls {
        if call.id.is_empty() || !seen.insert(call.id.clone()) {
            return Err(Error::Provider("duplicate_tool_call_id".into()));
        }
    }
    Ok(())
}

pub fn validate_profile(profile: &ProviderProfile, allow_test: bool) -> Result<()> {
    cowork_storage::library_config::validate_generation_settings(&profile.generation)?;
    valid_text(&profile.name, 100)?;
    valid_text(&profile.model, 200)?;
    if profile.context_tokens < 1024 || profile.context_tokens > 2_000_000 {
        return Err(Error::Validation(
            "Context limit must be between 1024 and 2000000 tokens".into(),
        ));
    }
    if matches!(profile.kind, ProviderKind::Test) {
        return if allow_test {
            Ok(())
        } else {
            Err(Error::Forbidden)
        };
    }
    if matches!(profile.kind, ProviderKind::Codex) {
        return if cowork_contracts::parse_codex_profile_endpoint(&profile.endpoint).is_some() {
            Ok(())
        } else {
            Err(Error::Validation(
                "Invalid frozen Codex account binding".into(),
            ))
        };
    }
    validate_endpoint(&profile.endpoint)
}

pub fn validate_endpoint(endpoint: &str) -> Result<()> {
    if endpoint.len() > 8192 || endpoint.chars().any(char::is_control) {
        return Err(Error::Validation("Invalid provider URL".into()));
    }
    let url = reqwest::Url::parse(endpoint)
        .map_err(|_| Error::Validation("Invalid provider URL".into()))?;
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if url.host_str().is_none()
        || !(url.scheme() == "https" || (url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Validation(
            "Use HTTPS or loopback HTTP, without embedded credentials, query or fragment".into(),
        ));
    }
    Ok(())
}

pub fn tool_catalog(spec: &RunSpec) -> Value {
    cowork_domain::model_tool_catalog(spec)
}

pub async fn probe(
    profile: &ProviderProfile,
    credential: Option<String>,
    allow_test: bool,
) -> Result<()> {
    validate_profile(profile, allow_test)?;
    if matches!(profile.kind, ProviderKind::Codex) {
        return Err(Error::Provider("codex_account_probe_required".into()));
    }
    if matches!(profile.kind, ProviderKind::Test) {
        return Ok(());
    }
    if profile.credential_required && credential.is_none() {
        return Err(Error::Provider("credential_missing".into()));
    }
    let models = model_catalog(&profile.kind, &profile.endpoint, credential).await?;
    let ollama = matches!(profile.kind, ProviderKind::Ollama);
    if !models.iter().any(|candidate| {
        candidate == &profile.model || (ollama && candidate == &format!("{}:latest", profile.model))
    }) {
        return Err(Error::Provider("model_missing".into()));
    }
    Ok(())
}

/// Only bounded GET requests; no profile creation, model execution or redirects.
pub async fn model_catalog(
    kind: &ProviderKind,
    endpoint: &str,
    credential: Option<String>,
) -> Result<Vec<String>> {
    validate_endpoint(endpoint)?;
    if !matches!(kind, ProviderKind::Ollama | ProviderKind::OpenaiCompatible) {
        return Err(Error::Validation(
            "Unsupported model catalogue provider".into(),
        ));
    }
    let ollama = matches!(kind, ProviderKind::Ollama);
    let suffix = if ollama { "/api/tags" } else { "/models" };
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| Error::Provider("HTTP initialization failed".into()))?;
    let mut request = client.get(format!("{}{suffix}", endpoint.trim_end_matches('/')));
    if let Some(key) = credential {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(connection_error)?;
    if !response.status().is_success() {
        return Err(Error::Provider(format!(
            "http_{}",
            response.status().as_u16()
        )));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| Error::Provider("model_catalog_interrupted".into()))?;
        if bytes.len() + chunk.len() > 2_000_000 {
            return Err(Error::Provider("model_catalog_too_large".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::Provider("invalid_model_catalog".into()))?;
    let models = if ollama {
        &value["models"]
    } else {
        &value["data"]
    };
    let array = models
        .as_array()
        .ok_or_else(|| Error::Provider("invalid_model_catalog".into()))?;
    let mut models = array
        .iter()
        .filter_map(|model| {
            if ollama {
                model["name"].as_str()
            } else {
                model["id"].as_str()
            }
        })
        .filter(|name| {
            !name.trim().is_empty() && name.len() <= 200 && !name.chars().any(char::is_control)
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Ok(models)
}

fn connection_error(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        return Error::Provider("connection_timeout".into());
    }
    // Inspect locally, but never expose transport messages (URLs or credentials).
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(current) = cause {
        let message = current.to_string().to_ascii_lowercase();
        if message.contains("certificate") || message.contains("tls") {
            return Error::Provider("tls_failed".into());
        }
        cause = current.source();
    }
    Error::Provider("connection_failed".into())
}

/// Versioned logical request digest and conservative context guard. This is not a tokenizer.
pub fn request_fingerprint(spec: &RunSpec, messages: &[Value]) -> Result<String> {
    model_request_fingerprint(spec, messages, &tool_catalog(spec))
}

/// A finalized request can only be produced by [`prepare`] and is consumed by
/// [`complete_prepared`]. Its payload contains conversation data, so it has no
/// `Debug`, serialization, or cloning implementation.
pub struct PreparedModelRequest {
    codex_transport: Option<codex_gateway::CodexTransport>,
    spec: RunSpec,
    spec_sha256: String,
    messages_sha256: String,
    logical_fingerprint: String,
    payload: Vec<u8>,
    endpoint: Option<reqwest::Url>,
    wire_sha256: String,
    test_has_tool: bool,
    allow_test: bool,
    exact_count: Option<TokenCount>,
    raw_prompt: bool,
    deployment_sha256: Option<String>,
    dispatch_id: Option<String>,
}

impl PreparedModelRequest {
    pub fn bind_codex_transport(&mut self, transport: codex_gateway::CodexTransport) -> Result<()> {
        transport.assert_profile(self.spec.provider.as_ref().ok_or(Error::Forbidden)?)?;
        if self.codex_transport.is_some() {
            return Err(Error::Conflict);
        }
        self.codex_transport = Some(transport);
        Ok(())
    }
    /// Caller supplies its actual already admitted run/attempt identity. This
    /// binds receiver deduplication; it grants no local or remote admission.
    pub fn bind_dispatch_id(&mut self, run: &str, attempt: &str) -> Result<()> {
        if run.is_empty() || attempt.is_empty() || run.len() > 128 || attempt.len() > 128 {
            return Err(Error::Forbidden);
        }
        let id = canonical_hash(
            &json!({"purpose":"cowork-model-dispatch-id-v1","run":run,"attempt":attempt}),
        );
        if self.dispatch_id.as_ref().is_some_and(|old| old != &id) {
            return Err(Error::Conflict);
        }
        self.dispatch_id = Some(id);
        Ok(())
    }
    pub fn context_measurement(&self) -> Value {
        match &self.exact_count {
            Some(count) => {
                json!({"mode":"exact_artifact_renderer","count":count,"binding":self.spec.model_binding})
            }
            None => json!({"mode":"conservative_bytes","exact":false}),
        }
    }
    pub fn exact_reserved_tokens(&self) -> Option<u64> {
        self.exact_count
            .as_ref()
            .and_then(|c| c.input_tokens.checked_add(c.output_allowance))
    }
    pub fn logical_fingerprint(&self) -> &str {
        &self.logical_fingerprint
    }

    /// The already serialized provider-specific JSON, including Ollama's
    /// structured tool arguments. No credential is accepted during preparation.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Format 1 wire digest binds the final endpoint and exact payload hash.
    /// This is separate from the existing logical fingerprint and byte guard.
    pub fn wire_sha256(&self) -> &str {
        &self.wire_sha256
    }

    pub fn assert_binding(&self, spec: &RunSpec, messages: &[Value]) -> Result<()> {
        if canonical_hash(spec) != self.spec_sha256
            || canonical_hash(&messages) != self.messages_sha256
            || request_fingerprint(spec, messages)? != self.logical_fingerprint
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

/// Finalize and validate the provider-specific request before model admission.
/// The legacy logical context guard remains conservative and is not a tokenizer.
pub fn prepare(
    spec: &RunSpec,
    messages: &[Value],
    allow_test: bool,
) -> Result<PreparedModelRequest> {
    prepare_registered(spec, messages, allow_test, None)
}

/// Some OpenAI-compatible chat templates accept exactly one system message,
/// at the beginning. Preserve system authority and instruction order on the
/// wire, without rewriting the durable conversation or its logical binding.
fn single_system_message(messages: &[Value]) -> Result<Vec<Value>> {
    let mut instructions = Vec::new();
    let mut conversation = Vec::new();
    for message in messages {
        if message["role"] == "system" {
            instructions.push(
                message["content"].as_str().ok_or_else(|| {
                    Error::Validation("System messages require text content".into())
                })?,
            );
        } else {
            conversation.push(message.clone());
        }
    }
    if !instructions.is_empty() {
        conversation.insert(
            0,
            json!({"role":"system","content":instructions.join("\n\n")}),
        );
    }
    Ok(conversation)
}

/// Verified image bytes by SHA-256 for one request. The logical binding keeps only the
/// `cowork_images` references of the durable conversation; bytes reach the wire payload only.
pub type RequestImages = std::collections::BTreeMap<String, (String, Vec<u8>)>;
const IMAGE_BATCHES: usize = 2;
const IMAGES_PER_BATCH: usize = 4;

fn image_message(
    refs: &[String],
    images: &RequestImages,
    kind: &ProviderKind,
    total: &mut usize,
) -> Value {
    use base64::Engine;
    let mut encoded = Vec::new();
    for sha in refs.iter().take(IMAGES_PER_BATCH) {
        if let Some((media_type, bytes)) = images.get(sha) {
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            *total += data.len();
            encoded.push((media_type.clone(), data));
        }
    }
    let missing = refs.len() - encoded.len();
    let mut text = "Images returned by the previous tool call(s), in order.".to_owned();
    if missing > 0 {
        text.push_str(&format!(" {missing} further image(s) are not shown."));
    }
    if matches!(kind, ProviderKind::Ollama) {
        json!({"role":"user","content":text,"images":encoded.into_iter().map(|(_, data)| data).collect::<Vec<_>>()})
    } else {
        let mut parts = vec![json!({"type":"text","text":text})];
        parts.extend(encoded.into_iter().map(|(media_type, data)| {
            json!({"type":"image_url","image_url":{"url":format!("data:{media_type};base64,{data}")}})
        }));
        json!({"role":"user","content":parts})
    }
}

/// Removes image references and, for vision models, adds one user message with the images
/// after each block of tool results. Only the latest batches carry bytes.
fn attach_images(
    messages: Vec<Value>,
    images: &RequestImages,
    vision: bool,
    kind: &ProviderKind,
) -> (Vec<Value>, usize) {
    let mut output = Vec::with_capacity(messages.len() + IMAGE_BATCHES);
    let mut batches: Vec<(usize, Vec<String>)> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for mut message in messages {
        let references: Vec<String> = message
            .as_object_mut()
            .and_then(|object| object.remove("cowork_images"))
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|reference| reference["sha256"].as_str().map(str::to_owned))
            .collect();
        let tool = message["role"] == "tool";
        if !tool && !pending.is_empty() {
            batches.push((output.len(), std::mem::take(&mut pending)));
        }
        output.push(message);
        if tool {
            pending.extend(references);
        }
    }
    if !pending.is_empty() {
        batches.push((output.len(), pending));
    }
    if !vision {
        return (output, 0);
    }
    let keep_from = batches.len().saturating_sub(IMAGE_BATCHES);
    let mut total = 0;
    for (index, (position, references)) in batches.into_iter().enumerate().rev() {
        let message = if index < keep_from {
            json!({"role":"user","content":format!("[{} earlier tool image(s) are no longer shown]", references.len())})
        } else {
            image_message(&references, images, kind, &mut total)
        };
        output.insert(position, message);
    }
    (output, total)
}

pub fn prepare_registered(
    spec: &RunSpec,
    messages: &[Value],
    allow_test: bool,
    registered: Option<&RegisteredModel>,
) -> Result<PreparedModelRequest> {
    prepare_registered_images(
        spec,
        messages,
        allow_test,
        registered,
        &RequestImages::new(),
    )
}

pub fn prepare_registered_images(
    spec: &RunSpec,
    messages: &[Value],
    allow_test: bool,
    registered: Option<&RegisteredModel>,
    images: &RequestImages,
) -> Result<PreparedModelRequest> {
    let profile = spec.provider.as_ref().ok_or(Error::Forbidden)?;
    if spec.file_action.is_some() {
        return Err(Error::Forbidden);
    }
    validate_profile(profile, allow_test)?;
    if matches!(profile.kind, ProviderKind::Codex) && spec.model_binding.is_some() {
        return Err(Error::Validation(
            "Codex profiles use the Responses runtime, not a raw model renderer".into(),
        ));
    }
    let catalog = tool_catalog(spec);
    let logical_fingerprint = model_request_fingerprint(spec, messages, &catalog)?;
    let tools = if cowork_domain::hybrid_planning(spec, messages) {
        json!([])
    } else {
        catalog
    };
    let ollama = matches!(profile.kind, ProviderKind::Ollama);
    let planning = cowork_domain::hybrid_planning(spec, messages);
    let mut model_messages = messages.to_vec();
    for message in &mut model_messages {
        if let Some(object) = message.as_object_mut() {
            object.remove("cowork_phase");
            if !matches!(profile.kind, ProviderKind::Codex) {
                object.remove("cowork_provider_continuation");
            }
        }
    }
    let vision = profile.capabilities.image_input && spec.model_binding.is_none();
    let (model_messages, image_bytes) =
        attach_images(model_messages, images, vision, &profile.kind);
    let mut model_messages = model_messages;
    if planning {
        model_messages.push(json!({"role":"system","content":"Planning phase. Produce a concrete plan, checks and intended actions. No tool execution is permitted in this phase. The execution phase follows automatically under the configured approvals."}));
    }
    let mut api_messages = model_messages.clone();
    if matches!(profile.kind, ProviderKind::OpenaiCompatible) && spec.model_binding.is_none() {
        api_messages = single_system_message(&api_messages)?;
    }
    if ollama {
        for message in &mut api_messages {
            if let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) {
                for call in calls {
                    if let Some(args) = call.pointer_mut("/function/arguments")
                        && let Some(s) = args.as_str()
                    {
                        *args = serde_json::from_str(s).map_err(storage_error)?;
                    }
                }
            }
        }
    }
    let mut body = if ollama {
        json!({"model":profile.model,"messages":api_messages,"stream":true,
            "tools":tools,"options":{"num_predict":spec.budgets.max_output_tokens}})
    } else {
        json!({"model":profile.model,"messages":api_messages,"stream":true,
            "stream_options":{"include_usage":true},"tools":tools,"max_tokens":spec.budgets.max_output_tokens})
    };
    if let Some(config) = &spec.agent_config {
        cowork_storage::library_config::validate_effective_agent_config(config)?;
    }
    let mut raw_prompt = false;
    let mut exact_count = None;
    let mut deployment_sha256 = None;
    if let Some(binding) = &spec.model_binding {
        let model =
            registered.ok_or_else(|| Error::Provider("qualified_model_binding_required".into()))?;
        if model.sha256().map_err(storage_error)? != binding.sha256
            || model.bundle.id != binding.id
            || model.bundle.version != binding.version
            || model.bundle.provider_sha256 != canonical_hash(profile)
        {
            return Err(Error::Forbidden);
        }
        let output = serde_json::to_value(if planning { None } else { spec.output.as_ref() })
            .map_err(storage_error)?;
        let prompt = model
            .prompt(&model_messages, &tools, &output, &logical_fingerprint)
            .map_err(storage_error)?;
        exact_count = Some(
            model
                .tokenizer
                .count(&prompt, u64::from(spec.budgets.max_output_tokens))
                .map_err(|e| match e {
                    cowork_model_accounting::Error::ContextLimit => Error::ContextLimit,
                    _ => Error::Validation(e.to_string()),
                })?,
        );
        body = match model.bundle.wire_mode {
            RawWireMode::OllamaGenerate => {
                json!({"model":profile.model,"prompt":prompt.text,"raw":true,"stream":true,"options":{"num_predict":spec.budgets.max_output_tokens,"num_ctx":profile.context_tokens}})
            }
            RawWireMode::OpenaiCompletions => {
                json!({"model":profile.model,"prompt":prompt.text,"stream":true,"stream_options":{"include_usage":true},"max_tokens":spec.budgets.max_output_tokens})
            }
        };
        raw_prompt = true;
        deployment_sha256 = Some(model.bundle.evidence.deployment_sha256.clone());
    }
    if let Some(temperature) = spec
        .agent_config
        .as_ref()
        .and_then(|c| c.generation.temperature)
        .or(profile.generation.temperature)
    {
        if ollama {
            body["options"]["temperature"] = json!(temperature);
        } else {
            body["temperature"] = json!(temperature);
        }
    }
    if let Some(output) = spec.output.as_ref().filter(|_| !planning) {
        cowork_domain::structured_output::validate_schema(output)?;
        if matches!(profile.kind, ProviderKind::Test) {
            return Err(Error::Validation(
                "structured_output_requires_real_provider".into(),
            ));
        }
        let schema = if raw_prompt && !tools.as_array().is_none_or(|t| t.is_empty()) {
            json!({"oneOf":[output.schema,{"type":"object","properties":{"content":{"type":"string"},"tools":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","properties":{"id":{"type":"string"},"name":{"type":"string"},"arguments":{"type":"object"}},"required":["id","name","arguments"],"additionalProperties":false}}},"required":["content","tools"],"additionalProperties":false}]})
        } else {
            output.schema.clone()
        };
        if ollama {
            body["format"] = schema;
        } else if !raw_prompt {
            body["response_format"] = cowork_domain::structured_output::openai_format(output)?;
        }
    }
    if matches!(profile.kind, ProviderKind::Codex) {
        body = codex_gateway::responses_body(spec, &api_messages, &tools, planning)?;
    }
    let payload = serde_json::to_vec(&body).map_err(storage_error)?;
    if (planning || matches!(profile.kind, ProviderKind::Codex))
        && registered.is_none()
        && payload
            .len()
            .saturating_sub(image_bytes)
            .saturating_add(model_messages.len().saturating_mul(128))
            .saturating_add(spec.budgets.max_output_tokens as usize)
            > profile.context_tokens as usize
    {
        return Err(Error::ContextLimit);
    }
    let endpoint = if matches!(profile.kind, ProviderKind::Test) {
        None
    } else if matches!(profile.kind, ProviderKind::Codex) {
        Some(reqwest::Url::parse(codex_gateway::UPSTREAM).map_err(|_| Error::Forbidden)?)
    } else {
        let suffix = if raw_prompt && ollama {
            "/api/generate"
        } else if raw_prompt {
            "/completions"
        } else if ollama {
            "/api/chat"
        } else {
            "/chat/completions"
        };
        Some(
            reqwest::Url::parse(&format!(
                "{}{suffix}",
                profile.endpoint.trim_end_matches('/')
            ))
            .map_err(|_| Error::Validation("Invalid provider URL".into()))?,
        )
    };
    let wire_sha256 = canonical_hash(&json!({
        "format":1,
        "provider_kind":profile.kind,
        "endpoint":endpoint.as_ref().map(reqwest::Url::as_str).unwrap_or("test://complete"),
        "payload_sha256":hash(&payload),
    }));
    Ok(PreparedModelRequest {
        codex_transport: None,
        spec: spec.clone(),
        spec_sha256: canonical_hash(spec),
        messages_sha256: canonical_hash(&messages),
        logical_fingerprint,
        payload,
        endpoint,
        wire_sha256,
        test_has_tool: cowork_domain::hybrid_planning(spec, messages)
            || spec.agent_config.as_ref().and_then(|c| c.execution_mode)
                == Some(AgentExecutionMode::Plan)
            || messages.iter().any(|message| message["role"] == "tool"),
        allow_test,
        exact_count,
        raw_prompt,
        deployment_sha256,
        dispatch_id: None,
    })
}

/// Read the actual configured deployment with only the caller's already scoped
/// credential. This helper never looks up an installation-owner secret.
pub async fn assert_qualified_deployment(
    spec: &RunSpec,
    model: &RegisteredModel,
    credential: Option<&str>,
) -> Result<()> {
    let profile = spec.provider.as_ref().ok_or(Error::Forbidden)?;
    if canonical_hash(profile) != model.bundle.provider_sha256 {
        return Err(Error::Forbidden);
    }
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|_| Error::Provider("model_deployment_unverified".into()))?;
    let url = format!(
        "{}/.well-known/cowork-model-qualification",
        profile.endpoint.trim_end_matches('/')
    );
    let mut request = client.get(url).header(
        "x-cowork-deployment-sha256",
        &model.bundle.evidence.deployment_sha256,
    );
    if let Some(secret) = credential {
        request = request.bearer_auth(secret);
    }
    let response = request
        .send()
        .await
        .map_err(|_| Error::Provider("model_deployment_unverified".into()))?;
    if !response.status().is_success() || response.content_length().is_some_and(|n| n > 64 * 1024) {
        return Err(Error::Provider("model_deployment_unverified".into()));
    }
    let mut chunks = response.bytes_stream();
    let mut bytes = vec![];
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| Error::Provider("model_deployment_unverified".into()))?;
        if bytes.len() + chunk.len() > 64 * 1024 {
            return Err(Error::Forbidden);
        }
        bytes.extend_from_slice(&chunk);
    }
    let actual: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::Provider("model_deployment_unverified".into()))?;
    let identity = &model.bundle.qualification.model;
    if actual["format"] != 1
        || actual["deployment_sha256"] != model.bundle.evidence.deployment_sha256
        || actual["provider_sha256"] != model.bundle.provider_sha256
        || actual["model_sha256"] != identity.model_sha256
        || actual["tokenizer_sha256"] != identity.tokenizer_sha256
        || actual["chat_template_sha256"] != identity.chat_template_sha256
    {
        return Err(Error::Provider("model_deployment_changed".into()));
    }
    Ok(())
}

pub async fn complete(
    spec: &RunSpec,
    messages: &[Value],
    credential: Option<String>,
    deltas: mpsc::Sender<String>,
    allow_test: bool,
) -> Result<Reply> {
    complete_prepared(prepare(spec, messages, allow_test)?, credential, deltas).await
}

/// Send this exact prepared payload once. No request messages or tools are
/// rendered here, and credentials only become an HTTP authorization header.
pub async fn complete_prepared(
    prepared: PreparedModelRequest,
    credential: Option<String>,
    deltas: mpsc::Sender<String>,
) -> Result<Reply> {
    complete_prepared_with_dispatch(prepared, credential, deltas, None).await
}

/// Trusted Runtime path only, bound to its already committed actual attempt.
/// The legacy completion helpers never mint an accounted provider receipt.
pub async fn complete_prepared_with_dispatch(
    prepared: PreparedModelRequest,
    credential: Option<String>,
    deltas: mpsc::Sender<String>,
    dispatch: Option<cowork_storage::model_money::ModelDispatchScope>,
) -> Result<Reply> {
    complete_prepared_observed(
        prepared,
        credential,
        deltas,
        ProviderReceiveObserver::new(dispatch),
    )
    .await
}

pub async fn complete_prepared_observed(
    prepared: PreparedModelRequest,
    credential: Option<String>,
    deltas: mpsc::Sender<String>,
    observer: ProviderReceiveObserver,
) -> Result<Reply> {
    let dispatch = observer
        .0
        .lock()
        .expect("receive observer poisoned")
        .scope
        .clone();
    if let Some(scope) = &dispatch
        && (scope.logical_sha256() != prepared.logical_fingerprint()
            || scope.wire_sha256() != prepared.wire_sha256()
            || scope.model_spec_sha256() != prepared.spec_sha256
            || scope.provider_profile_sha256()
                != canonical_hash(prepared.spec.provider.as_ref().ok_or(Error::Forbidden)?))
    {
        return Err(Error::Forbidden);
    }
    let PreparedModelRequest {
        codex_transport,
        spec,
        payload,
        endpoint,
        test_has_tool,
        allow_test,
        raw_prompt,
        deployment_sha256,
        dispatch_id,
        ..
    } = prepared;
    let profile = spec.provider.as_ref().ok_or(Error::Forbidden)?;
    if matches!(profile.kind, ProviderKind::Codex) {
        if credential.is_some() {
            return Err(Error::Forbidden);
        }
        let transport =
            codex_transport.ok_or_else(|| Error::Provider("codex_transport_required".into()))?;
        transport.assert_profile(profile)?;
        return codex_gateway::complete(transport, &spec, payload, deltas, observer).await;
    }
    if matches!(profile.kind, ProviderKind::Test) {
        if !allow_test {
            return Err(Error::Forbidden);
        }
        let delay = if profile.model == "slow-test" {
            5000
        } else {
            150
        };
        tokio::time::sleep(Duration::from_millis(delay)).await;
        if !test_has_tool
            && spec
                .allowed_tools
                .iter()
                .any(|name| name == "create_artifact")
        {
            let tools = vec![ToolCall {
                id: format!("test-{}", uuid::Uuid::new_v4()),
                name: "create_artifact".into(),
                arguments: json!({"name":"test-result.md","content":"# Deterministic test artifact\n\nThis is test-provider output, not an actual model response."}),
            }];
            return Ok(Reply {
                provider_continuation: None,
                received_cost_metadata: None,
                content: "The deterministic test provider requests an artifact.".into(),
                tools,
                usage: Usage::default(),
                received: None,
            });
        }
        let content =
            "Deterministic test completed. This run did not call a real model.".to_string();
        let _ = deltas.send(content.clone()).await;
        return Ok(Reply {
            provider_continuation: None,
            received_cost_metadata: None,
            content,
            tools: vec![],
            usage: Usage::default(),
            received: None,
        });
    }
    let ollama = matches!(profile.kind, ProviderKind::Ollama);
    if profile.credential_required && credential.is_none() {
        return Err(Error::Provider("credential_missing".into()));
    }
    let endpoint = endpoint.ok_or(Error::Forbidden)?;
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_millis(
            spec.agent_config
                .as_ref()
                .and_then(|c| c.generation.timeout_ms)
                .or(profile.generation.timeout_ms)
                .unwrap_or(spec.budgets.max_seconds.saturating_mul(1000))
                .min(spec.budgets.max_seconds.saturating_mul(1000)),
        ))
        .build()
        .map_err(|_| Error::Provider("HTTP initialization failed".into()))?;
    let mut request = client
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload);
    if let Some(deployment) = &deployment_sha256 {
        let dispatch = dispatch_id
            .as_ref()
            .ok_or_else(|| Error::Provider("qualified_dispatch_binding_required".into()))?;
        request = request.header("x-cowork-model-attempt", dispatch);
        request = request.header("x-cowork-deployment-sha256", deployment);
    }
    if let Some(key) = credential {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(connection_error)?;
    if !response.status().is_success() {
        return Err(Error::Provider(format!(
            "http_{}",
            response.status().as_u16()
        )));
    }
    if let Some(expected) = &deployment_sha256
        && response
            .headers()
            .get("x-cowork-deployment-sha256")
            .and_then(|v| v.to_str().ok())
            != Some(expected.as_str())
    {
        return Err(Error::Provider("model_deployment_echo_unverified".into()));
    }
    let mut cost_headers = serde_json::Map::new();
    for name in [
        "x-litellm-response-cost",
        "x-litellm-response-cost-currency",
    ] {
        if let Some(value) = response
            .headers()
            .get(name)
            .and_then(|h| h.to_str().ok())
            .filter(|s| s.len() <= 128 && !s.chars().any(char::is_control))
        {
            cost_headers.insert(name.into(), json!(value));
        }
    }
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut received_bytes = Vec::new();
    let mut completed_usage = Value::Null;
    let mut completed_cost_metadata = serde_json::Map::new();
    let mut total = 0_usize;
    let mut content = String::new();
    let mut tools: BTreeMap<u64, (String, String, String)> = BTreeMap::new();
    let mut usage = Usage::default();
    let mut finished = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| Error::Provider("stream_interrupted".into()))?;
        total += chunk.len();
        if total > 4_000_000 {
            return Err(Error::Provider("response_limit_exceeded".into()));
        }
        received_bytes.extend_from_slice(&chunk);
        observer.observe_chunk(&chunk, ollama);
        buffer.extend_from_slice(&chunk);
        while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
            let bytes: Vec<_> = buffer.drain(..=end).collect();
            let line = std::str::from_utf8(&bytes)
                .map_err(|_| Error::Provider("invalid_utf8".into()))?
                .trim();
            let raw = if ollama {
                line
            } else {
                line.strip_prefix("data:").unwrap_or("").trim()
            };
            if raw.is_empty() {
                continue;
            }
            if raw == "[DONE]" {
                finished = true;
                continue;
            }
            if finished {
                return Err(Error::Provider("data_after_completed_stream".into()));
            }
            let value: Value = serde_json::from_str(raw)
                .map_err(|_| Error::Provider("invalid_stream_json".into()))?;
            observer.observe_usage(&value, ollama);
            if value.get("error").is_some() {
                return Err(Error::Provider("provider_reported_error".into()));
            }
            let delta = if ollama {
                &value["message"]
            } else {
                &value["choices"][0]["delta"]
            };
            let text = if raw_prompt && ollama {
                value["response"].as_str()
            } else if raw_prompt {
                value["choices"][0]["text"].as_str()
            } else {
                delta["content"].as_str()
            };
            // Providers send empty content for role, thinking, tool and final-usage frames.
            // Their metadata still matters below; only actual text belongs in the journal.
            if let Some(text) = text.filter(|text| !text.is_empty()) {
                content.push_str(text);
                if deltas.send(text.into()).await.is_err() {
                    return Err(Error::Provider("consumer_closed".into()));
                }
            }
            if let Some(calls) = delta["tool_calls"].as_array() {
                for (i, call) in calls.iter().enumerate() {
                    let index = call["index"].as_u64().unwrap_or(i as u64);
                    let entry = tools
                        .entry(index)
                        .or_insert_with(|| (String::new(), String::new(), String::new()));
                    if let Some(id) = call["id"].as_str() {
                        entry.0.push_str(id);
                    }
                    if let Some(name) = call["function"]["name"].as_str() {
                        entry.1.push_str(name);
                    }
                    if let Some(args) = call["function"]["arguments"].as_str() {
                        entry.2.push_str(args);
                    } else if ollama {
                        entry.2 = call["function"]["arguments"].to_string();
                    }
                }
            }
            if ollama {
                if value["done"] == true {
                    finished = true;
                    completed_usage = value.clone();
                    completed_cost_metadata = raw_cost_metadata(raw, true);
                }
                if let Some(n) = value["prompt_eval_count"].as_u64() {
                    usage.input_tokens = Some(n);
                }
                if let Some(n) = value["eval_count"].as_u64() {
                    usage.output_tokens = Some(n);
                }
            } else if let Some(u) = value.get("usage") {
                if dispatch.is_some() && !u.is_null() && !completed_usage.is_null() {
                    return Err(Error::Provider("ambiguous_completed_usage".into()));
                }
                if !u.is_null() {
                    completed_usage = u.clone();
                    completed_cost_metadata = raw_cost_metadata(raw, false);
                }
                if let Some(n) = u["prompt_tokens"].as_u64() {
                    usage.input_tokens = Some(n);
                }
                if let Some(n) = u["completion_tokens"].as_u64() {
                    usage.output_tokens = Some(n);
                }
            }
        }
    }
    observer.0.lock().expect("receive observer poisoned").eof = true;
    // A truncated stream must never be treated as a successful model decision.
    if !finished || !buffer.iter().all(u8::is_ascii_whitespace) {
        return Err(Error::Provider("stream_incomplete".into()));
    }
    {
        let mut observed = observer.0.lock().expect("receive observer poisoned");
        observed.completed_stream = true;
        observed.completed_usage = Some(completed_usage.clone());
    }
    let mut tools = tools
        .into_values()
        .map(|(id, name, args)| {
            if name.is_empty() {
                return Err(Error::Provider("missing_tool_name".into()));
            }
            let arguments = serde_json::from_str(&args)
                .map_err(|_| Error::Provider("invalid_tool_arguments".into()))?;
            Ok(ToolCall {
                id: if id.is_empty() {
                    uuid::Uuid::new_v4().to_string()
                } else {
                    id
                },
                name,
                arguments,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    // A valid final schema value wins over the tool protocol. This keeps a
    // user schema with content/tools fields from becoming execution authority.
    let valid_final = spec.output.as_ref().is_some_and(|output| {
        cowork_domain::structured_output::validate_result(output, &content).is_ok()
    });
    if raw_prompt
        && !valid_final
        && let Ok(envelope) = serde_json::from_str::<Value>(&content)
        && envelope
            .as_object()
            .is_some_and(|v| v.len() == 2 && v.contains_key("content") && v.contains_key("tools"))
        && envelope["tools"].as_array().is_some_and(|v| !v.is_empty())
    {
        if !tools.is_empty() {
            return Err(Error::Provider("ambiguous_raw_tool_response".into()));
        }
        content = envelope["content"]
            .as_str()
            .ok_or_else(|| Error::Provider("invalid_raw_tool_response".into()))?
            .into();
        tools = serde_json::from_value(envelope["tools"].clone())
            .map_err(|_| Error::Provider("invalid_raw_tool_response".into()))?;
        if tools.iter().any(|t| {
            !spec.allowed_tools.contains(&t.name) || t.id.is_empty() || !t.arguments.is_object()
        }) {
            return Err(Error::Forbidden);
        }
    }
    if tools.len() > 16 {
        return Err(Error::Provider("too_many_tool_calls".into()));
    }
    if content.is_empty() && tools.is_empty() {
        return Err(Error::Provider("empty_response".into()));
    }
    Ok(Reply {
        provider_continuation:None,
        received_cost_metadata: (!completed_cost_metadata.is_empty()||!cost_headers.is_empty()).then(||json!({"format":1,"body_usage":completed_cost_metadata,"headers":cost_headers,"provider_reported":true,"completed_stream":true})),
        content,
        tools,
        usage,
        received: dispatch
            .map(|scope| {
                cowork_storage::model_money::CompletedModelReceipt::from_completed_stream(
                    scope,
                    &hash(&received_bytes),
                    completed_usage,
                    ollama,
                )
            })
            .transpose()
            .map_err(|error| Error::Storage(Box::new(error)))?,
    })
}

#[cfg(test)]
mod image_tests {
    use super::*;

    fn tool(id: &str, sha: Option<&str>) -> Value {
        let mut message = json!({"role":"tool","tool_call_id":id,"content":"{}"});
        if let Some(sha) = sha {
            message["cowork_images"] = json!([{"sha256":sha,"media_type":"image/png"}]);
        }
        message
    }

    fn images() -> RequestImages {
        ["a", "b", "c"]
            .into_iter()
            .map(|s| {
                (
                    s.to_owned(),
                    ("image/png".to_owned(), s.as_bytes().to_vec()),
                )
            })
            .collect()
    }

    #[test]
    fn references_are_stripped_without_vision() {
        let (messages, bytes) = attach_images(
            vec![json!({"role":"user","content":"x"}), tool("1", Some("a"))],
            &images(),
            false,
            &ProviderKind::OpenaiCompatible,
        );
        assert_eq!(bytes, 0);
        assert_eq!(messages.len(), 2);
        assert!(messages[1].get("cowork_images").is_none());
    }

    #[test]
    fn only_the_latest_batches_carry_images() {
        let conversation = vec![
            json!({"role":"user","content":"x"}),
            tool("1", Some("a")),
            json!({"role":"assistant","content":"","tool_calls":[]}),
            tool("2", Some("b")),
            tool("3", None),
            json!({"role":"assistant","content":"","tool_calls":[]}),
            tool("4", Some("c")),
        ];
        let (messages, bytes) = attach_images(
            conversation,
            &images(),
            true,
            &ProviderKind::OpenaiCompatible,
        );
        assert!(bytes > 0);
        // user, tool1, [placeholder], assistant, tool2, tool3, [images b], assistant, tool4, [images c]
        assert_eq!(messages.len(), 10);
        assert!(
            messages[2]["content"]
                .as_str()
                .unwrap()
                .contains("no longer shown")
        );
        assert_eq!(
            messages[6]["content"][1]["image_url"]["url"],
            "data:image/png;base64,Yg=="
        );
        assert_eq!(
            messages[9]["content"][1]["image_url"]["url"],
            "data:image/png;base64,Yw=="
        );
        assert!(messages.iter().all(|m| m.get("cowork_images").is_none()));
    }

    #[test]
    fn ollama_uses_images_and_missing_previews_are_reported() {
        let (messages, _) = attach_images(
            vec![tool("1", Some("a")), tool("2", Some("missing"))],
            &images(),
            true,
            &ProviderKind::Ollama,
        );
        assert_eq!(messages[2]["images"], json!(["YQ=="]));
        assert!(
            messages[2]["content"]
                .as_str()
                .unwrap()
                .contains("1 further image")
        );
    }
}
