//! Read an original decision/stop receipt without resending any operation.
use super::source::*;
use super::*;
use crate::mcp_interactions::record;
use crate::mcp_tasks::lock_task;
use cowork_secrets::{KeyStore, McpSecretScope, SealedMcpBlob, open_mcp_blob};

impl Store {
    pub async fn offline_mcp_original_receipt(
        &self,
        run: Uuid,
        id: &str,
        interaction: bool,
        input: &McpAsyncTaskRequest,
        keys: &dyn KeyStore,
    ) -> Result<Option<Value>> {
        if !self.is_local_sqlite()
            || input.expected_revision <= 0
            || input.expected_revision == i64::MAX
            || input.idempotency_key.is_empty()
            || input.idempotency_key.len() > 128
            || input.idempotency_key.chars().any(char::is_control)
        {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let receipt = if interaction {
            let r = record(&mut tx, id, keys).await?;
            if r.task.view.run_id != run.to_string() {
                return Err(Error::Forbidden);
            }
            let p = task_source(&mut tx, &r.task, keys, false).await?;
            let row = sqlx::query("SELECT * FROM mcp_task_interaction_decisions WHERE interaction_id=$1 AND request_key=$2")
                .bind(id).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            if let Some(row) = row {
                if row
                    .try_get::<i64, _>("expected_revision")
                    .map_err(storage_error)?
                    != input.expected_revision
                {
                    return Err(Error::Conflict);
                }
                let mut expected = r.task.scope.clone();
                expected.record_id = format!(
                    "decision:{}",
                    canonical_hash(&json!([id, input.idempotency_key]))
                );
                expected.revision = 1;
                expected.purpose = "task_interaction_decision".into();
                let scope: McpSecretScope = parse(
                    &row.try_get::<String, _>("secret_scope")
                        .map_err(storage_error)?,
                )?;
                if scope != expected {
                    return Err(Error::Forbidden);
                }
                let blob: SealedMcpBlob = parse(
                    &row.try_get::<String, _>("sealed_decision")
                        .map_err(storage_error)?,
                )?;
                let envelope: Value = serde_json::from_slice(
                    &open_mcp_blob(keys, &scope, &blob).map_err(|_| Error::NativeKeyUnavailable)?,
                )
                .map_err(storage_error)?;
                let original: AnswerMcpTaskInteraction =
                    serde_json::from_value(envelope["answer"].clone()).map_err(storage_error)?;
                let actor = proof(&p);
                if original.idempotency_key != input.idempotency_key
                    || original.expected_revision != input.expected_revision
                    || envelope["actor"] != actor
                    || parse::<Value>(
                        &row.try_get::<String, _>("original_actor")
                            .map_err(storage_error)?,
                    )? != actor
                    || envelope["operation_id"] != r.operation_id
                    || envelope["request_sha256"] != r.view.request_sha256
                {
                    return Err(Error::Forbidden);
                }
                let state = match original.action.as_str() {
                    "accept" | "approve_sampling" | "release_sample" => "approved",
                    "decline" | "cancel" | "decline_sample" => "declined",
                    _ => return Err(Error::Forbidden),
                };
                let expected_receipt = json!({"interaction_id":id,"task_record_id":r.task.view.id,
                    "revision":input.expected_revision+1,"state":state,"decision":original.action,
                    "request_sha256":r.view.request_sha256,"effects_unknown":true});
                let receipt: Value =
                    parse(&row.try_get::<String, _>("receipt").map_err(storage_error)?)?;
                if receipt != expected_receipt {
                    return Err(Error::Forbidden);
                }
                Some(receipt)
            } else {
                None
            }
        } else {
            let task = lock_task(&mut tx, id).await?;
            if task.view.run_id != run.to_string() {
                return Err(Error::Forbidden);
            }
            let p = task_source(&mut tx, &task, keys, false).await?;
            let row = sqlx::query("SELECT expected_revision,original_actor,receipt FROM mcp_task_control_requests WHERE task_record_id=$1 AND request_key=$2 AND operation='cancel'")
                .bind(id).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            if let Some(row) = row {
                if row
                    .try_get::<i64, _>("expected_revision")
                    .map_err(storage_error)?
                    != input.expected_revision
                {
                    return Err(Error::Conflict);
                }
                if parse::<Value>(
                    &row.try_get::<String, _>("original_actor")
                        .map_err(storage_error)?,
                )? != proof(&p)
                {
                    return Err(Error::Forbidden);
                }
                let receipt: Value =
                    parse(&row.try_get::<String, _>("receipt").map_err(storage_error)?)?;
                if receipt
                    != json!({"task_record_id":id,"expected_revision":input.expected_revision,
                    "state":"requested","effects_unknown":true})
                {
                    return Err(Error::Forbidden);
                }
                Some(receipt)
            } else {
                None
            }
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
