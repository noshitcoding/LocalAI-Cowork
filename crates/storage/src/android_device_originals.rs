//! Immutable create acknowledgements. Reading one never creates execution authority.
use super::*;
use crate::android_device_runs::{device_run_permit, device_run_row};
use crate::android_devices::{device_invalid, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction};
use team_service::identity_error;

const MAX_RECEIPT_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn check_original_key(key: &str, digest: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > 200
        || key.chars().any(char::is_control)
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(device_invalid());
    }
    Ok(())
}

impl Store {
    /// A fresh authenticated read of the original response, including after later run updates.
    pub async fn android_device_original_receipt(
        &self,
        actor: &Principal,
        project: Uuid,
        operation_key: &str,
        request_sha256: &str,
    ) -> Result<Option<Value>> {
        check_original_key(operation_key, request_sha256)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        // Mutators take this same original identity/device writer fence. Revalidate after waiting.
        device_writer(&mut tx).await?;
        let value = self
            .android_device_original_in_transaction(
                &mut tx,
                actor,
                project,
                operation_key,
                request_sha256,
            )
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }

    pub(crate) async fn android_device_original_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        project: Uuid,
        operation_key: &str,
        request_sha256: &str,
    ) -> Result<Option<Value>> {
        check_original_key(operation_key, request_sha256)?;
        self.identity()
            .validate_principal_in_transaction(tx, actor)
            .await
            .map_err(identity_error)?;
        let row = sqlx::query("SELECT project_id,run_id,request_sha256,receipt,receipt_sha256,created_at FROM android_device_original_receipts WHERE owner_id=$1 AND operation_key=$2")
            .bind(actor.identity_id().to_string())
            .bind(operation_key)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
        let Some(row) = row else {
            self.identity()
                .validate_principal_in_transaction(tx, actor)
                .await
                .map_err(identity_error)?;
            return Ok(None);
        };
        if row
            .try_get::<String, _>("project_id")
            .map_err(storage_error)?
            != project.to_string()
        {
            return Err(Error::Forbidden);
        }
        let run = Uuid::parse_str(&row.try_get::<String, _>("run_id").map_err(storage_error)?)
            .map_err(storage_error)?;
        let original_run = device_run_row(tx, run).await?;
        if original_run
            .try_get::<String, _>("creator_id")
            .map_err(storage_error)?
            != actor.identity_id().to_string()
            || original_run
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
                != project.to_string()
            || original_run
                .try_get::<String, _>("creation_key")
                .map_err(storage_error)?
                != operation_key
            || original_run
                .try_get::<i64, _>("created_at")
                .map_err(storage_error)?
                != row.try_get::<i64, _>("created_at").map_err(storage_error)?
        {
            return Err(Error::Conflict);
        }
        device_run_permit(self, tx, actor, &original_run, ContentAction::Read).await?;
        if row
            .try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != request_sha256
        {
            return Err(Error::Conflict);
        }
        let encoded = row.try_get::<String, _>("receipt").map_err(storage_error)?;
        if encoded.len() > MAX_RECEIPT_BYTES
            || hash(encoded.as_bytes())
                != row
                    .try_get::<String, _>("receipt_sha256")
                    .map_err(storage_error)?
        {
            return Err(Error::Conflict);
        }
        let value: Value = parse(&encoded)?;
        validate_receipt(&value, actor.identity_id(), project, run, operation_key)?;
        if value["spec"]["thread_id"]
            != original_run
                .try_get::<String, _>("order_id")
                .map_err(storage_error)?
            || value["assigned_executor_id"]
                != original_run
                    .try_get::<String, _>("executor_id")
                    .map_err(storage_error)?
            || value["spec"]["project"]["revision"]
                != original_run
                    .try_get::<i64, _>("project_revision")
                    .map_err(storage_error)?
            || value["spec"]["project_privacy"]
                != if original_run
                    .try_get::<Option<String>, _>("team_id")
                    .map_err(storage_error)?
                    .is_some()
                {
                    "team_managed"
                } else {
                    "private_local"
                }
        {
            return Err(Error::Conflict);
        }
        // Read authority is current; original execute/device/project revisions are deliberately
        // not substituted into this acknowledgement or reused for dispatch.
        device_run_permit(self, tx, actor, &original_run, ContentAction::Read).await?;
        self.identity()
            .validate_principal_in_transaction(tx, actor)
            .await
            .map_err(identity_error)?;
        Ok(Some(value))
    }
}

fn validate_receipt(value: &Value, owner: Uuid, project: Uuid, run: Uuid, key: &str) -> Result<()> {
    if value["spec"]["id"] != json!(run)
        || value["spec"]["creator_user_id"] != json!(owner)
        || value["spec"]["project_id"] != json!(project)
        || value["spec"]["idempotency_key"] != key
        || value["revision"] != 1
        || value["etag"] != format!("{run}:1")
        || !matches!(
            value["state"].as_str(),
            Some("queued" | "waiting_for_snapshot")
        )
        || !value["result"].is_null()
        || !value["error"].is_null()
        || !value["started_at"].is_null()
        || !value["finished_at"].is_null()
        || !value["lease_expires_at"].is_null()
    {
        return Err(Error::Conflict);
    }
    Ok(())
}

/// Called exclusively by a fresh create transaction, never by old-row reconciliation.
#[expect(
    clippy::too_many_arguments,
    reason = "record_original: Keep owner/project/run, original request key/hash and durable response timestamp explicit"
)]
pub(crate) async fn record_original(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    run: Uuid,
    key: &str,
    request_sha256: &str,
    value: &Value,
    current: i64,
) -> Result<()> {
    check_original_key(key, request_sha256)?;
    validate_receipt(value, owner, project, run, key)?;
    let encoded = json_string(value)?;
    if encoded.len() > MAX_RECEIPT_BYTES {
        return Err(Error::BudgetExceeded);
    }
    let digest = hash(encoded.as_bytes());
    sqlx::query("INSERT INTO android_device_original_receipts(owner_id,operation_key,project_id,run_id,request_sha256,receipt,receipt_sha256,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(owner.to_string())
        .bind(key)
        .bind(project.to_string())
        .bind(run.to_string())
        .bind(request_sha256)
        .bind(encoded)
        .bind(digest)
        .bind(current)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    Ok(())
}
