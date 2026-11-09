//! A rollback selects a completed promotion and its verified pre-update backup.
use super::*;

pub(super) async fn point(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    instance: Uuid,
) -> Result<Option<InstanceRollback>, ApiError> {
    // A later restore/promotion/rollback invalidates the old generation's route
    // back. Tests, ordinary backups and starts do not replace its generation.
    let value: Option<Value> = sqlx::query_scalar("WITH latest AS (SELECT * FROM management_operations WHERE organisation_id=$1 AND instance_id=$2 AND state='succeeded' AND (action IN ('restore','rollback','provision') OR (action='upgrade' AND result->'qualification_only'='false'::jsonb)) ORDER BY created_at DESC,id DESC LIMIT 1) SELECT jsonb_build_object('operation_id',o.id,'backup_id',b.id,'source_release',i.current_release,'release',o.result->>'source_release','created_at',b.created_at) FROM latest o JOIN management_instances i ON i.id=o.instance_id JOIN management_backups b ON b.id=o.id AND b.instance_id=i.id AND b.organisation_id=i.organisation_id WHERE o.action='upgrade' AND o.result->>'release'=i.current_release AND o.result->>'generation'=o.id::text AND NOT EXISTS(SELECT 1 FROM management_releases r WHERE r.id=o.result->>'source_release' AND NOT r.active)")
        .bind(org).bind(instance).fetch_optional(&mut **tx).await?;
    if let Some(point) = &value {
        let profile = backend_profiles::current(tx, org, instance).await?;
        let original: Option<Value> =
            sqlx::query_scalar("SELECT backend_profile FROM management_operations WHERE id=$1")
                .bind(serde_json::from_value::<Uuid>(
                    point["operation_id"].clone(),
                )?)
                .fetch_one(&mut **tx)
                .await?;
        if original != profile {
            return Ok(None);
        }
    }
    value
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}

pub(super) async fn instance_rollbacks(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<InstanceRollback>>, ApiError> {
    instance_access(&state, &principal, org, id, false).await?;
    let mut tx = state.pool.begin().await?;
    let result = point(&mut tx, org, id).await?.into_iter().collect();
    tx.commit().await?;
    Ok(Json(result))
}

pub(super) async fn validate(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    instance: Uuid,
    arguments: &Value,
) -> Result<bool, ApiError> {
    Ok(point(tx, org, instance).await?.is_some_and(|p| {
        arguments["rollback_operation_id"] == json!(p.operation_id)
            && arguments["source_release"] == p.source_release
            && arguments["release"] == p.release
            && arguments["backup_id"] == json!(p.backup_id)
    }))
}

pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    instance: Uuid,
    arguments: &Value,
    result: &Value,
) -> Result<(), ApiError> {
    let source: Option<String> = sqlx::query_scalar(
        "SELECT current_release FROM management_instances WHERE id=$1 FOR UPDATE",
    )
    .bind(instance)
    .fetch_one(&mut **tx)
    .await?;
    if result["receipt_id"] != json!(id)
        || result["instance_id"] != json!(instance)
        || result["source_release"] != arguments["source_release"]
        || result["source_release"].as_str() != source.as_deref()
        || result["release"] != arguments["release"]
        || result["rollback_operation_id"] != arguments["rollback_operation_id"]
        || result["restored_backup_id"] != arguments["backup_id"]
        || result["generation"] != json!(id)
        || result["instance_state"] != "stopped"
        || !result["qualification_only"].is_null()
        || !result["rollback_verified"].is_null()
    {
        return Err(invalid(
            "rollback receipt does not match its requested transition",
        ));
    }
    sqlx::query("UPDATE management_instances SET current_release=$2 WHERE id=$1")
        .bind(instance)
        .bind(
            arguments["release"]
                .as_str()
                .ok_or_else(|| invalid("missing rollback release"))?,
        )
        .execute(&mut **tx)
        .await?;
    Ok(())
}
