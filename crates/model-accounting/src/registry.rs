//! Operator-qualified full prompt rendering. No remote reply establishes trust.
use crate::tokenizer::{
    ModelIdentity, QualifiedTokenizer, RenderedPrompt, TokenizerQualification,
    TrustedTokenizerBinding,
};
use crate::{Error, Result, digest, nonempty, sha256, valid_hash};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const RENDERER: &str = "cowork-complete-json-chat-template-v1";
pub const TOOL_PROTOCOL: &str = "When requesting tools, return exactly a JSON object {\"content\":\"explanation\",\"tools\":[{\"id\":\"unique-id\",\"name\":\"allowed-tool-name\",\"arguments\":{}}]}. Return plain final answer text when no tool is requested. A tool request is never permission: the application checks scope and explicit approvals. For a structured final output, return the requested JSON value without the tools envelope.\n";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawWireMode {
    OllamaGenerate,
    OpenaiCompletions,
}

/// Deliberately finite, deterministic template. Every full logical message is
/// rendered as canonical JSON so no tool IDs, arguments or fields disappear.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteChatTemplate {
    pub format: u32,
    pub prefix: String,
    pub role_prefix: BTreeMap<String, String>,
    pub role_suffix: BTreeMap<String, String>,
    pub tools_prefix: String,
    pub tools_suffix: String,
    pub output_prefix: String,
    pub output_suffix: String,
    pub generation_prompt: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceRequest {
    pub messages: Vec<Value>,
    pub tools: Value,
    pub output: Value,
    pub rendered_text: String,
    pub token_ids: Vec<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentEvidence {
    pub issuer: String,
    pub reference_implementation: String,
    pub deployment_sha256: String,
    pub reviewed_at: i64,
    pub reference_requests: Vec<ReferenceRequest>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationBundle {
    pub format: u32,
    pub id: String,
    pub version: u64,
    pub provider_sha256: String,
    pub wire_mode: RawWireMode,
    pub renderer: String,
    pub tokenizer_artifact: String,
    pub chat_template_artifact: String,
    pub qualification: TokenizerQualification,
    pub evidence: DeploymentEvidence,
}

pub struct RegisteredModel {
    pub bundle: QualificationBundle,
    pub tokenizer: QualifiedTokenizer,
    template: CompleteChatTemplate,
}
impl RegisteredModel {
    pub fn load(bundle: QualificationBundle) -> Result<Self> {
        if bundle.format != 1
            || !nonempty(&bundle.id)
            || bundle.version == 0
            || !valid_hash(&bundle.provider_sha256)
            || bundle.renderer != RENDERER
            || bundle.tokenizer_artifact.len() > 32 * 1024 * 1024
            || bundle.chat_template_artifact.len() > 256 * 1024
            || !nonempty(&bundle.evidence.issuer)
            || !nonempty(&bundle.evidence.reference_implementation)
            || bundle.evidence.reviewed_at <= 0
            || !valid_hash(&bundle.evidence.deployment_sha256)
            || bundle.evidence.reference_requests.len() < 3
            || bundle.evidence.reference_requests.len() > 128
            || bundle.qualification.reference_implementation
                != bundle.evidence.reference_implementation
            || bundle.qualification.parity_evidence_sha256 != digest(&bundle.evidence)?
        {
            return Err(Error::UnqualifiedTokenizer);
        }
        let template: CompleteChatTemplate = serde_json::from_str(&bundle.chat_template_artifact)
            .map_err(|_| Error::UnqualifiedTokenizer)?;
        if template.format != 1
            || ["system", "user", "assistant", "tool"].iter().any(|r| {
                !template.role_prefix.contains_key(*r) || !template.role_suffix.contains_key(*r)
            })
            || template.role_prefix.len() != 4
            || template.role_suffix.len() != 4
        {
            return Err(Error::UnqualifiedTokenizer);
        }
        let trusted = TrustedTokenizerBinding {
            model: bundle.qualification.model.clone(),
            qualification_sha256: digest(&bundle.qualification)?,
        };
        let tokenizer = QualifiedTokenizer::load(
            bundle.tokenizer_artifact.as_bytes(),
            bundle.chat_template_artifact.as_bytes(),
            &bundle.qualification,
            &trusted,
        )?;
        let registered = Self {
            bundle,
            tokenizer,
            template,
        };
        let mut tool_sample = false;
        let mut structured_sample = false;
        for sample in &registered.bundle.evidence.reference_requests {
            let rendered = registered.render(&sample.messages, &sample.tools, &sample.output)?;
            if rendered != sample.rendered_text || sample.token_ids.is_empty() {
                return Err(Error::UnqualifiedTokenizer);
            }
            // Reference vectors must be part of the independently attested
            // tokenizer qualification; matching an application-generated sample
            // does not create a trusted operator register entry on its own.
            if !registered
                .bundle
                .qualification
                .reference_encodings
                .iter()
                .any(|r| r.rendered_text == rendered && r.token_ids == sample.token_ids)
            {
                return Err(Error::UnqualifiedTokenizer);
            }
            tool_sample |= sample.tools.as_array().is_some_and(|t| !t.is_empty())
                && sample.messages.iter().any(|m| m["role"] == "tool");
            structured_sample |= !sample.output.is_null();
        }
        if !tool_sample || !structured_sample {
            return Err(Error::UnqualifiedTokenizer);
        }
        Ok(registered)
    }
    pub fn render(&self, messages: &[Value], tools: &Value, output: &Value) -> Result<String> {
        if messages.len() > 10000 || !tools.is_array() {
            return Err(Error::Invalid("invalid complete request".into()));
        }
        let mut text = self.template.prefix.clone();
        for message in messages {
            let role = message["role"]
                .as_str()
                .ok_or(Error::UnqualifiedTokenizer)?;
            text.push_str(
                self.template
                    .role_prefix
                    .get(role)
                    .ok_or(Error::UnqualifiedTokenizer)?,
            );
            text.push_str(
                &serde_json::to_string(message).map_err(|e| Error::Invalid(e.to_string()))?,
            );
            text.push_str(
                self.template
                    .role_suffix
                    .get(role)
                    .ok_or(Error::UnqualifiedTokenizer)?,
            );
        }
        text.push_str(&self.template.tools_prefix);
        text.push_str(TOOL_PROTOCOL);
        text.push_str(&serde_json::to_string(tools).map_err(|e| Error::Invalid(e.to_string()))?);
        text.push_str(&self.template.tools_suffix);
        text.push_str(&self.template.output_prefix);
        text.push_str(&serde_json::to_string(output).map_err(|e| Error::Invalid(e.to_string()))?);
        text.push_str(&self.template.output_suffix);
        text.push_str(&self.template.generation_prompt);
        if text.len() > 64 * 1024 * 1024 {
            return Err(Error::ContextLimit);
        }
        Ok(text)
    }
    pub fn prompt(
        &self,
        messages: &[Value],
        tools: &Value,
        output: &Value,
        logical: &str,
    ) -> Result<RenderedPrompt> {
        Ok(RenderedPrompt {
            model: self.bundle.qualification.model.clone(),
            logical_request_sha256: logical.into(),
            text: self.render(messages, tools, output)?,
        })
    }
    pub fn identity(&self) -> &ModelIdentity {
        self.tokenizer.model()
    }
    pub fn sha256(&self) -> Result<String> {
        digest(&self.bundle)
    }
    pub fn artifact_sha256(&self) -> String {
        sha256(self.bundle.tokenizer_artifact.as_bytes())
    }
}
