//! Durable two-instance import. Unknown I/O retains both locks and allocations.
use super::*;
use cowork_contracts::enterprise_import::*;
#[path = "instance_import_authority.rs"]
mod authority;
use authority::ImportAuthority;

fn digest(value: &Value) -> Result<String, ApiError> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}
fn prepared_receipt(id: Uuid, operation: Uuid) -> Value {
    json!({"id":id,"phase":"prepared","revision":1,"operation_id":operation})
}
pub(super) fn requested(action: &Value) -> bool {
    action.as_str().and_then(ImportStep::from_action).is_some()
}
pub(super) async fn unlocked(
    tx: &mut Transaction<'_, Postgres>,
    instance: Uuid,
) -> Result<(), ApiError> {
    let locked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM management_instance_import_locks WHERE instance_id=$1)",
    )
    .bind(instance)
    .fetch_one(&mut **tx)
    .await?;
    if locked {
        return Err(ApiError::Conflict(
            "instance is retained by an unfinished or committed import".into(),
        ));
    }
    Ok(())
}
async fn admin(tx: &mut Transaction<'_, Postgres>, org: Uuid, user: Uuid) -> Result<(), ApiError> {
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u LEFT JOIN management_members m ON m.user_id=u.id AND m.organisation_id=$1 WHERE u.id=$2 AND u.deleted_at IS NULL AND (u.platform_admin OR m.role='admin'))")
        .bind(org).bind(user).fetch_one(&mut **tx).await?;
    if !allowed {
        return Err(denied());
    }
    Ok(())
}
async fn locks(tx: &mut Transaction<'_, Postgres>, org: Uuid) -> Result<(), ApiError> {
    rollouts::lock(tx).await?;
    execution_admission::lock_global(tx).await?;
    execution_generation::lock_organisation(tx, org).await
}
async fn host_binding(
    tx: &mut Transaction<'_, Postgres>,
    instance: Uuid,
) -> Result<Value, ApiError> {
    Ok(sqlx::query_scalar::<_, Option<Value>>("SELECT (SELECT jsonb_build_object('version',1,'host_id',h.id,'trust_domain',h.trust_domain,'daemon_id',h.daemon_id,'operator_state_id',h.operator_state_id) FROM management_executor_hosts h WHERE h.id=i.executor_host_id AND h.enabled) FROM management_instances i WHERE i.id=$1")
        .bind(instance).fetch_one(&mut **tx).await?.unwrap_or(Value::Null))
}
async fn allocation(tx: &mut Transaction<'_, Postgres>, instance: Uuid) -> Result<Value, ApiError> {
    Ok(sqlx::query_scalar::<_, Value>("SELECT jsonb_build_object('host_id',host_id,'revision',revision,'plan_sha256',plan_sha256,'plan',plan) FROM management_host_allocations WHERE instance_id=$1 AND state<>'released'")
        .bind(instance).fetch_optional(&mut **tx).await?.unwrap_or(Value::Null))
}
async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    binding: &InstanceImportBinding,
    actor: Uuid,
    step: ImportStep,
    export: Option<Value>,
) -> Result<Uuid, ApiError> {
    let id = Uuid::new_v4();
    let instance: Uuid = serde_json::from_value(if step.target() {
        binding.target["id"].clone()
    } else {
        binding.source["id"].clone()
    })?;
    let frozen = serde_json::to_value(binding)?;
    let source_stop:Option<Value>=sqlx::query_scalar("SELECT receipt FROM management_instance_import_receipts WHERE import_id=$1 AND step='quiesce'")
        .bind(binding.import_id).fetch_optional(&mut **tx).await?;
    let arguments = json!({"import_binding":frozen,"import_binding_sha256":digest(&serde_json::to_value(binding)?)?,"import_export":export,"import_source_stop":source_stop});
    sqlx::query("INSERT INTO management_operations(id,organisation_id,instance_id,actor_user_id,action,arguments,idempotency_key,request_digest,instance_import_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(id).bind(binding.organisation_id).bind(instance).bind(actor).bind(step.action()).bind(&arguments)
        .bind(format!("import:{}:{}", binding.import_id, step.action())).bind(digest(&arguments)?).bind(binding.import_id)
        .execute(&mut **tx).await?;
    Ok(id)
}

pub(super) async fn prepare(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<PrepareInstanceImport>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    if request.source_instance_id == request.target_instance_id
        || request.source_revision < 1
        || request.target_revision < 1
    {
        return Err(invalid(
            "select distinct unchanged source and fresh target instances",
        ));
    }
    let request_digest = digest(&json!({"organisation_id":org,"request":request}))?;
    let authority = ImportAuthority::from_request(&state, &principal, &headers).await?;
    let mut tx = state.pool.begin().await?;
    locks(&mut tx, org).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    authority.lock(&mut tx).await?;
    admin(&mut tx, org, principal.user_id).await?;
    if let Some((id, previous_digest, operation)) = sqlx::query_as::<_, (Uuid, String, Option<Uuid>)>("SELECT m.id,m.request_digest,(SELECT o.id FROM management_operations o WHERE o.instance_import_id=m.id AND o.organisation_id=m.organisation_id AND o.instance_id=m.source_instance_id AND o.action='import_quiesce') FROM management_instance_imports m WHERE actor_user_id=$1 AND idempotency_key=$2")
        .bind(principal.user_id).bind(request.idempotency_key).fetch_optional(&mut *tx).await? {
        if previous_digest != request_digest { return Err(ApiError::Conflict("import key reused for different bindings".into())); }
        let operation = operation.ok_or_else(|| ApiError::Conflict("original import prepare receipt is unavailable".into()))?;
        // A lost first response is replayed even after later phases advance. The
        // inspect endpoint supplies current state; prepare retains its first receipt.
        authority.validate(&mut tx).await?;
        return Ok(Json(prepared_receipt(id, operation)));
    }
    // Observe identities under the org lock, then take host/allocation locks before instance rows.
    let source: Value = sqlx::query_scalar(
        "SELECT to_jsonb(i) FROM management_instances i WHERE organisation_id=$1 AND id=$2",
    )
    .bind(org)
    .bind(request.source_instance_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    let target: Value = sqlx::query_scalar(
        "SELECT to_jsonb(i) FROM management_instances i WHERE organisation_id=$1 AND id=$2",
    )
    .bind(org)
    .bind(request.target_instance_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    if source["state"] != "stopped"
        || target["state"] != "stopped"
        || target["executor_host_id"].is_null()
        || source["backend"] != "compose"
        || target["backend"] != "compose"
        || target["execution_required"] != true
        || target["current_release"].is_null()
        || source["owner_user_id"] != target["owner_user_id"]
        || source["revision"] != request.source_revision
        || target["revision"] != request.target_revision
    {
        return Err(ApiError::Conflict("import requires stopped source and fresh already-host-bound enrolled target with unchanged revisions and owner".into()));
    }
    environments::instance_unlocked(&mut tx, request.source_instance_id).await?;
    environments::instance_unlocked(&mut tx, request.target_instance_id).await?;
    let epoch: Uuid = serde_json::from_value(target["execution_epoch"].clone())?;
    provisioning_releases::validate_selection(
        &mut tx,
        target["current_release"].as_str(),
        DeploymentBackend::Compose,
        &serde_json::from_value(target["policy"].clone())?,
        Some(serde_json::from_value(target["executor_host_id"].clone())?),
    )
    .await?;
    host_capacity::require_admission(
        &mut tx,
        org,
        serde_json::from_value(target["executor_host_id"].clone())?,
        request.target_instance_id,
        epoch,
        &serde_json::from_value(target["policy"].clone())?,
    )
    .await?;
    let rows: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM management_instances WHERE id=ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(vec![request.source_instance_id, request.target_instance_id])
    .fetch_all(&mut *tx)
    .await?;
    if rows.len() != 2 {
        return Err(denied());
    }
    for instance in rows {
        unlocked(&mut tx, instance).await?;
        if rollouts::reserved(&mut tx, instance).await? {
            return Err(ApiError::Conflict(
                "import instance is reserved by rollout".into(),
            ));
        }
        let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_operations WHERE instance_id=$1 AND state IN ('queued','running','reconciling'))")
            .bind(instance).fetch_one(&mut *tx).await?;
        if busy {
            return Err(ApiError::Conflict(
                "settle existing instance operation before import".into(),
            ));
        }
    }
    let used: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_operations WHERE instance_id=$1 AND (action NOT IN ('provision','stop') OR state<>'succeeded')) OR EXISTS(SELECT 1 FROM management_execution_reservations WHERE instance_id=$1)")
        .bind(request.target_instance_id).fetch_one(&mut *tx).await?;
    if used {
        return Err(invalid(
            "import target must be newly provisioned and never used for execution or restore",
        ));
    }
    let backup: Value = sqlx::query_scalar("SELECT to_jsonb(b) FROM management_backups b JOIN management_operations o ON o.id=b.operation_id WHERE b.id=$1 AND b.organisation_id=$2 AND b.instance_id=$3 AND b.purged_at IS NULL AND o.state='succeeded'")
        .bind(request.backup_id).bind(org).bind(request.source_instance_id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    let changed_since_backup: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_operations WHERE instance_id=$1 AND updated_at>$2 AND action<>'backup')")
        .bind(request.source_instance_id).bind(serde_json::from_value::<chrono::DateTime<chrono::Utc>>(backup["created_at"].clone())?)
        .fetch_one(&mut *tx).await?;
    if changed_since_backup {
        return Err(ApiError::Conflict(
            "capture a fresh stopped-source backup after its last lifecycle operation".into(),
        ));
    }
    let reservations = sqlx::query_scalar("SELECT id FROM management_execution_reservations WHERE instance_id=$1 AND state='held' ORDER BY id")
        .bind(request.source_instance_id).fetch_all(&mut *tx).await?;
    let binding = InstanceImportBinding {
        version: 1,
        import_id: Uuid::new_v4(),
        organisation_id: org,
        source_host_binding: host_binding(&mut tx, request.source_instance_id).await?,
        target_host_binding: host_binding(&mut tx, request.target_instance_id).await?,
        source_allocation: allocation(&mut tx, request.source_instance_id).await?,
        target_allocation: allocation(&mut tx, request.target_instance_id).await?,
        source_epoch: serde_json::from_value(source["execution_epoch"].clone())?,
        target_epoch: epoch,
        candidate_epoch: Uuid::new_v4(),
        source,
        target,
        backup,
        reservation_ids: reservations,
    };
    let value = serde_json::to_value(&binding)?;
    authority.validate(&mut tx).await?;
    sqlx::query("INSERT INTO management_instance_imports(id,organisation_id,source_instance_id,target_instance_id,actor_user_id,backup_id,idempotency_key,request_digest,binding,binding_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(binding.import_id).bind(org).bind(request.source_instance_id).bind(request.target_instance_id).bind(principal.user_id)
        .bind(request.backup_id).bind(request.idempotency_key).bind(request_digest).bind(&value).bind(digest(&value)?).execute(&mut *tx).await?;
    for instance in [request.source_instance_id, request.target_instance_id] {
        sqlx::query("INSERT INTO management_instance_import_locks(instance_id,organisation_id,import_id) VALUES($1,$2,$3)")
            .bind(instance).bind(org).bind(binding.import_id).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO management_instance_routes(organisation_id,source_instance_id,target_instance_id) VALUES($1,$2,$2) ON CONFLICT(source_instance_id) DO NOTHING")
        .bind(org).bind(request.source_instance_id).execute(&mut *tx).await?;
    let operation = enqueue(
        &mut tx,
        &binding,
        principal.user_id,
        ImportStep::Quiesce,
        None,
    )
    .await?;
    append_audit(&mut tx,org,principal.user_id,"instance.import.prepare",binding.import_id,
        json!({"source_instance_id":request.source_instance_id,"target_instance_id":request.target_instance_id,"backup_id":request.backup_id,"binding_sha256":digest(&value)?})).await?;
    authority.validate(&mut tx).await?;
    tx.commit().await?;
    Ok(Json(prepared_receipt(binding.import_id, operation)))
}

async fn current(
    tx: &mut Transaction<'_, Postgres>,
    binding: &InstanceImportBinding,
    phase: &str,
) -> Result<(), ApiError> {
    let source_id: Uuid = serde_json::from_value(binding.source["id"].clone())?;
    let target_id: Uuid = serde_json::from_value(binding.target["id"].clone())?;
    let catalog:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(b) FROM management_backups b WHERE id=$1 AND organisation_id=$2 AND instance_id=$3 AND purged_at IS NULL")
        .bind(serde_json::from_value::<Uuid>(binding.backup["id"].clone())?).bind(binding.organisation_id).bind(source_id).fetch_optional(&mut **tx).await?;
    if catalog.as_ref() != Some(&binding.backup) {
        return Err(ApiError::Conflict(
            "original import backup catalog changed".into(),
        ));
    }
    let owner_valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u JOIN management_members m ON m.user_id=u.id WHERE u.id=$1 AND m.organisation_id=$2 AND m.role IN ('admin','member') AND u.deleted_at IS NULL)")
        .bind(serde_json::from_value::<Uuid>(binding.source["owner_user_id"].clone())?).bind(binding.organisation_id).fetch_one(&mut **tx).await?;
    if !owner_valid {
        return Err(denied());
    }
    for (id, frozen, epoch, host, plan) in [
        (
            source_id,
            &binding.source,
            binding.source_epoch,
            &binding.source_host_binding,
            &binding.source_allocation,
        ),
        (
            target_id,
            &binding.target,
            if matches!(phase, "target_restored_paused" | "qualified") {
                binding.candidate_epoch
            } else {
                binding.target_epoch
            },
            &binding.target_host_binding,
            &binding.target_allocation,
        ),
    ] {
        let value: Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 FOR UPDATE")
            .bind(id).bind(binding.organisation_id).fetch_one(&mut **tx).await?;
        let mut expected_plan = plan.clone();
        if id == target_id && matches!(phase, "target_restored_paused" | "qualified") {
            let original = plan["revision"]
                .as_i64()
                .ok_or_else(|| invalid("import target allocation revision is missing"))?;
            expected_plan["revision"] = json!(original
                .checked_add(1)
                .ok_or_else(|| invalid("import allocation revision overflow"))?);
        }
        if value["state"] != "stopped"
            || value["revision"] != frozen["revision"]
            || value["owner_user_id"] != frozen["owner_user_id"]
            || value["execution_epoch"] != json!(epoch)
            || value["policy"] != frozen["policy"]
            || value["executor_host_id"] != frozen["executor_host_id"]
            || value["current_release"] != frozen["current_release"]
            || host_binding(tx, id).await? != *host
            || allocation(tx, id).await? != expected_plan
        {
            return Err(ApiError::Conflict(
                "import source, target, host or resource plan changed".into(),
            ));
        }
    }
    Ok(())
}
pub(super) async fn dispatch(
    tx: &mut Transaction<'_, Postgres>,
    operation: &mut Value,
) -> Result<(), ApiError> {
    let Some(step) = operation["action"]
        .as_str()
        .and_then(ImportStep::from_action)
    else {
        return Ok(());
    };
    let binding: InstanceImportBinding =
        serde_json::from_value(operation["arguments"]["import_binding"].clone())?;
    let row=sqlx::query("SELECT phase,actor_user_id FROM management_instance_imports WHERE id=$1 AND binding_sha256=$2 FOR UPDATE")
        .bind(binding.import_id).bind(operation["arguments"]["import_binding_sha256"].as_str().unwrap_or(""))
        .fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let phase: String = row.try_get("phase")?;
    if phase != step.phase() || operation["instance_import_id"] != json!(binding.import_id) {
        return Err(ApiError::Conflict("import phase changed".into()));
    }
    admin(tx, binding.organisation_id, row.try_get("actor_user_id")?).await?;
    current(tx, &binding, &phase).await?;
    let held: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_execution_reservations WHERE instance_id=ANY($1) AND state='held')")
        .bind(vec![serde_json::from_value::<Uuid>(binding.source["id"].clone())?,serde_json::from_value::<Uuid>(binding.target["id"].clone())?])
        .fetch_one(&mut **tx).await?;
    if held {
        return Err(ApiError::Conflict("original physical execution still requires exact completion; import does not release reservations".into()));
    }
    let settled:i64=sqlx::query_scalar("SELECT count(*) FROM management_execution_reservations WHERE id=ANY($1) AND instance_id=$2 AND state='released' AND completion IS NOT NULL")
        .bind(&binding.reservation_ids).bind(serde_json::from_value::<Uuid>(binding.source["id"].clone())?).fetch_one(&mut **tx).await?;
    if settled != binding.reservation_ids.len() as i64 {
        return Err(ApiError::Conflict(
            "original reservations require their immutable exact completion receipts".into(),
        ));
    }
    operation["import_binding"] = serde_json::to_value(&binding)?;
    Ok(())
}
pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    row: &sqlx::postgres::PgRow,
    succeeded: bool,
    result: &Value,
) -> Result<(), ApiError> {
    let action: String = row.try_get("action")?;
    let Some(step) = ImportStep::from_action(&action) else {
        return Ok(());
    };
    if !succeeded {
        return Err(invalid(
            "unverified import I/O must stay in reconciliation with both locks retained",
        ));
    }
    let arguments: Value = row.try_get("arguments")?;
    let binding: InstanceImportBinding =
        serde_json::from_value(arguments["import_binding"].clone())?;
    let state = sqlx::query(
        "SELECT phase,export_proof FROM management_instance_imports WHERE id=$1 FOR UPDATE",
    )
    .bind(binding.import_id)
    .fetch_one(&mut **tx)
    .await?;
    let phase: String = state.try_get("phase")?;
    if phase != step.phase() {
        return Err(ApiError::Conflict("import completion phase changed".into()));
    }
    admin(tx, binding.organisation_id, row.try_get("actor_user_id")?).await?;
    let export: Option<Value> = state.try_get("export_proof")?;
    let receipt: InstanceImportReceipt = serde_json::from_value(result["instance_import"].clone())?;
    receipt
        .validate(
            &binding,
            row.try_get("id")?,
            step,
            arguments["import_binding_sha256"].as_str().unwrap_or(""),
            export.as_ref().and_then(|v| v["manifest_sha256"].as_str()),
        )
        .map_err(invalid)?;
    if step != ImportStep::Quiesce
        && json!(receipt.source_inventory_sha256)
            != arguments["import_source_stop"]["source_inventory_sha256"]
    {
        return Err(invalid("original source stop inventory changed"));
    }
    // Restore's exact generation receipt has already atomically promoted the target epoch.
    current(
        tx,
        &binding,
        if step == ImportStep::Restore {
            "target_restored_paused"
        } else {
            &phase
        },
    )
    .await?;
    let new_export = receipt
        .export
        .map(serde_json::to_value)
        .transpose()?
        .or(export);
    sqlx::query("INSERT INTO management_instance_import_receipts(import_id,operation_id,step,receipt) VALUES($1,$2,$3,$4)")
        .bind(binding.import_id).bind(row.try_get::<Uuid,_>("id")?).bind(tag(step)?).bind(&result["instance_import"]).execute(&mut **tx).await?;
    sqlx::query("UPDATE management_instance_imports SET phase=$2,revision=revision+1,export_proof=$3,updated_at=clock_timestamp() WHERE id=$1")
        .bind(binding.import_id).bind(step.completed_phase()).bind(&new_export).execute(&mut **tx).await?;
    if let Some(next) = step.next() {
        enqueue(
            tx,
            &binding,
            row.try_get("actor_user_id")?,
            next,
            new_export,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn inspect(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, false).await?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(m)-'binding' FROM management_instance_imports m WHERE organisation_id=$1 AND id=$2")
        .bind(org).bind(id).fetch_optional(&state.pool).await?.ok_or_else(denied)?))
}
pub(super) async fn cutover(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(request): Json<CutoverInstanceImport>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    let authority = ImportAuthority::from_request(&state, &principal, &headers).await?;
    let mut tx = state.pool.begin().await?;
    locks(&mut tx, org).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    authority.lock(&mut tx).await?;
    admin(&mut tx, org, principal.user_id).await?;
    let row = sqlx::query(
        "SELECT * FROM management_instance_imports WHERE id=$1 AND organisation_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(org)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    let request_digest = digest(&serde_json::to_value(&request)?)?;
    if let Some(receipt) = row.try_get::<Option<Value>, _>("cutover_receipt")? {
        if receipt["request_digest"] != request_digest {
            return Err(ApiError::Conflict(
                "cutover was accepted with a different request".into(),
            ));
        }
        authority.validate(&mut tx).await?;
        return Ok(Json(receipt));
    }
    let binding: InstanceImportBinding = serde_json::from_value(row.try_get("binding")?)?;
    admin(&mut tx, org, row.try_get("actor_user_id")?).await?;
    let target: Uuid = serde_json::from_value(binding.target["id"].clone())?;
    let source: Uuid = serde_json::from_value(binding.source["id"].clone())?;
    if row.try_get::<String, _>("phase")? != "qualified"
        || row.try_get::<i64, _>("revision")? != request.expected_revision
        || target != request.target_instance_id
    {
        return Err(ApiError::Conflict(
            "explicit cutover requires the exact qualified target and import revision".into(),
        ));
    }
    host_capacity::require_admission(
        &mut tx,
        org,
        serde_json::from_value(binding.target["executor_host_id"].clone())?,
        target,
        binding.candidate_epoch,
        &serde_json::from_value(binding.target["policy"].clone())?,
    )
    .await?;
    current(&mut tx, &binding, "qualified").await?;
    authority.validate(&mut tx).await?;
    let changed=sqlx::query("UPDATE management_instance_routes SET target_instance_id=$3,revision=revision+1,import_id=$4 WHERE source_instance_id=$1 AND organisation_id=$2 AND target_instance_id=$1 AND revision=$5")
        .bind(source).bind(org).bind(target).bind(id).bind(request.expected_route_revision).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict(
            "source route changed before cutover".into(),
        ));
    }
    sqlx::query("INSERT INTO management_instance_members(organisation_id,instance_id,user_id,role) SELECT organisation_id,$2,user_id,role FROM management_instance_members WHERE instance_id=$1 ON CONFLICT(instance_id,user_id) DO NOTHING")
        .bind(source).bind(target).execute(&mut *tx).await?;
    let receipt = json!({"id":id,"source_instance_id":source,"target_instance_id":target,"request_digest":request_digest,"idempotency_key":request.idempotency_key,
        "revision":request.expected_revision+1,"route_revision":request.expected_route_revision+1,"target_state":"stopped","schedules_paused":true});
    sqlx::query("UPDATE management_instance_imports SET phase='source_retained',revision=revision+1,cutover_receipt=$2,updated_at=clock_timestamp() WHERE id=$1")
        .bind(id).bind(&receipt).execute(&mut *tx).await?;
    sqlx::query(
        "DELETE FROM management_instance_import_locks WHERE import_id=$1 AND instance_id=$2",
    )
    .bind(id)
    .bind(target)
    .execute(&mut *tx)
    .await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "instance.import.cutover",
        id,
        receipt.clone(),
    )
    .await?;
    authority.validate(&mut tx).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
pub(super) async fn route(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, source)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, false).await?;
    let value:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM management_instance_routes r WHERE organisation_id=$1 AND source_instance_id=$2")
        .bind(org).bind(source).fetch_optional(&state.pool).await?.ok_or_else(denied)?;
    let target: Uuid = serde_json::from_value(value["target_instance_id"].clone())?;
    instance_access(&state, &principal, org, target, false).await?;
    Ok(Json(value))
}

/// Cancel only before target I/O has ever been claimed. Neither epoch, source
/// generation nor charge is rewound; later failures retain both locks.
pub(super) async fn abort(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(request): Json<AbortInstanceImport>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    let authority = ImportAuthority::from_request(&state, &principal, &headers).await?;
    let mut tx = state.pool.begin().await?;
    locks(&mut tx, org).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    authority.lock(&mut tx).await?;
    admin(&mut tx, org, principal.user_id).await?;
    let row = sqlx::query(
        "SELECT * FROM management_instance_imports WHERE id=$1 AND organisation_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(org)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    let request_digest = digest(&serde_json::to_value(&request)?)?;
    if let Some(receipt) = row.try_get::<Option<Value>, _>("abort_receipt")? {
        if receipt["request_digest"] != request_digest {
            return Err(ApiError::Conflict(
                "abort was accepted with a different request".into(),
            ));
        }
        authority.validate(&mut tx).await?;
        return Ok(Json(receipt));
    }
    let phase: String = row.try_get("phase")?;
    if !matches!(phase.as_str(), "source_quiescent" | "export_verified")
        || row.try_get::<i64, _>("revision")? != request.expected_revision
    {
        return Err(ApiError::Conflict(
            "pre-write abort requires proven source quiescence and exact import revision".into(),
        ));
    }
    let writing:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_operations WHERE instance_import_id=$1 AND (state NOT IN ('queued','succeeded') OR (action IN ('import_restore','import_qualify') AND state<>'queued')))")
        .bind(id).fetch_one(&mut *tx).await?;
    if writing {
        return Err(ApiError::Conflict("claimed or unknown import I/O requires reconciliation; target writes cannot be rolled back".into()));
    }
    let binding: InstanceImportBinding = serde_json::from_value(row.try_get("binding")?)?;
    admin(&mut tx, org, row.try_get("actor_user_id")?).await?;
    current(&mut tx, &binding, &phase).await?;
    authority.validate(&mut tx).await?;
    let receipt = json!({"id":id,"phase":"aborted","revision":request.expected_revision+1,"request_digest":request_digest,
        "idempotency_key":request.idempotency_key,"source_instance_id":binding.source["id"],"target_instance_id":binding.target["id"],"source_state":"stopped","target_state":"stopped","charges_retained":true});
    sqlx::query("UPDATE management_operations SET state='failed',result=$2,updated_at=clock_timestamp() WHERE instance_import_id=$1 AND state='queued'")
        .bind(id).bind(json!({"error_code":"import_canceled_before_claim","abort":receipt})).execute(&mut *tx).await?;
    sqlx::query("UPDATE management_instance_imports SET phase='aborted',revision=revision+1,abort_receipt=$2,updated_at=clock_timestamp() WHERE id=$1")
        .bind(id).bind(&receipt).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM management_instance_import_locks WHERE import_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "instance.import.abort",
        id,
        receipt.clone(),
    )
    .await?;
    authority.validate(&mut tx).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}

#[cfg(test)]
#[path = "instance_import_tests.rs"]
mod handler_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL; mutates only its fresh UUID schema"]
    async fn postgres_import_scope_durable_locks_receipts_auth_and_cutover_cas() {
        let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated test database URL");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let schema = format!("cowork_import_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(&format!("CREATE SCHEMA {schema};SET search_path TO {schema};
            CREATE TABLE users(id uuid PRIMARY KEY,deleted_at timestamptz,platform_admin boolean DEFAULT false);
            CREATE TABLE management_organisations(id uuid PRIMARY KEY,archived_at TIMESTAMPTZ);
            CREATE TABLE management_members(organisation_id uuid,user_id uuid,role text);
            CREATE TABLE management_instances(id uuid PRIMARY KEY,organisation_id uuid NOT NULL,UNIQUE(organisation_id,id));
            CREATE TABLE management_operations(id uuid PRIMARY KEY,organisation_id uuid,instance_id uuid,action text);
            CREATE TABLE management_backups(id uuid PRIMARY KEY,organisation_id uuid,instance_id uuid);
            CREATE TABLE runs(id uuid PRIMARY KEY,state text DEFAULT 'interrupted',lease_owner uuid,lease_token uuid,lease_expires_at timestamptz);"))
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/0060_instance_imports.sql"))
            .execute(&pool)
            .await
            .unwrap();
        // Always clean up the owned schema after assertion failures too.
        let result=tokio::spawn(async move {
            let org=Uuid::new_v4();let foreign=Uuid::new_v4();let source=Uuid::new_v4();let target=Uuid::new_v4();
            let other=Uuid::new_v4();let actor=Uuid::new_v4();let backup=Uuid::new_v4();let import=Uuid::new_v4();
            sqlx::query("INSERT INTO users(id) VALUES($1)").bind(actor).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO management_organisations(id) VALUES($1),($2)").bind(org).bind(foreign).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO management_members VALUES($1,$2,'admin')").bind(org).bind(actor).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO management_instances VALUES($1,$3),($2,$3),($4,$5)")
                .bind(source).bind(target).bind(org).bind(other).bind(foreign).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO management_backups VALUES($1,$2,$3)").bind(backup).bind(org).bind(source).execute(&pool).await.unwrap();
            let insert="INSERT INTO management_instance_imports(id,organisation_id,source_instance_id,target_instance_id,actor_user_id,backup_id,idempotency_key,request_digest,binding,binding_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$8)";
            assert!(sqlx::query(insert).bind(Uuid::new_v4()).bind(org).bind(source).bind(other).bind(actor).bind(backup)
                .bind(Uuid::new_v4()).bind("a".repeat(64)).bind(json!({"source":source,"target":other})).execute(&pool).await.is_err());
            sqlx::query(insert).bind(import).bind(org).bind(source).bind(target).bind(actor).bind(backup)
                .bind(Uuid::new_v4()).bind("a".repeat(64)).bind(json!({"source":source,"target":target})).execute(&pool).await.unwrap();
            assert!(sqlx::query("UPDATE management_instance_imports SET binding='{}' WHERE id=$1").bind(import).execute(&pool).await.is_err());
            sqlx::query("INSERT INTO management_instance_import_locks VALUES($1,$3,$4),($2,$3,$4)")
                .bind(source).bind(target).bind(org).bind(import).execute(&pool).await.unwrap();
            let mut tx=pool.begin().await.unwrap();assert!(unlocked(&mut tx,source).await.is_err());
            assert!(unlocked(&mut tx,target).await.is_err());admin(&mut tx,org,actor).await.unwrap();tx.commit().await.unwrap();
            sqlx::query("UPDATE management_members SET role='viewer' WHERE user_id=$1").bind(actor).execute(&pool).await.unwrap();
            let mut tx=pool.begin().await.unwrap();assert!(admin(&mut tx,org,actor).await.is_err());tx.commit().await.unwrap();
            let op=Uuid::new_v4();
            assert!(sqlx::query("INSERT INTO management_operations VALUES($1,$2,$3,'start',NULL)")
                .bind(op).bind(org).bind(source).execute(&pool).await.is_err());
            assert!(sqlx::query("INSERT INTO management_operations VALUES($1,$2,$3,'import_restore',$4)")
                .bind(op).bind(org).bind(source).bind(import).execute(&pool).await.is_err());
            sqlx::query("INSERT INTO management_operations VALUES($1,$2,$3,'import_quiesce',$4)")
                .bind(op).bind(org).bind(source).bind(import).execute(&pool).await.unwrap();
            assert!(sqlx::query("UPDATE management_operations SET instance_import_id=NULL WHERE id=$1").bind(op).execute(&pool).await.is_err());
            sqlx::query("INSERT INTO management_instance_import_receipts(import_id,operation_id,step,receipt) VALUES($1,$2,'quiesce','{}')")
                .bind(import).bind(op).execute(&pool).await.unwrap();
            assert!(sqlx::query("DELETE FROM management_instance_import_receipts WHERE import_id=$1").bind(import).execute(&pool).await.is_err());
            sqlx::query("INSERT INTO management_instance_routes(organisation_id,source_instance_id,target_instance_id) VALUES($1,$2,$2)")
                .bind(org).bind(source).execute(&pool).await.unwrap();
            let cas="UPDATE management_instance_routes SET target_instance_id=$2,revision=revision+1,import_id=$3 WHERE source_instance_id=$1 AND target_instance_id=$1 AND revision=$4";
            assert_eq!(sqlx::query(cas).bind(source).bind(target).bind(import).bind(2_i64).execute(&pool).await.unwrap().rows_affected(),0);
            assert_eq!(sqlx::query(cas).bind(source).bind(target).bind(import).bind(1_i64).execute(&pool).await.unwrap().rows_affected(),1);
            assert_eq!(sqlx::query(cas).bind(source).bind(target).bind(import).bind(1_i64).execute(&pool).await.unwrap().rows_affected(),0);
            sqlx::query("DELETE FROM management_instance_import_locks WHERE instance_id=$1").bind(target).execute(&pool).await.unwrap();
            let mut tx=pool.begin().await.unwrap();unlocked(&mut tx,target).await.unwrap();assert!(unlocked(&mut tx,source).await.is_err());tx.commit().await.unwrap();
            let run=Uuid::new_v4();sqlx::query("INSERT INTO runs(id) VALUES($1)").bind(run).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO run_instance_import_origins(run_id,import_id,organisation_id,source_instance_id,target_instance_id,source_backup_id,source_manifest_sha256) VALUES($1,$2,$3,$4,$5,$6,$7)")
                .bind(run).bind(import).bind(org).bind(source).bind(target).bind(backup).bind("b".repeat(64)).execute(&pool).await.unwrap();
            assert!(sqlx::query("DELETE FROM run_instance_import_origins WHERE run_id=$1").bind(run).execute(&pool).await.is_err());
            assert!(sqlx::query("UPDATE runs SET state='queued' WHERE id=$1").bind(run).execute(&pool).await.is_err());
            assert!(sqlx::query("UPDATE runs SET lease_token=$2 WHERE id=$1").bind(run).bind(Uuid::new_v4()).execute(&pool).await.is_err());
            pool
        }).await;
        // Separate connection recovers cleanup if the task panicked.
        let cleanup = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&cleanup)
            .await
            .unwrap();
        cleanup.close().await;
        result.unwrap().close().await;
    }
}
