//! Original asynchronous MCP evidence. No Actor/session or replacement send is
//! constructed here; all child responses must already have actual receipts.
use super::*;
use crate::mcp_tasks::McpTaskRecord;
use cowork_secrets::{KeyId, KeyStore, McpSecretScope, SealedMcpBlob, open_mcp_blob};
use sqlx::{Any, Transaction, any::AnyRow};

fn chain(previous: &mut String, value: &Value) {
    *previous = canonical_hash(&json!([previous, value]));
}
fn open(keys: &dyn KeyStore, scope: &McpSecretScope, raw: &str) -> Result<Value> {
    serde_json::from_slice(
        &open_mcp_blob(keys, scope, &parse::<SealedMcpBlob>(raw)?)
            .map_err(|_| Error::NativeKeyUnavailable)?,
    )
    .map_err(storage_error)
}
fn scope(task: &McpTaskRecord, id: &str, revision: i64, purpose: &str) -> McpSecretScope {
    let mut s = task.scope.clone();
    s.record_id = id.into();
    s.revision = revision;
    s.purpose = purpose.into();
    s
}

pub(crate) async fn private_quiescence(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    keys: &dyn KeyStore,
) -> Result<(String, String)> {
    let run = crate::android_private_server::id(r, "id")?.to_string();
    let owner = crate::android_private_server::id(r, "owner_id")?.to_string();
    let project = crate::android_private_server::id(r, "project_id")?.to_string();
    let mut intents = canonical_hash(&json!(["mcp-original-intents/1", run, owner, project]));
    let mut receipts = canonical_hash(&json!(["mcp-original-receipts/1", run, owner, project]));
    let mut after = String::new();
    loop {
        let row = sqlx::query(
            "SELECT * FROM mcp_async_tasks WHERE run_id=$1 AND id>$2 ORDER BY id LIMIT 1",
        )
        .bind(&run)
        .bind(&after)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?;
        let Some(row) = row else { break };
        let task = crate::mcp_tasks::task_row(&row)?;
        after = task.view.id.clone();
        if task.owner_kind != "personal"
            || task.owner_id != owner
            || task.project_id != project
            || task.pending_operation_id.is_some()
        {
            return Err(Error::UncertainEffect);
        }
        let ack = task.open(keys)?;
        let final_scope: McpSecretScope = parse(
            &row.try_get::<Option<String>, _>("final_result_scope")
                .map_err(storage_error)?
                .ok_or(Error::UncertainEffect)?,
        )?;
        if final_scope.owner_kind != task.owner_kind
            || final_scope.owner_id != owner
            || final_scope.project_id != project
            || final_scope.server_id != task.view.server_id
            || final_scope.configuration_sha256 != task.binding_sha256
            || final_scope.purpose != "task_response"
        {
            return Err(Error::Forbidden);
        }
        let final_raw = row
            .try_get::<Option<String>, _>("sealed_final_result")
            .map_err(storage_error)?
            .ok_or(Error::UncertainEffect)?;
        let result = open(keys, &final_scope, &final_raw)?;
        if crate::mcp_tasks::result_state(&ack.current, &result)? != task.view.state
            || Some(canonical_hash(&result["response"])) != task.view.result_sha256
        {
            return Err(Error::Forbidden);
        }
        let mut op_after = String::new();
        let mut final_found = false;
        loop {
            let op=sqlx::query("SELECT * FROM mcp_async_task_operations WHERE task_record_id=$1 AND id>$2 ORDER BY id LIMIT 1").bind(&after).bind(&op_after).fetch_optional(&mut **tx).await.map_err(storage_error)?;
            let Some(op) = op else { break };
            op_after = op.try_get("id").map_err(storage_error)?;
            if op.try_get::<String, _>("state").map_err(storage_error)? != "committed" {
                return Err(Error::UncertainEffect);
            }
            let revision = op
                .try_get::<i64, _>("expected_revision")
                .map_err(storage_error)?;
            let expected = scope(&task, &op_after, revision, "task_response");
            let actual: McpSecretScope = parse(
                &op.try_get::<String, _>("secret_scope")
                    .map_err(storage_error)?,
            )?;
            if actual != expected {
                return Err(Error::Forbidden);
            }
            let raw = op
                .try_get::<Option<String>, _>("sealed_response")
                .map_err(storage_error)?
                .ok_or(Error::UncertainEffect)?;
            let response = open(keys, &expected, &raw)?;
            let sha = canonical_hash(&response);
            if Some(sha.clone())
                != op
                    .try_get::<Option<String>, _>("response_sha256")
                    .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            let receipt: Value = parse(
                &op.try_get::<Option<String>, _>("receipt")
                    .map_err(storage_error)?
                    .ok_or(Error::UncertainEffect)?,
            )?;
            if receipt["operation_id"] != op_after
                || receipt["task_record_id"] != after
                || receipt["response_sha256"] != sha
                || receipt["task_revision"].as_i64() != revision.checked_add(1)
            {
                return Err(Error::Forbidden);
            }
            let operation = op
                .try_get::<String, _>("operation")
                .map_err(storage_error)?;
            if op_after == final_scope.record_id {
                if operation != "result"
                    || actual != final_scope
                    || response != result
                    || raw != final_raw
                {
                    return Err(Error::Forbidden);
                }
                final_found = true;
            } else if operation != "result" {
                let current: cowork_mcp::TaskAcknowledgement =
                    serde_json::from_value(response["ack"].clone()).map_err(storage_error)?;
                current.validate().map_err(|_| Error::Forbidden)?;
                if response["kind"] != "status" || current.task_id() != ack.original.task_id() {
                    return Err(Error::Forbidden);
                }
            }
            chain(
                &mut intents,
                &json!([
                    after,
                    op_after,
                    revision,
                    operation,
                    op.try_get::<String, _>("rpc_request_id")
                        .map_err(storage_error)?,
                    op.try_get::<String, _>("request_sha256")
                        .map_err(storage_error)?,
                    op.try_get::<String, _>("authority_sha256")
                        .map_err(storage_error)?,
                    actual
                ]),
            );
            chain(&mut receipts, &json!([op_after, sha, receipt]));
        }
        if !final_found {
            return Err(Error::UncertainEffect);
        }
        let mut control_after = String::new();
        loop {
            let control=sqlx::query("SELECT * FROM mcp_task_control_requests WHERE task_record_id=$1 AND request_key>$2 ORDER BY request_key LIMIT 1").bind(&after).bind(&control_after).fetch_optional(&mut **tx).await.map_err(storage_error)?;
            let Some(control) = control else { break };
            control_after = control.try_get("request_key").map_err(storage_error)?;
            let state = control
                .try_get::<String, _>("state")
                .map_err(storage_error)?;
            if matches!(state.as_str(), "unknown" | "admitted") {
                return Err(Error::UncertainEffect);
            }
            chain(
                &mut intents,
                &json!([
                    after,
                    control_after,
                    control
                        .try_get::<i64, _>("expected_revision")
                        .map_err(storage_error)?,
                    control
                        .try_get::<String, _>("original_actor")
                        .map_err(storage_error)?
                ]),
            );
            chain(
                &mut receipts,
                &json!([
                    after,
                    control_after,
                    state,
                    parse::<Value>(
                        &control
                            .try_get::<String, _>("receipt")
                            .map_err(storage_error)?
                    )?
                ]),
            );
        }
        chain(
            &mut intents,
            &json!([
                after,
                task.view.tool_call_id,
                task.view.server_id,
                task.server_revision,
                task.binding_sha256,
                task.spec_sha256,
                task.connection_id,
                task.connection_epoch,
                task.original_rpc_id,
                task.original_request_sha256,
                task.transport_request_sha256,
                task.task_id_sha256
            ]),
        );
        chain(
            &mut receipts,
            &json!([
                after,
                canonical_hash(&ack),
                task.view.state,
                task.view.revision,
                canonical_hash(&result)
            ]),
        );
        let mut interaction_after = String::new();
        loop {
            let row=sqlx::query("SELECT * FROM mcp_task_interactions WHERE task_record_id=$1 AND id>$2 ORDER BY id LIMIT 1").bind(&after).bind(&interaction_after).fetch_optional(&mut **tx).await.map_err(storage_error)?;
            let Some(row) = row else { break };
            interaction_after = row.try_get("id").map_err(storage_error)?;
            let record = crate::mcp_interactions::record(tx, &interaction_after, keys).await?;
            if record.view.state != "responded" || record.task.view.id != after {
                return Err(Error::UncertainEffect);
            }
            let response_scope: McpSecretScope = parse(
                &row.try_get::<String, _>("response_scope")
                    .map_err(storage_error)?,
            )?;
            if response_scope != scope(&task, &interaction_after, 1, "task_interaction_response") {
                return Err(Error::Forbidden);
            }
            let response = open(
                keys,
                &response_scope,
                &row.try_get::<String, _>("sealed_response")
                    .map_err(storage_error)?,
            )?;
            if response != crate::mcp_interactions::interaction_response(&record)?
                || Some(canonical_hash(&response)) != record.view.response_sha256
            {
                return Err(Error::Forbidden);
            }
            let receipt: Value =
                parse(&row.try_get::<String, _>("receipt").map_err(storage_error)?)?;
            if receipt["interaction_id"] != interaction_after
                || receipt["task_record_id"] != after
                || receipt["operation_id"] != record.operation_id
                || receipt["response_sha256"] != canonical_hash(&response)
                || receipt["state"] != "responded"
                || !receipt["transport_receipt"].is_object()
            {
                return Err(Error::UncertainEffect);
            }
            let mut decisions_after = String::new();
            loop {
                let decision=sqlx::query("SELECT * FROM mcp_task_interaction_decisions WHERE interaction_id=$1 AND request_key>$2 ORDER BY request_key LIMIT 1").bind(&interaction_after).bind(&decisions_after).fetch_optional(&mut **tx).await.map_err(storage_error)?;
                let Some(decision) = decision else { break };
                decisions_after = decision.try_get("request_key").map_err(storage_error)?;
                let expected = scope(
                    &task,
                    &format!(
                        "decision:{}",
                        canonical_hash(&json!([interaction_after, decisions_after]))
                    ),
                    1,
                    "task_interaction_decision",
                );
                if parse::<McpSecretScope>(
                    &decision
                        .try_get::<String, _>("secret_scope")
                        .map_err(storage_error)?,
                )? != expected
                {
                    return Err(Error::Forbidden);
                }
                let envelope = open(
                    keys,
                    &expected,
                    &decision
                        .try_get::<String, _>("sealed_decision")
                        .map_err(storage_error)?,
                )?;
                let actor: Value = parse(
                    &decision
                        .try_get::<String, _>("original_actor")
                        .map_err(storage_error)?,
                )?;
                let key: KeyId = parse(
                    &decision
                        .try_get::<String, _>("request_key_id")
                        .map_err(storage_error)?,
                )?;
                let material = keys.load(&key).map_err(|_| Error::NativeKeyUnavailable)?;
                let digest = ring::hmac::sign(
                    &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, material.as_ref()),
                    json_string(&json!([run, interaction_after, envelope["answer"], actor]))?
                        .as_bytes(),
                );
                let digest = digest
                    .as_ref()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>();
                if envelope["actor"] != actor
                    || envelope["operation_id"] != record.operation_id
                    || envelope["request_sha256"] != record.view.request_sha256
                    || digest
                        != decision
                            .try_get::<String, _>("request_sha256")
                            .map_err(storage_error)?
                    || envelope["answer"]["idempotency_key"] != decisions_after
                    || envelope["answer"]["expected_revision"].as_i64()
                        != Some(
                            decision
                                .try_get::<i64, _>("expected_revision")
                                .map_err(storage_error)?,
                        )
                {
                    return Err(Error::Forbidden);
                }
                chain(
                    &mut receipts,
                    &json!([
                        interaction_after,
                        decisions_after,
                        canonical_hash(&envelope),
                        digest,
                        parse::<Value>(
                            &decision
                                .try_get::<String, _>("receipt")
                                .map_err(storage_error)?
                        )?
                    ]),
                );
            }
            if let Some(intent_raw) = row
                .try_get::<Option<String>, _>("sealed_model_intent")
                .map_err(storage_error)?
            {
                let expected = scope(
                    &task,
                    &interaction_after,
                    1,
                    "task_interaction_model_intent",
                );
                if parse::<McpSecretScope>(
                    &row.try_get::<String, _>("model_intent_scope")
                        .map_err(storage_error)?,
                )? != expected
                {
                    return Err(Error::Forbidden);
                }
                let intent = open(keys, &expected, &intent_raw)?;
                if canonical_hash(&intent)
                    != row
                        .try_get::<String, _>("model_request_sha256")
                        .map_err(storage_error)?
                    || intent["task_operation_id"] != record.operation_id
                {
                    return Err(Error::Forbidden);
                }
                let generated = record
                    .view
                    .generated_response
                    .as_ref()
                    .ok_or(Error::UncertainEffect)?;
                if generated["wire_evidence"]["completed_stream"] != true
                    || !generated["tools"].as_array().is_some_and(Vec::is_empty)
                    || generated["result"]["content"]["type"] != "text"
                    || !generated["result"]["content"]["text"].is_string()
                {
                    return Err(Error::UncertainEffect);
                }
                let claim = sqlx::query(
                    "SELECT * FROM mcp_task_sampling_claims WHERE interaction_id=$1 AND run_id=$2",
                )
                .bind(&interaction_after)
                .bind(&run)
                .fetch_one(&mut **tx)
                .await
                .map_err(storage_error)?;
                let node = if intent["workflow_node"].is_null() {
                    String::new()
                } else {
                    canonical_hash(&intent["workflow_node"])
                };
                if claim.try_get::<String, _>("state").map_err(storage_error)? != "received"
                    || claim
                        .try_get::<String, _>("node_scope_sha256")
                        .map_err(storage_error)?
                        != node
                    || claim
                        .try_get::<String, _>("original_checkpoint_sha256")
                        .map_err(storage_error)?
                        != intent["parent_checkpoint_sha256"]
                            .as_str()
                            .ok_or(Error::Forbidden)?
                    || claim
                        .try_get::<String, _>("original_request_sha256")
                        .map_err(storage_error)?
                        != canonical_hash(&intent)
                    || claim
                        .try_get::<String, _>("original_response_sha256")
                        .map_err(storage_error)?
                        != canonical_hash(generated)
                    || generated["intent_sha256"] != canonical_hash(&intent)
                {
                    return Err(Error::UncertainEffect);
                }
                chain(
                    &mut intents,
                    &json!([interaction_after, canonical_hash(&intent), node]),
                );
                chain(
                    &mut receipts,
                    &json!([
                        interaction_after,
                        canonical_hash(generated),
                        claim
                            .try_get::<i64, _>("charged_tokens")
                            .map_err(storage_error)?
                    ]),
                );
            }
            chain(
                &mut intents,
                &json!([
                    interaction_after,
                    record.operation_id,
                    record.authority_sha256,
                    record.view.request_sha256,
                    canonical_hash(&record.view.original_request)
                ]),
            );
            chain(
                &mut receipts,
                &json!([interaction_after, canonical_hash(&response), receipt]),
            );
        }
    }
    Ok((intents, receipts))
}
