//! Lifecycle fencing does not settle any outstanding execution reservation.
use super::*;
use cowork_contracts::enterprise_execution::{
    ExecutionGenerationBinding, ExecutionGenerationReceipt,
};

pub(super) async fn lock_organisation(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
        "SELECT id FROM management_organisations WHERE id=$1 AND archived_at IS NULL FOR UPDATE",
    )
    .bind(org)
    .fetch_one(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn prepare(
    tx: &mut Transaction<'_, Postgres>,
    operation: &mut Value,
    instance: &Value,
) -> Result<(), ApiError> {
    environments::instance_unlocked(tx, serde_json::from_value(instance["id"].clone())?).await?;
    if instance["execution_required"] != true
        && !execution_enrollment::requested(&operation["action"])
    {
        return Ok(());
    }
    let source_epoch = serde_json::from_value(instance["execution_epoch"].clone())?;
    let binding = ExecutionGenerationBinding {
        operation_id: serde_json::from_value(operation["id"].clone())?,
        organisation_id: serde_json::from_value(operation["organisation_id"].clone())?,
        instance_id: serde_json::from_value(instance["id"].clone())?,
        source_epoch,
        candidate_epoch: if matches!(
            operation["action"].as_str(),
            Some("restore" | "upgrade" | "rollback" | "import_restore")
        ) {
            if operation["action"] == "import_restore" {
                serde_json::from_value(
                    operation["arguments"]["import_binding"]["candidate_epoch"].clone(),
                )?
            } else {
                Uuid::new_v4()
            }
        } else {
            source_epoch
        },
    };
    let value = serde_json::to_value(&binding)?;
    let changed = sqlx::query("UPDATE management_operations SET execution_binding=$2,execution_runtime_required=true WHERE id=$1 AND execution_binding IS NULL")
        .bind(binding.operation_id).bind(&value).execute(&mut **tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict(
            "operation execution binding is already frozen".into(),
        ));
    }
    operation["execution_binding"] = value;
    operation["execution_runtime_required"] = json!(true);
    Ok(())
}

pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    row: &sqlx::postgres::PgRow,
    succeeded: bool,
    recovered: bool,
    result: &Value,
) -> Result<(), ApiError> {
    let binding: Option<Value> = row.try_get("execution_binding")?;
    let Some(binding) = binding else {
        if result.get("execution").is_some() {
            return Err(invalid("operation has no execution generation binding"));
        }
        return Ok(());
    };
    let binding: ExecutionGenerationBinding = serde_json::from_value(binding)?;
    let receipt: ExecutionGenerationReceipt =
        serde_json::from_value(result["execution"].clone())
            .map_err(|_| invalid("verified execution generation receipt is required"))?;
    if receipt.binding != binding
        || binding.operation_id != row.try_get::<Uuid, _>("id")?
        || binding.instance_id != row.try_get::<Uuid, _>("instance_id")?
        || binding.organisation_id != row.try_get::<Uuid, _>("organisation_id")?
    {
        return Err(invalid(
            "execution generation receipt does not match this operation",
        ));
    }
    let action: String = row.try_get("action")?;
    let arguments: Value = row.try_get("arguments")?;
    let promotes = succeeded
        && (matches!(action.as_str(), "restore" | "rollback" | "import_restore")
            || (action == "upgrade" && arguments["qualification_only"] != true));
    let expected = if promotes {
        binding.candidate_epoch
    } else {
        binding.source_epoch
    };
    // An interrupted restore may already have atomically switched its pointer.
    // Explicit stopping recovery can attest either exact known generation; it
    // cannot invent a third generation or re-run the restore.
    let recovered_restore =
        recovered && action == "restore" && receipt.active_epoch == binding.candidate_epoch;
    if receipt.active_epoch != expected && !recovered_restore {
        return Err(invalid(
            "receipt reports an unexpected active execution generation",
        ));
    }
    let changed = sqlx::query(
        "UPDATE management_instances SET execution_epoch=$3 WHERE id=$1 AND execution_epoch=$2",
    )
    .bind(binding.instance_id)
    .bind(binding.source_epoch)
    .bind(receipt.active_epoch)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict(
            "execution generation changed during the operation".into(),
        ));
    }
    if receipt.active_epoch != binding.source_epoch {
        append_audit(tx,binding.organisation_id,row.try_get("actor_user_id")?,"instance.execution.epoch",binding.instance_id,
            json!({"operation_id":binding.operation_id,"source_epoch":binding.source_epoch,"active_epoch":receipt.active_epoch})).await?;
    }
    Ok(())
}
