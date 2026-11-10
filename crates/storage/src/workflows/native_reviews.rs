//! Native human review reuses the durable input journal without a model role.
use super::*;
use cowork_domain::workflows::{WorkflowReviewForm, WorkflowReviewRequest};

pub(super) fn spec(run: &RunRecord, node: &WorkflowNodePlan) -> Result<RunSpec> {
    if node.role_id.is_some()
        || !node.rights.tools.contains("request_input")
        || !node.rights.folders.is_empty()
    {
        return Err(Error::Forbidden);
    }
    let mut spec = run.spec.clone();
    spec.provider = None;
    spec.model_binding = None;
    spec.output = None;
    spec.agent_config = None;
    spec.environment = None;
    spec.budgets = node.budgets.clone();
    spec.allowed_tools.retain(|tool| tool == "request_input");
    spec.folders.clear();
    spec.mcp_tools.clear();
    if spec.allowed_tools.len() != 1 {
        return Err(Error::Forbidden);
    }
    Ok(spec)
}
pub(super) fn system(node: &str, form: &WorkflowReviewForm) -> Value {
    json!({"role":"system","content":json!({"native_review_step":node,"form":form}).to_string()})
}
pub(super) fn request(
    workflow: &StoredWorkflow,
    turn: &WorkflowModelTurn,
) -> Result<Option<WorkflowReviewRequest>> {
    let definition = workflow.execution_definition()?;
    if definition.review_form(&turn.claim.node_id).is_none() {
        return Ok(None);
    }
    if !turn.deterministic || turn.context.spec.provider.is_some() {
        return Err(Error::Forbidden);
    }
    let review = definition.review_request(
        &turn.claim.node_id,
        workflow.binding.inputs.as_deref(),
        &super::native_documents::completed(workflow),
    )?;
    if serde_json::to_vec(&review.values)
        .map_err(storage_error)?
        .len()
        > 32_000
    {
        return Err(Error::ContextLimit);
    }
    Ok(Some(review))
}
pub(super) fn fingerprint(spec: &RunSpec, messages: &[Value], tools: &Value) -> Result<String> {
    if spec.provider.is_some()
        || spec.model_binding.is_some()
        || spec.workflow.is_none()
        || spec.environment.is_some()
        || spec.allowed_tools != ["request_input"]
        || !spec.folders.is_empty()
        || !spec.mcp_tools.is_empty()
    {
        return Err(Error::Forbidden);
    }
    let frame = json!({"format":"workflow-native-review-context/1","spec":spec,"messages":messages,"tools":tools});
    if serde_json::to_vec(&frame).map_err(storage_error)?.len() > 2 * 1024 * 1024 {
        return Err(Error::ContextLimit);
    }
    Ok(canonical_hash(&frame))
}
pub(crate) fn answer(review: Option<&WorkflowReviewRequest>, answer: &str) -> Result<()> {
    if let Some(review) = review {
        let value: Value = serde_json::from_str(answer)
            .map_err(|_| Error::Validation("workflow_review_invalid_json".into()))?;
        review.form.validate_answer(&value)?;
    }
    Ok(())
}
pub(super) fn output(turn: &WorkflowModelTurn) -> Result<String> {
    if !turn.deterministic
        || turn.tool_results.len() != 1
        || !turn.tool_results[0].succeeded
        || turn.tool_results[0].tool.name != "request_input"
    {
        return Err(Error::Forbidden);
    }
    let system: Value = serde_json::from_str(
        turn.context
            .messages
            .first()
            .and_then(|m| m["content"].as_str())
            .ok_or(Error::Forbidden)?,
    )
    .map_err(storage_error)?;
    if system["native_review_step"] != turn.claim.node_id {
        return Err(Error::Forbidden);
    }
    let form: WorkflowReviewForm =
        serde_json::from_value(system["form"].clone()).map_err(storage_error)?;
    let answer = turn.tool_results[0].result["answer"]
        .as_str()
        .ok_or(Error::Forbidden)?;
    let value: Value = serde_json::from_str(answer).map_err(storage_error)?;
    form.validate_answer(&value)?;
    serde_json::to_string(&value).map_err(storage_error)
}
