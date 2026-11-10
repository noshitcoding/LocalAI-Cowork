//! Sampling consumes the original graph budgets without replacing its tool turn.
use super::*;
use crate::mcp_interactions::McpInteractionRecord;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSamplingModel {
    pub claim: WorkflowClaim,
    pub parent_turn_id: String,
    pub raw_id: String,
    pub parent_tool_id: String,
    pub interaction_id: String,
    pub context: WorkflowModelContext,
    pub response: Option<WorkflowModelResponse>,
    pub response_sha256: Option<String>,
}

pub(crate) fn context<'a>(
    workflow: &'a StoredWorkflow,
    id: &str,
) -> Result<&'a WorkflowModelContext> {
    if let Some(turn) = agents::find_turn(workflow, id) {
        return Ok(&turn.context);
    }
    Ok(&workflow.samplings.get(id).ok_or(Error::Forbidden)?.context)
}

pub(crate) fn deadline(parent: &StoredRun, workflow: &StoredWorkflow, id: &str) -> Result<i64> {
    if let Some(turn) = agents::find_turn(workflow, id) {
        return tools::record_tool_deadline(&parent.record, workflow, turn);
    }
    let sampling = workflow.samplings.get(id).ok_or(Error::Forbidden)?;
    let turn = agents::find_turn(workflow, &sampling.parent_turn_id).ok_or(Error::Forbidden)?;
    tools::record_tool_deadline(&parent.record, workflow, turn)
}

pub(super) fn validate(workflow: &StoredWorkflow, run: &RunRecord) -> Result<()> {
    if !workflow.samplings.is_empty() && workflow.format_version < 5 {
        return Err(Error::Forbidden);
    }
    for (id, value) in &workflow.samplings {
        let (turn, tool) = tools::proposal(workflow, &value.parent_turn_id, &value.raw_id)?;
        let attempt = workflow
            .progress
            .attempts()
            .get(id)
            .ok_or(Error::Forbidden)?;
        let parent = workflow
            .progress
            .attempts()
            .get(&tool.id)
            .ok_or(Error::Forbidden)?;
        let mut expected = agents::spec_for(run, workflow, &turn.claim.node_id)?;
        let cap = value.context.spec.budgets.max_output_tokens;
        if cap == 0 || cap > expected.budgets.max_output_tokens {
            return Err(Error::Forbidden);
        }
        expected.budgets.max_output_tokens = cap;
        expected.allowed_tools.clear();
        expected.mcp_tools.clear();
        expected.output = None;
        expected.file_action = None;
        if id
            != &canonical_hash(&json!([
                "workflow-mcp-sampling/1",
                run.id,
                value.parent_tool_id,
                value.interaction_id
            ]))
            || !tool.name.starts_with("mcp_")
            || value.parent_tool_id != tool.id
            || value.claim != parent.claim
            || value.claim != attempt.claim
            || canonical_hash(&expected) != canonical_hash(&value.context.spec)
            || value.context.tools != json!([])
            || !value.context.child_results.is_null()
            || value.context.request_sha256
                != model_request_fingerprint(&expected, &value.context.messages, &json!([]))?
            || !matches!(&attempt.operation, WorkflowOperation::Model { request_sha256, .. } if request_sha256 == &value.context.request_sha256)
            || value.response.as_ref().map(canonical_hash) != value.response_sha256
            || value.response_sha256 != attempt.response_sha256
            || json_string(value)?.len() > 2 * 1024 * 1024
        {
            return Err(Error::Forbidden);
        }
        let system = turn
            .context
            .messages
            .iter()
            .filter(|m| m["role"] == "system")
            .collect::<Vec<_>>();
        if value
            .context
            .messages
            .iter()
            .take(system.len())
            .collect::<Vec<_>>()
            != system
            || value
                .context
                .messages
                .iter()
                .skip(system.len())
                .any(|m| !matches!(m["role"].as_str(), Some("user" | "assistant")))
        {
            return Err(Error::Forbidden);
        }
        if value.response.as_ref().is_some_and(|response| {
            !response.tools.is_empty()
                || response.content.len() > 65_536
                || response.content.contains('\0')
                || response.rejection_code.is_some()
                || response
                    .usage
                    .cost
                    .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
        }) {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}

impl Store {
    #[expect(
        clippy::too_many_arguments,
        reason = "admit_workflow_mcp_sampling: Keep workflow project/run/lease/turn and original sampling interaction/wire bindings explicit"
    )]
    pub async fn admit_workflow_mcp_sampling(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn_id: &str,
        raw: &str,
        id: &str,
        interaction: &McpInteractionRecord,
        context: WorkflowModelContext,
        wire: &str,
    ) -> Result<()> {
        let parent = self.stored_run(run).await?;
        parent.assert_lease(lease, now())?;
        if parent.record.spec.project_id != project {
            return Err(Error::Forbidden);
        }
        let before = self.stored_workflow(project, run).await?;
        let (turn, tool) = tools::proposal(&before, turn_id, raw)?;
        let claim = tools::execution_claim(&before, turn)?.clone();
        if interaction.view.method != "sampling/createMessage"
            || interaction.view.state != "approved"
            || interaction.task.view.run_id != run
            || interaction.task.view.tool_call_id != tool.id
            || interaction.task.spec_sha256 != canonical_hash(&parent.record.spec)
            || before.samplings.contains_key(id)
        {
            return Err(Error::UncertainEffect);
        }
        let registered = self.registered_model(&context.spec).await?;
        let reserved = registered
            .as_ref()
            .map(|model| {
                let prompt = model
                    .prompt(
                        &context.messages,
                        &json!([]),
                        &Value::Null,
                        &context.request_sha256,
                    )
                    .map_err(storage_error)?;
                let count = model
                    .tokenizer
                    .count(&prompt, u64::from(context.spec.budgets.max_output_tokens))
                    .map_err(|_| Error::ContextLimit)?;
                count
                    .input_tokens
                    .checked_add(count.output_allowance)
                    .ok_or(Error::ContextLimit)
            })
            .transpose()?;
        let mut next = before.clone();
        let admission = next.progress.admit_tool_sampling(
            &next.plan,
            next.progress.revision(),
            &claim,
            id,
            cowork_domain::workflows::WorkflowSamplingRequest {
                parent_tool_id: &tool.id,
                request_sha256: &context.request_sha256,
                reserved_tokens: reserved,
            },
            seconds(),
        )?;
        if !matches!(admission, WorkflowAdmission::Dispatch(_)) {
            return Err(Error::UncertainEffect);
        }
        next.samplings.insert(
            id.into(),
            WorkflowSamplingModel {
                claim,
                parent_turn_id: turn_id.into(),
                raw_id: raw.into(),
                parent_tool_id: tool.id,
                interaction_id: interaction.view.id.clone(),
                context: context.clone(),
                response: None,
                response_sha256: None,
            },
        );
        next.format_version = 5;
        validate(&next, &parent.record)?;
        let intent = crate::model_money::AccountingIntent::Admit {
            attempt_id: id,
            logical: &context.request_sha256,
            wire,
        };
        self.mutate_with_accounting(run, Some(lease), Some(&intent), |current| {
            tools::current_authority(current)?;
            if canonical_hash(&current.record.spec) != canonical_hash(&parent.record.spec) { return Err(Error::Conflict); }
            let mut change = Change::event("workflow_mcp_sampling_started", json!({"attempt_id":id,"interaction_id":interaction.view.id,"parent_tool_id":interaction.task.view.tool_call_id,"request_sha256":context.request_sha256,"wire_sha256":wire}));
            change.workflow_tool_gate = Some(tools::ToolGate::ExternalCheck { turn: turn_id.into(), raw_id: raw.into() });
            change.workflow_write = Some(WorkflowWrite { new_action: true, expected_revision: before.progress.revision(), expected_sha256: canonical_hash(&before.progress), next: next.clone() });
            Ok(change)
        }).await?;
        Ok(())
    }

    pub async fn finish_workflow_mcp_sampling(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        id: &str,
        response: WorkflowModelResponse,
        received: Option<&crate::model_money::CompletedModelReceipt>,
    ) -> Result<()> {
        let parent = self.stored_run(run).await?;
        if parent.record.spec.project_id != project {
            return Err(Error::Forbidden);
        }
        let before = self.stored_workflow(project, run).await?;
        let sampling = before.samplings.get(id).ok_or(Error::Forbidden)?;
        if sampling.response.is_some() || !response.tools.is_empty() {
            return Err(Error::UncertainEffect);
        }
        let outcome = match (
            response.usage.input_tokens,
            response.usage.output_tokens,
            response.usage.incomplete,
        ) {
            (Some(input_tokens), Some(output_tokens), false) => WorkflowOutcome::Measured {
                input_tokens,
                output_tokens,
            },
            _ => WorkflowOutcome::Unmeasured {
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
            },
        };
        let sha = canonical_hash(&response);
        let mut next = before.clone();
        next.progress.settle_response(
            &next.plan,
            next.progress.revision(),
            &sampling.claim,
            id,
            WorkflowResponseEvidence {
                outcome,
                sha256: sha.clone(),
            },
            seconds(),
        )?;
        let changed = next.samplings.get_mut(id).ok_or(Error::Forbidden)?;
        changed.response = Some(response.clone());
        changed.response_sha256 = Some(sha.clone());
        let intent = received.map(|receipt| crate::model_money::AccountingIntent::Settle {
            attempt_id: id,
            receipt,
        });
        self.mutate_with_accounting(run, Some(lease), intent.as_ref(), |current| {
            tools::current_authority(current)?;
            let mut change = Change::event(
                "workflow_mcp_sampling_received",
                json!({"attempt_id":id,"response_sha256":sha,"awaiting_human_release":true}),
            );
            change.workflow_tool_gate = Some(tools::ToolGate::ExternalCheck {
                turn: sampling.parent_turn_id.clone(),
                raw_id: sampling.raw_id.clone(),
            });
            change.workflow_write = Some(WorkflowWrite {
                new_action: false,
                expected_revision: before.progress.revision(),
                expected_sha256: canonical_hash(&before.progress),
                next: next.clone(),
            });
            if next.progress.state() == WorkflowState::Interrupted {
                current.transition(RunState::Interrupted)?;
                current.lease = None;
                current.record.error_code = Some("workflow_sampling_incomplete".into());
            }
            Ok(change)
        })
        .await?;
        Ok(())
    }
}
