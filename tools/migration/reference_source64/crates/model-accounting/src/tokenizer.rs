//! Exact local tokenizer artifacts. Unknown bindings fail closed.
use crate::{Error, Result, digest, nonempty, sha256, valid_hash};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tokenizers::Tokenizer;

pub const ENGINE: &str = "huggingface-tokenizers-0.23.1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelIdentity {
    pub provider_id: String,
    pub model_id: String,
    /// Immutable weights/deployment identity, never an alias such as "latest".
    pub model_sha256: String,
    pub tokenizer_sha256: String,
    pub vocabulary_sha256: String,
    pub chat_template_sha256: String,
    pub context_tokens: u64,
}

impl ModelIdentity {
    pub fn validate(&self) -> Result<()> {
        if !nonempty(&self.provider_id)
            || !nonempty(&self.model_id)
            || self.context_tokens == 0
            || [
                &self.model_sha256,
                &self.tokenizer_sha256,
                &self.vocabulary_sha256,
                &self.chat_template_sha256,
            ]
            .iter()
            .any(|h| !valid_hash(h))
        {
            return Err(Error::UnqualifiedTokenizer);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceEncoding {
    pub rendered_text: String,
    pub token_ids: Vec<u32>,
}

/// Must come from a separately trusted qualification register, not a model reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenizerQualification {
    pub model: ModelIdentity,
    pub engine: String,
    pub reference_implementation: String,
    pub parity_evidence_sha256: String,
    pub reference_encodings: Vec<ReferenceEncoding>,
}

/// Authority supplied by the trusted register after external parity review.
/// Merely constructing a tokenizer or matching its own vectors never grants it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustedTokenizerBinding {
    pub model: ModelIdentity,
    pub qualification_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderedPrompt {
    pub model: ModelIdentity,
    pub logical_request_sha256: String,
    /// The complete exact provider framing including tools and generation prompt.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenCount {
    pub model: ModelIdentity,
    pub qualification_sha256: String,
    pub logical_request_sha256: String,
    pub rendered_sha256: String,
    pub input_tokens: u64,
    pub output_allowance: u64,
}

pub struct QualifiedTokenizer {
    model: ModelIdentity,
    qualification_sha256: String,
    tokenizer: Tokenizer,
}

impl QualifiedTokenizer {
    pub fn load(
        artifact: &[u8],
        chat_template: &[u8],
        qualification: &TokenizerQualification,
        trusted: &TrustedTokenizerBinding,
    ) -> Result<Self> {
        qualification.model.validate()?;
        if qualification.model != trusted.model
            || qualification.engine != ENGINE
            || !nonempty(&qualification.reference_implementation)
            || !valid_hash(&qualification.parity_evidence_sha256)
            || !valid_hash(&trusted.qualification_sha256)
            || digest(qualification)? != trusted.qualification_sha256
            || sha256(artifact) != trusted.model.tokenizer_sha256
            || sha256(chat_template) != trusted.model.chat_template_sha256
            || qualification.reference_encodings.is_empty()
        {
            return Err(Error::UnqualifiedTokenizer);
        }
        // A serialized truncation/padding policy can make encode under-report
        // the actual prompt or introduce framing the provider never receives.
        let config: serde_json::Value =
            serde_json::from_slice(artifact).map_err(|_| Error::UnqualifiedTokenizer)?;
        if !config["truncation"].is_null() || !config["padding"].is_null() {
            return Err(Error::UnqualifiedTokenizer);
        }
        let tokenizer = Tokenizer::from_bytes(artifact).map_err(|_| Error::UnqualifiedTokenizer)?;
        let vocabulary: BTreeMap<_, _> = tokenizer.get_vocab(true).into_iter().collect();
        if digest(&vocabulary)? != trusted.model.vocabulary_sha256 {
            return Err(Error::UnqualifiedTokenizer);
        }
        for reference in &qualification.reference_encodings {
            // The chat template supplies special tokens. Adding them again is
            // deliberately forbidden by this complete-rendered-prompt contract.
            let encoding = tokenizer
                .encode(reference.rendered_text.as_str(), false)
                .map_err(|_| Error::UnqualifiedTokenizer)?;
            if encoding.get_ids() != reference.token_ids {
                return Err(Error::UnqualifiedTokenizer);
            }
        }
        Ok(Self {
            model: trusted.model.clone(),
            qualification_sha256: trusted.qualification_sha256.clone(),
            tokenizer,
        })
    }

    pub fn model(&self) -> &ModelIdentity {
        &self.model
    }

    /// Exact full-template IDs, with special tokens already supplied by the
    /// qualified template. Used by the owned gateway's literal-token prefill.
    pub fn encode_complete(&self, text: &str) -> Result<Vec<u32>> {
        if text.len() > 64 * 1024 * 1024 {
            return Err(Error::ContextLimit);
        }
        Ok(self
            .tokenizer
            .encode(text, false)
            .map_err(|_| Error::UnqualifiedTokenizer)?
            .get_ids()
            .to_vec())
    }

    pub fn count(&self, prompt: &RenderedPrompt, output_allowance: u64) -> Result<TokenCount> {
        let result = self.measure(prompt, output_allowance)?;
        if result
            .input_tokens
            .checked_add(output_allowance)
            .ok_or(Error::Overflow)?
            > self.model.context_tokens
        {
            return Err(Error::ContextLimit);
        }
        Ok(result)
    }

    /// Measurement remains available for an explicit compaction preview even
    /// when admission would reject the full prompt. This grants no send permit.
    pub fn measure(&self, prompt: &RenderedPrompt, output_allowance: u64) -> Result<TokenCount> {
        if prompt.model != self.model || !valid_hash(&prompt.logical_request_sha256) {
            return Err(Error::UnqualifiedTokenizer);
        }
        let encoding = self
            .tokenizer
            .encode(prompt.text.as_str(), false)
            .map_err(|_| Error::UnqualifiedTokenizer)?;
        let input_tokens = u64::try_from(encoding.get_ids().len()).map_err(|_| Error::Overflow)?;
        Ok(TokenCount {
            model: self.model.clone(),
            qualification_sha256: self.qualification_sha256.clone(),
            logical_request_sha256: prompt.logical_request_sha256.clone(),
            rendered_sha256: sha256(prompt.text.as_bytes()),
            input_tokens,
            output_allowance,
        })
    }
}
