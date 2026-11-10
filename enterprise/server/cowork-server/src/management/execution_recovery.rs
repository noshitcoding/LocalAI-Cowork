//! Explicit recovery of server-only capacity after a proven whole-instance stop.
use super::*;
use cowork_contracts::enterprise_execution::*;

pub(super) fn requested(arguments: &Value) -> bool {
    arguments["reconcile_reservation_ids"]
        .as_array()
        .is_some_and(|v| !v.is_empty())
}

pub(super) async fn validate_request(
    tx: &mut Transaction<'_, Postgres>,
    instance: &Instance,
    request: &ActionRequest,
) -> Result<(), ApiError> {
    if request.reconcile_reservation_ids.is_empty() {
        if request.acknowledge_execution_stop {
            return Err(invalid(
                "execution stop acknowledgement requires selected reservations",
            ));
        }
        return Ok(());
    }
    let profile = backend_profiles::current(tx, instance.organisation_id, instance.id).await?;
    let backend_available = instance.backend == DeploymentBackend::Compose
        && instance.policy.minimum_isolation == Isolation::Container
        || backend_profiles::available(instance, InstanceAction::Stop, profile.as_ref());
    if !execution_admission::active()
        || request.action != InstanceAction::Stop
        || !request.acknowledge_execution_stop
        || request.backup_id.is_some()
        || request.release.is_some()
        || !backend_available
        || !matches!(
            instance.state,
            InstanceState::Ready | InstanceState::Stopped
        )
    {
        return Err(invalid("capacity recovery requires an acknowledged stop of a ready or stopped instance on its current bound backend"));
    }
    reservations(
        tx,
        instance.organisation_id,
        instance.id,
        &request.reconcile_reservation_ids,
        true,
    )
    .await?;
    Ok(())
}

async fn reservations(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    instance: Uuid,
    ids: &[Uuid],
    held_only: bool,
) -> Result<Vec<ExecutionRecoveryReservation>, ApiError> {
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    if ids.is_empty() || ids.len() > 100 || unique.len() != ids.len() {
        return Err(invalid("select 1 to 100 distinct reservations"));
    }
    let rows=sqlx::query("SELECT id,execution_epoch,run_id,target,state FROM management_execution_reservations WHERE organisation_id=$1 AND instance_id=$2 AND id=ANY($3) ORDER BY id")
        .bind(org).bind(instance).bind(ids).fetch_all(&mut **tx).await?;
    if rows.len() != ids.len() {
        return Err(denied());
    }
    rows.into_iter()
        .map(|row| {
            let target: cowork_contracts::ExecutorTarget =
                serde_json::from_value(row.try_get("target")?)?;
            if !matches!(
                target,
                cowork_contracts::ExecutorTarget::ServerLinux { pool_id: None }
            ) {
                return Err(invalid(
                    "remote device and VM capacity requires executor completion evidence",
                ));
            }
            if held_only && row.try_get::<String, _>("state")? != "held" {
                return Err(ApiError::Conflict(
                    "selected reservation is already settled".into(),
                ));
            }
            Ok(ExecutionRecoveryReservation {
                reservation_id: row.try_get("id")?,
                execution_epoch: row.try_get("execution_epoch")?,
                run_id: row.try_get("run_id")?,
            })
        })
        .collect()
}

pub(super) async fn prepare(
    tx: &mut Transaction<'_, Postgres>,
    operation: &mut Value,
) -> Result<(), ApiError> {
    if !requested(&operation["arguments"]) {
        return Ok(());
    }
    let generation: ExecutionGenerationBinding =
        serde_json::from_value(operation["execution_binding"].clone())?;
    let ids: Vec<Uuid> =
        serde_json::from_value(operation["arguments"]["reconcile_reservation_ids"].clone())?;
    let binding = ExecutionRecoveryBinding {
        reservations: reservations(
            tx,
            generation.organisation_id,
            generation.instance_id,
            &ids,
            false,
        )
        .await?,
        generation,
    };
    let value = serde_json::to_value(&binding)?;
    let changed=sqlx::query("UPDATE management_operations SET execution_recovery_binding=$2 WHERE id=$1 AND execution_recovery_binding IS NULL")
        .bind(binding.generation.operation_id).bind(&value).execute(&mut **tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict(
            "execution recovery binding already frozen".into(),
        ));
    }
    operation["execution_recovery_binding"] = value;
    Ok(())
}

pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    row: &sqlx::postgres::PgRow,
    succeeded: bool,
    result: &Value,
) -> Result<(), ApiError> {
    let binding: Option<Value> = row.try_get("execution_recovery_binding")?;
    let Some(binding) = binding else {
        if result.get("execution_recovery").is_some()
            || requested(&row.try_get::<Value, _>("arguments")?)
        {
            return Err(invalid("execution recovery has no frozen binding"));
        }
        return Ok(());
    };
    let binding: ExecutionRecoveryBinding = serde_json::from_value(binding)?;
    let receipt: ExecutionRecoveryReceipt =
        serde_json::from_value(result["execution_recovery"].clone())
            .map_err(|_| invalid("verified execution stop receipt required"))?;
    let generation = &binding.generation;
    if !succeeded
        || row.try_get::<String, _>("action")? != "stop"
        || result["receipt_id"] != json!(generation.operation_id)
        || generation.operation_id != row.try_get::<Uuid, _>("id")?
        || generation.instance_id != row.try_get::<Uuid, _>("instance_id")?
        || generation.organisation_id != row.try_get::<Uuid, _>("organisation_id")?
        || receipt.binding != binding
        || !receipt.stopped
        || receipt.verified_generations == 0
        || receipt.inventory_sha256.len() != 64
        || !receipt
            .inventory_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid(
            "unverified execution recovery must remain in reconciliation",
        ));
    }
    for reservation in &binding.reservations {
        let changed=sqlx::query("UPDATE management_execution_reservations SET state='released',released_at=now(),completion=$6,recovery_operation_id=$7 WHERE id=$1 AND organisation_id=$2 AND instance_id=$3 AND execution_epoch=$4 AND run_id=$5 AND state='held'")
            .bind(reservation.reservation_id).bind(generation.organisation_id).bind(generation.instance_id).bind(reservation.execution_epoch).bind(reservation.run_id)
            .bind(json!({"recovery_operation_id":generation.operation_id,"execution_stopped":true,"inventory_sha256":receipt.inventory_sha256}))
            .bind(generation.operation_id).execute(&mut **tx).await?.rows_affected();
        if changed == 1 {
            append_audit(tx,generation.organisation_id,row.try_get("actor_user_id")?,"execution.recover",generation.instance_id,
                json!({"reservation_id":reservation.reservation_id,"operation_id":generation.operation_id,"run_id":reservation.run_id,"execution_epoch":reservation.execution_epoch,"inventory_sha256":receipt.inventory_sha256})).await?;
        }
    }
    Ok(())
}
