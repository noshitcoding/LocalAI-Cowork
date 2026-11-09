//! Permanent deletion is opt-in, acknowledged by name, and receipt-driven.
use super::*;
use std::sync::OnceLock;

static ENABLED: OnceLock<bool> = OnceLock::new();
pub(super) fn initialize() -> anyhow::Result<()> {
    ENABLED
        .set(
            std::env::var("COWORK_MANAGEMENT_DELETION_ENABLED")
                .unwrap_or_else(|_| "false".into())
                .parse()?,
        )
        .map_err(|_| anyhow::anyhow!("deletion configuration already initialized"))
}
pub(super) fn enabled() -> bool {
    ENABLED.get().copied().unwrap_or(false)
}

pub(super) async fn arguments(
    tx: &mut Transaction<'_, Postgres>,
    instance: &Instance,
    request: &ActionRequest,
    arguments: &mut Value,
) -> Result<(), ApiError> {
    if request.action != InstanceAction::Delete {
        if request.confirmation_name.is_some() || request.purge_backups {
            return Err(invalid(
                "deletion acknowledgement is only valid for deletion",
            ));
        }
        return Ok(());
    }
    if request.confirmation_name.as_deref() != Some(instance.name.as_str())
        || !request.purge_backups
    {
        return Err(invalid("permanent deletion requires the exact instance name and acknowledgement that all managed backups will be purged"));
    }
    if request.backup_id.is_some()
        || request.release.is_some()
        || request.rollback_operation_id.is_some()
        || request.qualification_only
    {
        return Err(invalid("deletion cannot select a backup or release"));
    }
    if execution_admission::active() && sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM management_execution_reservations WHERE instance_id=$1 AND state='held')")
        .bind(instance.id).fetch_one(&mut **tx).await? {
        return Err(ApiError::Conflict("settle outstanding executions before deleting their instance".into()));
    }
    let backups:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM management_backups WHERE organisation_id=$1 AND instance_id=$2 AND purged_at IS NULL ORDER BY id")
        .bind(instance.organisation_id).bind(instance.id).fetch_all(&mut **tx).await?;
    arguments["delete_backup_ids"] = json!(backups);
    Ok(())
}
pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    operation: Uuid,
    instance: Uuid,
    arguments: &Value,
    result: &Value,
) -> Result<(), ApiError> {
    let digest = result["deletion_inventory_sha256"].as_str().unwrap_or("");
    let expected = arguments["delete_backup_ids"]
        .as_array()
        .ok_or_else(|| invalid("deletion backup inventory missing"))?
        .len();
    if result["receipt_id"] != json!(operation)
        || result["instance_id"] != json!(instance)
        || result["deletion_verified"] != true
        || result["deleted_backup_count"]
            .as_u64()
            .is_none_or(|n| n < expected as u64)
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid(
            "permanent deletion requires verified scoped resource and backup removal",
        ));
    }
    sqlx::query(
        "UPDATE management_backups SET purged_at=now() WHERE instance_id=$1 AND purged_at IS NULL",
    )
    .bind(instance)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
