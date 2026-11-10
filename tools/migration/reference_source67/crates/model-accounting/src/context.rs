//! Explicit, archived context selection; no automatic summarization.
use crate::tokenizer::{ModelIdentity, QualifiedTokenizer, RenderedPrompt, TokenCount};
use crate::{Error, Result, digest, nonempty, valid_hash};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRetention {
    /// System/rights/current user request, pending effects, tool call/result pairs,
    /// unresolved approvals, questions, unconfirmed usage and receipts.
    Required,
    /// One complete historical exchange with no unresolved effect or accounting.
    ConfirmedHistory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextGroup {
    pub id: String,
    pub retention: ContextRetention,
    /// Exact complete logical messages. Group boundaries are effect boundaries.
    pub messages: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextSnapshot {
    pub run_id: String,
    pub run_revision: u64,
    pub scope_sha256: String,
    pub model: ModelIdentity,
    pub groups: Vec<ContextGroup>,
}

/// The application must validate explicit human authority and durable archive
/// existence before passing this capability. It must never derive it from LLM text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionAuthority {
    pub operation_id: String,
    pub snapshot_sha256: String,
    pub scope_sha256: String,
    pub archive_receipt_sha256: String,
    /// Exact full group hashes permitted for removal, not a broad discard grant.
    pub removable_groups: BTreeMap<String, String>,
}

/// Adapter is trusted and qualified against the exact provider/model template.
/// It must render all supplied groups/tools; partial or approximate renderers do
/// not satisfy this interface's integration contract.
pub trait ExactPromptRenderer {
    fn render(&self, snapshot: &ContextSnapshot) -> Result<RenderedPrompt>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionReceipt {
    pub operation_id: String,
    pub original_snapshot_sha256: String,
    pub retained_snapshot_sha256: String,
    pub archive_receipt_sha256: String,
    pub removed_groups: BTreeMap<String, String>,
    pub retained: ContextSnapshot,
    pub count: TokenCount,
}

pub fn compact(
    original: &ContextSnapshot,
    authority: &CompactionAuthority,
    tokenizer: &QualifiedTokenizer,
    renderer: &impl ExactPromptRenderer,
    output_allowance: u64,
) -> Result<CompactionReceipt> {
    original.model.validate()?;
    if !nonempty(&original.run_id)
        || !nonempty(&authority.operation_id)
        || !valid_hash(&original.scope_sha256)
        || authority.scope_sha256 != original.scope_sha256
        || authority.snapshot_sha256 != digest(original)?
        || !valid_hash(&authority.archive_receipt_sha256)
        || original.model != *tokenizer.model()
    {
        return Err(Error::Conflict);
    }
    let mut ids = BTreeSet::new();
    for group in &original.groups {
        if !nonempty(&group.id) || group.messages.is_empty() || !ids.insert(group.id.clone()) {
            return Err(Error::Invalid("invalid context group".into()));
        }
    }
    for (id, expected) in &authority.removable_groups {
        let group = original
            .groups
            .iter()
            .find(|group| &group.id == id)
            .ok_or(Error::Conflict)?;
        if group.retention != ContextRetention::ConfirmedHistory || digest(group)? != *expected {
            return Err(Error::Conflict);
        }
    }
    let mut retained = original.clone();
    let mut removed_groups = BTreeMap::new();
    loop {
        let prompt = renderer.render(&retained)?;
        if prompt.logical_request_sha256 != digest(&retained)? || prompt.model != retained.model {
            return Err(Error::UnqualifiedTokenizer);
        }
        match tokenizer.count(&prompt, output_allowance) {
            Ok(count) => {
                return Ok(CompactionReceipt {
                    operation_id: authority.operation_id.clone(),
                    original_snapshot_sha256: digest(original)?,
                    retained_snapshot_sha256: digest(&retained)?,
                    archive_receipt_sha256: authority.archive_receipt_sha256.clone(),
                    removed_groups,
                    retained,
                    count,
                });
            }
            Err(Error::ContextLimit) => {}
            Err(error) => return Err(error),
        }
        let index = retained
            .groups
            .iter()
            .position(|group| authority.removable_groups.contains_key(&group.id))
            .ok_or(Error::ContextLimit)?;
        let group = retained.groups.remove(index);
        removed_groups.insert(group.id.clone(), digest(&group)?);
    }
}
