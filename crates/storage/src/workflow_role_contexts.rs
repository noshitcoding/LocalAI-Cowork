//! Admission-time role context snapshots. Child execution never reads mutable memory.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
const MARKER: &str = "cowork_workflow_role_contexts_v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    format: u8,
    plan_sha256: String,
    parent_context_index: Option<usize>,
    parent_context_sha256: Option<String>,
    roles: BTreeMap<String, String>,
    contexts: BTreeMap<String, Option<String>>,
}
pub(crate) struct SelectedContext {
    pub content: Option<String>,
    pub parent_context_index: Option<usize>,
}
pub(crate) async fn freeze(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    spec: &RunSpec,
    plan: &cowork_domain::workflows::WorkflowPlan,
    messages: &mut Vec<Value>,
) -> Result<()> {
    if messages.iter().any(|m| m.get(MARKER).is_some()) {
        return Err(Error::Conflict);
    }
    let parent_context = crate::product_context::context(tx, spec).await?;
    let parent_context_index = parent_context
        .as_ref()
        .map(|text| {
            messages
                .iter()
                .position(|m| m["role"] == "user" && m["content"] == text.as_str())
                .ok_or(Error::Conflict)
        })
        .transpose()?;
    let mut snapshot = Snapshot {
        format: 1,
        plan_sha256: plan.sha256.clone(),
        parent_context_index,
        parent_context_sha256: parent_context.as_ref().map(canonical_hash),
        roles: BTreeMap::new(),
        contexts: BTreeMap::new(),
    };
    for (id, role) in &plan.roles {
        let mut scoped = spec.clone();
        scoped.agent_config = cowork_domain::merge_agent_values(
            spec.agent_config.as_ref(),
            role.agent_config.as_ref(),
        );
        let context = crate::product_context::context(tx, &scoped).await?;
        let digest = canonical_hash(&context);
        snapshot.roles.insert(id.clone(), digest.clone());
        snapshot.contexts.entry(digest).or_insert(context);
    }
    let value = serde_json::to_value(snapshot).map_err(storage_error)?;
    if json_string(&value)?.len() > 1_048_576 {
        return Err(Error::Validation("workflow_role_context_limit".into()));
    }
    let mut frame = json!({"role":"system","content":""});
    frame[MARKER] = value;
    messages.push(frame);
    Ok(())
}
fn read(messages: &[Value], plan_sha256: &str) -> Result<Option<Snapshot>> {
    let frames = messages
        .iter()
        .filter(|m| m.get(MARKER).is_some())
        .collect::<Vec<_>>();
    if frames.is_empty() {
        return Ok(None);
    }
    if frames.len() != 1 || frames[0]["role"] != "system" || frames[0]["content"] != "" {
        return Err(Error::Forbidden);
    }
    let snapshot: Snapshot =
        serde_json::from_value(frames[0][MARKER].clone()).map_err(|_| Error::Forbidden)?;
    if snapshot.format != 1 || snapshot.plan_sha256 != plan_sha256 {
        return Err(Error::Forbidden);
    }
    if snapshot.roles.values().collect::<BTreeSet<_>>()
        != snapshot.contexts.keys().collect::<BTreeSet<_>>()
    {
        return Err(Error::Forbidden);
    }
    match (
        snapshot.parent_context_index,
        snapshot.parent_context_sha256.as_ref(),
    ) {
        (None, None) => {}
        (Some(index), Some(digest)) => {
            let message = messages.get(index).ok_or(Error::Forbidden)?;
            if message["role"] != "user"
                || message["content"]
                    .as_str()
                    .is_none_or(|content| canonical_hash(&content) != *digest)
            {
                return Err(Error::Forbidden);
            }
        }
        _ => return Err(Error::Forbidden),
    }
    for (digest, context) in &snapshot.contexts {
        if canonical_hash(context) != *digest
            || context.as_ref().is_some_and(|text| {
                !text.starts_with(crate::product_context::PROJECT_CONTEXT_PREFIX)
            })
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(Some(snapshot))
}
pub(crate) fn selected(
    messages: &[Value],
    plan: &cowork_domain::workflows::WorkflowPlan,
    role: &str,
) -> Result<Option<SelectedContext>> {
    let Some(snapshot) = read(messages, &plan.sha256)? else {
        return Ok(None);
    };
    if snapshot.roles.keys().collect::<BTreeSet<_>>() != plan.roles.keys().collect::<BTreeSet<_>>()
    {
        return Err(Error::Forbidden);
    }
    let digest = snapshot.roles.get(role).ok_or(Error::Forbidden)?;
    Ok(Some(SelectedContext {
        content: snapshot
            .contexts
            .get(digest)
            .ok_or(Error::Forbidden)?
            .clone(),
        parent_context_index: snapshot.parent_context_index,
    }))
}
pub(crate) fn audit_messages(messages: &[Value], plan_sha256: &str) -> Result<Vec<Value>> {
    Ok(read(messages, plan_sha256)?
        .map(|snapshot| {
            snapshot
                .contexts
                .into_values()
                .flatten()
                .map(|text| json!({"role":"user","content":text}))
                .collect()
        })
        .unwrap_or_default())
}
