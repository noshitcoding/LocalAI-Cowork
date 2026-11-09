//! Shared logical provider request digest. This conservative byte guard is not a tokenizer.
use crate::{Error, Result, canonical_hash};
use cowork_contracts::RunSpec;
use serde_json::{Value, json};
pub fn model_request_fingerprint(
    spec: &RunSpec,
    messages: &[Value],
    tools: &Value,
) -> Result<String> {
    request_fingerprint(spec, messages, tools, true)
}
/// Native preparations do not contact a model or reserve its context window.
/// The same logical frame retains compatible digests; the absolute byte bound
/// still limits checkpoint data. This digest is never a model dispatch permit.
pub fn workflow_preparation_fingerprint(
    spec: &RunSpec,
    messages: &[Value],
    tools: &Value,
) -> Result<String> {
    request_fingerprint(spec, messages, tools, false)
}
fn request_fingerprint(
    spec: &RunSpec,
    messages: &[Value],
    tools: &Value,
    model_context: bool,
) -> Result<String> {
    let profile = spec.provider.as_ref().ok_or(Error::Forbidden)?;
    let mut request = json!({"format":1,"provider":profile,"messages":messages,"tools":tools,"max_output_tokens":spec.budgets.max_output_tokens});
    if let Some(config) = &spec.agent_config {
        if config
            .generation
            .temperature
            .is_some_and(|n| !n.is_finite() || !(0.0..=2.0).contains(&n))
            || config
                .generation
                .timeout_ms
                .is_some_and(|n| !(1000..=1_200_000).contains(&n))
            || config.instructions.len() > 256_000
        {
            return Err(Error::Validation("Invalid agent configuration".into()));
        }
        if config.generation.temperature.is_some() || config.generation.timeout_ms.is_some() {
            request["generation"] =
                serde_json::to_value(&config.generation).map_err(crate::storage_error)?;
        }
    }
    if let Some(output) = &spec.output {
        crate::structured_output::validate_schema(output)?;
        request["output"] = serde_json::to_value(output).map_err(crate::storage_error)?;
    }
    if let Some(binding) = &spec.model_binding {
        if binding.id.is_empty()
            || binding.id.len() > 128
            || binding.version == 0
            || binding.sha256.len() != 64
            || !binding
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Forbidden);
        }
        request["model_binding"] = serde_json::to_value(binding).map_err(crate::storage_error)?;
    }
    let bytes = request
        .to_string()
        .len()
        .saturating_add(messages.len().saturating_mul(128));
    if !model_context && bytes > 2 * 1024 * 1024 {
        return Err(Error::ContextLimit);
    }
    if model_context
        && spec.model_binding.is_none()
        && bytes.saturating_add(spec.budgets.max_output_tokens as usize)
            > profile.context_tokens as usize
    {
        return Err(Error::ContextLimit);
    }
    Ok(canonical_hash(&request))
}
