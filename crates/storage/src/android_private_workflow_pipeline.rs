//! Pure proposals over an authenticated actor graph, followed by exactly the
//! existing browser approval, original network admission and received evidence.
use super::*;
use crate::android_private_tools as tools;

/// V1's canonical import explicitly depends on every prior result. Preserve
/// those actual bytes without requiring an extra artifact tool grant. The
/// existing encrypted-checkpoint and model-context budgets still bound them.
pub(super) fn complete_inline_history(plan: &WorkflowPlan) -> bool {
    matches!(plan.definition.mode, WorkflowMode::Sequential)
        && plan
            .definition
            .steps
            .iter()
            .enumerate()
            .all(|(index, step)| {
                let expected: std::collections::BTreeSet<_> = plan.definition.steps[..index]
                    .iter()
                    .map(|step| step.id.as_str())
                    .collect();
                step.dependencies
                    .iter()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>()
                    == expected
            })
}

fn proposal(plan: &WorkflowPlan, cp: &Value) -> Result<Option<ToolCall>> {
    let node = cp["workflow_node"]["node_id"]
        .as_str()
        .ok_or(Error::Forbidden)?;
    let Some(execution) = plan
        .definition
        .steps
        .iter()
        .find(|step| step.id == node)
        .and_then(|step| step.execution.as_ref())
    else {
        return Ok(None);
    };
    if cp["workflow_node"]["plan_sha256"] != plan.sha256
        || !cp["spec"]["allowed_tools"]
            .as_array()
            .is_some_and(|tools| tools.contains(&json!("browser")))
    {
        return Err(Error::Forbidden);
    }
    let arguments = crate::android_private_browser::pipeline_action(execution)?;
    Ok(Some(ToolCall {
        id: format!(
            "pipeline-{}",
            canonical_hash(
                &json!({"plan":plan.sha256,"node":node,"claim":cp["workflow_node"]["claim"],"execution":execution})
            )
        ),
        name: "browser".into(),
        arguments,
    }))
}
fn receipt_output(receipt: &Value, tool: &ToolCall) -> Result<String> {
    let result = &receipt["result"];
    if result["operation"] != tool.arguments["operation"]
        || !result["error_type"].is_null()
        || result["denied"] == true
        || receipt["paused"] != true
    {
        return Err(Error::Validation("actor_pipeline_web_step_failed".into()));
    }
    result["text"]
        .as_str()
        .filter(|text| !text.is_empty() && text.len() <= 65536 && !text.contains('\0'))
        .map(str::to_owned)
        .ok_or_else(|| Error::Validation("actor_pipeline_web_step_failed".into()))
}
impl Store {
    /// True means this is a deterministic node. This call never performs IO or
    /// obtains a model credential; the worker next uses its ordinary tool path.
    pub async fn android_private_workflow_pipeline_progress(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<bool> {
        let key = workspace::writer_cipher(keys, key)?;
        let scope = current_scope().ok_or(Error::Forbidden)?;
        if scope.run != run || scope.lease != lease {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let mut cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(self, &mut tx, &r, &cp, now).await?;
        let root = root_checkpoint(&mut tx, &r, keys).await?;
        let plan: WorkflowPlan =
            serde_json::from_value(root["workflow"]["plan"].clone()).map_err(storage_error)?;
        let Some(tool) = proposal(&plan, &cp)? else {
            tx.commit().await.map_err(storage_error)?;
            return Ok(false);
        };
        if !cp["pending_tools"].as_array().is_some_and(Vec::is_empty) {
            return Err(Error::Conflict);
        }
        let previous = canonical_hash(&cp);
        let marker = json!({"format":"actor-private-pipeline/1","tool":tool});
        if cp["pipeline_preparation"].is_null() {
            if cp["model_steps"] != 0 || cp["tool_steps"] != 0 || !cp["last_content"].is_null() {
                return Err(Error::Conflict);
            }
            let prefix = invocation_prefix(&cp)?.ok_or(Error::Forbidden)?;
            let sent:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_loop_steps WHERE run_id=$1 AND invocation_id LIKE $2").bind(run.to_string()).bind(format!("{prefix}%")).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if sent != 0 {
                return Err(Error::UncertainEffect);
            }
            cp["messages"].as_array_mut().ok_or(Error::Forbidden)?.push(json!({"role":"assistant","content":"","tool_calls":[{"id":tool.id,"type":"function","function":{"name":tool.name,"arguments":json_string(&tool.arguments)?}}]}));
            cp["pending_tools"] = json!([tool]);
            cp["last_turn_has_tools"] = json!(true);
            cp["pipeline_preparation"] = marker;
        } else {
            if cp["pipeline_preparation"] != marker
                || cp["model_steps"] != 0
                || cp["tool_steps"] != 1
            {
                return Err(Error::UncertainEffect);
            }
            let invocation = tools::invocation(&cp, &tool)?;
            let step=sqlx::query("SELECT sequence,intent,request_sha FROM android_private_loop_steps WHERE run_id=$1 AND invocation_id=$2 AND lease_id=$3 AND state='received'").bind(run.to_string()).bind(invocation).bind(lease.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
            let sequence: i64 = step.try_get("sequence").map_err(storage_error)?;
            let intent: Value = serde_json::from_slice(&workspace::open_value(
                keys,
                &r,
                &format!("intent:{sequence}"),
                &parse(&step.try_get::<String, _>("intent").map_err(storage_error)?)?,
            )?)
            .map_err(storage_error)?;
            if intent["tool"] != serde_json::to_value(&tool).map_err(storage_error)?
                || canonical_hash(&intent)
                    != step
                        .try_get::<String, _>("request_sha")
                        .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            let receipt = tools::step_receipt(&mut tx, &r, sequence, keys).await?;
            let output = receipt_output(&receipt, &tool)?;
            cp["last_content"] = json!(output);
            cp["last_turn_has_tools"] = json!(false);
            cp["last_receipt_sha"] = json!(canonical_hash(&receipt));
            cp["pipeline_receipt_sequence"] = json!(sequence);
        }
        tools::loop_authority(self, &mut tx, &r, &cp, now).await?;
        tools::set_checkpoint(&mut tx, &r, &cp, Some(&previous), keys, key, now).await?;
        tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        event(&mut tx,run,"workflow_pipeline_progress",&json!({"node_id":scope.node,"provider_contacted":false,"prepared":cp["pipeline_receipt_sequence"].is_null()}),now).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_received_successful_bounded_browser_text_can_complete() {
        let tool = ToolCall {
            id: "typed".into(),
            name: "browser".into(),
            arguments: json!({"operation":"fetch_text"}),
        };
        assert_eq!(receipt_output(&json!({"paused":true,"result":{"operation":"fetch_text","text":"Actual retained evidence"}}),&tool).unwrap(),"Actual retained evidence");
        for value in [
            json!({"paused":false,"result":{"operation":"fetch_text","text":"unsafe"}}),
            json!({"paused":true,"result":{"operation":"fetch_text","error_type":"TimeoutError","text":"partial"}}),
            json!({"paused":true,"result":{"denied":true}}),
            json!({"paused":true,"result":{"operation":"search","text":"wrong action"}}),
        ] {
            assert!(receipt_output(&value, &tool).is_err())
        }
    }
}
