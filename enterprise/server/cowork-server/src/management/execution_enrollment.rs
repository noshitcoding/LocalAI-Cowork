//! Explicit, receipt-driven enrollment; preflight observations cannot activate it.
use super::*;
use cowork_contracts::enterprise_execution::*;

static ENABLED: OnceLock<bool> = OnceLock::new();
pub(super) fn initialize() -> anyhow::Result<()> {
    ENABLED
        .set(
            std::env::var("COWORK_MANAGEMENT_EXECUTION_ENROLLMENT_ENABLED")
                .unwrap_or_else(|_| "false".into())
                .parse()?,
        )
        .map_err(|_| anyhow::anyhow!("enrollment configuration already initialized"))
}
pub(super) fn enabled() -> bool {
    ENABLED.get().copied().unwrap_or(false)
}
pub(super) fn requested(action: &Value) -> bool {
    action == "enroll_execution"
}
pub(super) fn operator_contract(action: &Value, version: Option<u32>) -> Result<(), ApiError> {
    if requested(action) && version != Some(1) {
        return Err(ApiError::Unavailable(
            "enrollment requires provisioner enrollment contract version 1".into(),
        ));
    }
    Ok(())
}
pub(super) fn valid_instance(instance: &Value) -> bool {
    enabled()
        && instance["execution_required"] == false
        && instance["state"] == "stopped"
        && instance["backend"] == "compose"
        && instance["policy"]["minimum_isolation"] == "container"
}
pub(super) fn validate(instance: &Value, request: &ActionRequest) -> Result<(), ApiError> {
    validate_profiled(instance, request, false)
}
pub(super) fn validate_profiled(
    instance: &Value,
    request: &ActionRequest,
    profiled: bool,
) -> Result<(), ApiError> {
    if request.action != InstanceAction::EnrollExecution {
        if request.acknowledge_execution_enrollment {
            return Err(invalid("enrollment acknowledgement requires enrollment"));
        }
        return Ok(());
    }
    let allowed = valid_instance(instance)
        || (profiled
            && enabled()
            && instance["execution_required"] == false
            && instance["state"] == "stopped");
    if !allowed
        || !request.acknowledge_execution_enrollment
        || request.backup_id.is_some()
        || request.release.is_some()
        || request.rollback_operation_id.is_some()
        || request.qualification_only
        || request.confirmation_name.is_some()
        || request.purge_backups
        || request.acknowledge_execution_stop
        || !request.reconcile_reservation_ids.is_empty()
    {
        return Err(invalid("enrollment requires an acknowledged, stopped legacy Compose instance and a fresh operator backup"));
    }
    Ok(())
}
fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    row: &sqlx::postgres::PgRow,
    succeeded: bool,
    result: &Value,
) -> Result<(), ApiError> {
    if row.try_get::<String, _>("action")? != "enroll_execution" {
        if result.get("execution_enrollment").is_some() {
            return Err(invalid("unexpected execution enrollment receipt"));
        }
        return Ok(());
    }
    let binding: ExecutionGenerationBinding =
        serde_json::from_value(row.try_get::<Value, _>("execution_binding")?)?;
    let proof: ExecutionEnrollmentReceipt =
        serde_json::from_value(result["execution_enrollment"].clone())
            .map_err(|_| invalid("verified enrollment receipt required"))?;
    if !row.try_get::<bool, _>("execution_runtime_required")?
        || proof.binding != binding
        || result["receipt_id"] != json!(binding.operation_id)
        || !sha(&proof.source_manifest_sha256)
        || result["instance_state"] != "stopped"
    {
        return Err(invalid("enrollment receipt does not match this operation"));
    }
    let enrolled = proof.outcome == ExecutionEnrollmentOutcome::Enrolled;
    if enrolled != succeeded
        || (enrolled
            && (proof.verified_generations == 0
                || !proof.inventory_sha256.as_deref().is_some_and(sha)
                || result["backup_id"] != json!(binding.operation_id)
                || result["execution"]["runtime"].is_null()))
        || (!enrolled
            && (proof.verified_generations != 0
                || proof.inventory_sha256.is_some()
                || result["error_code"] != "execution_enrollment_precheck_failed"
                || !result["execution"]["runtime"].is_null()))
    {
        return Err(invalid("unproven enrollment must remain in reconciliation"));
    }
    let current: bool = sqlx::query_scalar(
        "SELECT execution_required FROM management_instances WHERE id=$1 FOR UPDATE",
    )
    .bind(binding.instance_id)
    .fetch_one(&mut **tx)
    .await?;
    if current {
        return Err(ApiError::Conflict("instance is already enrolled".into()));
    }
    if enrolled {
        sqlx::query("UPDATE management_instances SET execution_required=true,execution_enrollment=$2 WHERE id=$1")
            .bind(binding.instance_id).bind(&result["execution_enrollment"]).execute(&mut **tx).await?;
    }
    append_audit(tx,binding.organisation_id,row.try_get("actor_user_id")?,
        if enrolled {"instance.execution.enrolled"}else{"instance.execution.enrollment_unchanged"},binding.instance_id,
        json!({"operation_id":binding.operation_id,"receipt":proof,"backup_id":result["backup_id"]})).await?;
    Ok(())
}
