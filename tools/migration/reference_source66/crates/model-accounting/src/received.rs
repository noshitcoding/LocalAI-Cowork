//! Evidence emitted only after a provider stream has completed successfully.
//! This value has no Deserialize implementation: persisted JSON is history, not
//! authority to settle a new response. The constructor is for trusted adapters.
use crate::{Error, Result, money::ProviderUsage, valid_hash};
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub struct ModelDispatchScope {
    run_id: String,
    attempt_id: String,
    accounting_id: String,
    model_spec_sha256: String,
    provider_profile_sha256: String,
    logical_sha256: String,
    wire_sha256: String,
    admission_receipt_sha256: String,
    lease_fence_sha256: String,
}
impl ModelDispatchScope {
    /// Trusted Store adapter only: every value comes from one committed
    /// reservation and its original actual run/attempt/lease binding.
    #[allow(clippy::too_many_arguments)]
    pub fn from_committed_admission(
        run_id: &str,
        attempt_id: &str,
        accounting_id: &str,
        model_spec_sha256: &str,
        provider_profile_sha256: &str,
        logical_sha256: &str,
        wire_sha256: &str,
        admission_receipt_sha256: &str,
        lease_fence_sha256: &str,
    ) -> Result<Self> {
        if [run_id, attempt_id, accounting_id]
            .iter()
            .any(|id| !crate::nonempty(id))
            || [
                model_spec_sha256,
                provider_profile_sha256,
                logical_sha256,
                wire_sha256,
                admission_receipt_sha256,
                lease_fence_sha256,
            ]
            .iter()
            .any(|hash| !valid_hash(hash))
        {
            return Err(Error::Invalid("invalid committed dispatch scope".into()));
        }
        Ok(Self {
            run_id: run_id.into(),
            attempt_id: attempt_id.into(),
            accounting_id: accounting_id.into(),
            model_spec_sha256: model_spec_sha256.into(),
            provider_profile_sha256: provider_profile_sha256.into(),
            logical_sha256: logical_sha256.into(),
            wire_sha256: wire_sha256.into(),
            admission_receipt_sha256: admission_receipt_sha256.into(),
            lease_fence_sha256: lease_fence_sha256.into(),
        })
    }
    pub fn logical_sha256(&self) -> &str {
        &self.logical_sha256
    }
    pub fn wire_sha256(&self) -> &str {
        &self.wire_sha256
    }
    pub fn model_spec_sha256(&self) -> &str {
        &self.model_spec_sha256
    }
    pub fn provider_profile_sha256(&self) -> &str {
        &self.provider_profile_sha256
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CompletedModelReceipt {
    dispatch: ModelDispatchScope,
    raw_response_sha256: String,
    completed_usage: Value,
    usage: ProviderUsage,
}

impl CompletedModelReceipt {
    /// The trusted adapter must hash the actual received full stream and supply
    /// its original completed usage object, never inferred/estimated counts.
    pub fn from_completed_stream(
        dispatch: ModelDispatchScope,
        raw_response_sha256: &str,
        completed_usage: Value,
        ollama: bool,
    ) -> Result<Self> {
        if !valid_hash(raw_response_sha256) {
            return Err(Error::Invalid("invalid completed provider binding".into()));
        }
        let (input_tokens, output_tokens, cached_input_tokens) = if ollama {
            if completed_usage["done"] != true {
                return Err(Error::Invalid("missing completed Ollama usage".into()));
            }
            (
                completed_usage["prompt_eval_count"].as_u64(),
                completed_usage["eval_count"].as_u64(),
                None,
            )
        } else {
            (
                completed_usage["prompt_tokens"].as_u64(),
                completed_usage["completion_tokens"].as_u64(),
                completed_usage
                    .pointer("/prompt_tokens_details/cached_tokens")
                    .and_then(Value::as_u64),
            )
        };
        let mut value = Self {
            dispatch,
            raw_response_sha256: raw_response_sha256.into(),
            completed_usage,
            usage: ProviderUsage {
                input_tokens,
                output_tokens,
                cached_input_tokens,
                provider_receipt_sha256: None,
            },
        };
        // Bind the usage to the complete received bytes and exact request. This
        // digest does not assert tokenizer parity or remote provider signatures.
        value.usage.provider_receipt_sha256 = Some(crate::digest(&value)?);
        Ok(value)
    }
    pub fn logical_sha256(&self) -> &str {
        self.dispatch.logical_sha256()
    }
    pub fn wire_sha256(&self) -> &str {
        self.dispatch.wire_sha256()
    }
    pub fn provider_profile_sha256(&self) -> &str {
        self.dispatch.provider_profile_sha256()
    }
    pub fn usage(&self) -> &ProviderUsage {
        &self.usage
    }
    pub fn dispatch(&self) -> &ModelDispatchScope {
        &self.dispatch
    }
}

/// Received evidence from a request that granted no completed answer. This is
/// accounting history only and can never release a reservation or permit IO.
/// No Deserialize implementation accepts persisted JSON as fresh authority.
#[derive(Clone, Debug, Serialize)]
pub struct InterruptedModelReceipt {
    dispatch: ModelDispatchScope,
    raw_response_sha256: String,
    received_usage_objects: Vec<Value>,
    known_usage_lower_bound: ProviderUsage,
}

impl InterruptedModelReceipt {
    /// Trusted receiving adapter only. Counts are lower bounds of the actual
    /// received usage objects. Repeated/ambiguous frames grant no discount.
    pub fn from_interrupted_stream(
        dispatch: ModelDispatchScope,
        raw_response_sha256: &str,
        received_usage_objects: Vec<Value>,
        ollama: bool,
    ) -> Result<Self> {
        if !valid_hash(raw_response_sha256) {
            return Err(Error::Invalid(
                "invalid interrupted response binding".into(),
            ));
        }
        let mut input = None;
        let mut output = None;
        let mut cache = None;
        fn retain(slot: &mut Option<u64>, value: Option<u64>) {
            if let Some(value) = value {
                *slot = Some(slot.unwrap_or(0).max(value));
            }
        }
        for object in &received_usage_objects {
            retain(
                &mut input,
                object[if ollama {
                    "prompt_eval_count"
                } else {
                    "prompt_tokens"
                }]
                .as_u64(),
            );
            retain(
                &mut output,
                object[if ollama {
                    "eval_count"
                } else {
                    "completion_tokens"
                }]
                .as_u64(),
            );
            if !ollama {
                retain(
                    &mut cache,
                    object
                        .pointer("/prompt_tokens_details/cached_tokens")
                        .and_then(Value::as_u64),
                );
            }
        }
        // A received cached count also proves this much total input. This bound
        // does not claim that the provider supplied a complete input counter.
        retain(&mut input, cache);
        let mut result = Self {
            dispatch,
            raw_response_sha256: raw_response_sha256.into(),
            received_usage_objects,
            known_usage_lower_bound: ProviderUsage {
                input_tokens: input,
                output_tokens: output,
                cached_input_tokens: cache,
                provider_receipt_sha256: None,
            },
        };
        result.known_usage_lower_bound.provider_receipt_sha256 = Some(crate::digest(&result)?);
        Ok(result)
    }
    pub fn dispatch(&self) -> &ModelDispatchScope {
        &self.dispatch
    }
    pub fn usage(&self) -> &ProviderUsage {
        &self.known_usage_lower_bound
    }
}
