//! Original VM disk snapshots; no implicit task replay or replacement host.
use super::*;

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    request_id: Uuid,
    expected_instance_revision: i64,
    action: String,
    #[serde(default)]
    snapshot_id: Option<Uuid>,
    #[serde(default)]
    expected_snapshot_revision: Option<i64>,
    #[serde(default)]
    confirmation_name: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Operator {
    executor_host_binding: executor_hosts::HostBinding,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Completion {
    executor_host_binding: executor_hosts::HostBinding,
    lease_token: Uuid,
    plan_sha256: String,
    receipt: Value,
}
fn conflict() -> ApiError {
    ApiError::Conflict("original VM snapshot, controller, authority or revision changed".into())
}
fn hash(value: &Value) -> Result<String, ApiError> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}
pub(super) async fn unlocked(
    tx: &mut Transaction<'_, Postgres>,
    instance: Uuid,
) -> Result<(), ApiError> {
    let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_vm_jobs WHERE instance_id=$1 AND state IN ('queued','running','held'))").bind(instance).fetch_one(&mut **tx).await?;
    if busy {
        return Err(conflict());
    }
    Ok(())
}
async fn authority(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    plan: &Value,
) -> Result<(), ApiError> {
    let org = serde_json::from_value::<Uuid>(plan["organisation_id"].clone())?;
    let instance = serde_json::from_value::<Uuid>(plan["instance_id"].clone())?;
    let principal = Principal {
        user_id: serde_json::from_value(plan["actor_user_id"].clone())?,
        session_id: None,
        bootstrap: false,
    };
    sso::authorize(tx, org, &principal).await?;
    instance_members::authorize(tx, &principal, org, instance).await?;
    if plan["actor_authority"]["user_id"] != plan["actor_user_id"] {
        return Err(denied());
    }
    job_authority::validate(state, tx, &plan["actor_authority"], org).await?;
    let current:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE organisation_id=$1 AND id=$2 FOR UPDATE").bind(org).bind(instance).fetch_one(&mut **tx).await?;
    if current["state"] != "stopped"
        || current["revision"] != plan["instance"]["revision"]
        || current["execution_epoch"] != plan["instance"]["execution_epoch"]
        || backend_profiles::current(tx, org, instance).await?.as_ref()
            != Some(&plan["backend_profile"])
    {
        return Err(conflict());
    }
    environments::instance_unlocked(tx, instance).await?;
    let held:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_execution_reservations WHERE instance_id=$1 AND state='held') OR EXISTS(SELECT 1 FROM management_operations WHERE instance_id=$1 AND state IN ('queued','running','reconciling'))").bind(instance).fetch_one(&mut **tx).await?;
    if held {
        return Err(conflict());
    }
    Ok(())
}
pub(super) async fn prepare(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance)): Path<(Uuid, Uuid)>,
    Json(request): Json<Request>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    if request.request_id.is_nil()
        || !matches!(
            request.action.as_str(),
            "snapshot" | "restore_snapshot" | "delete_snapshot"
        )
        || (request.action == "snapshot") != request.snapshot_id.is_none()
    {
        return Err(invalid("select an original snapshot action"));
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    let digest = hash(&json!({"organisation_id":org,"instance_id":instance,"request":request}))?;
    if let Some(prior)=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(j)-'lease_token'-'plan' FROM management_vm_jobs j WHERE organisation_id=$1 AND actor_user_id=$2 AND request_id=$3").bind(org).bind(principal.user_id).bind(request.request_id).fetch_optional(&mut *tx).await?{
        if prior["request_sha256"]!=digest || prior["instance_id"]!=json!(instance){return Err(conflict());}return Ok(Json(prior));
    }
    let current:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE organisation_id=$1 AND id=$2 FOR UPDATE").bind(org).bind(instance).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    if current["revision"].as_i64() != Some(request.expected_instance_revision) {
        return Err(conflict());
    }
    let profile = backend_profiles::current(&mut tx, org, instance)
        .await?
        .ok_or_else(conflict)?;
    if profile["specification"]["kind"] != "vm_compose" {
        return Err(invalid(
            "this snapshot adapter requires an activated original VM profile",
        ));
    }
    unlocked(&mut tx, instance).await?;
    if rollouts::reserved(&mut tx, instance).await? {
        return Err(conflict());
    }
    let snapshot = if let Some(id) = request.snapshot_id {
        let row:Value=sqlx::query_scalar("SELECT to_jsonb(s) FROM management_vm_snapshots s WHERE id=$1 AND organisation_id=$2 AND instance_id=$3 AND state='retained' FOR UPDATE").bind(id).bind(org).bind(instance).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
        if row["revision"].as_i64() != request.expected_snapshot_revision
            || request.confirmation_name.as_deref() != current["name"].as_str()
        {
            return Err(invalid(
                "confirm the original instance name and snapshot revision",
            ));
        }
        row
    } else {
        Value::Null
    };
    let id = Uuid::new_v4();
    let snapshot_id = request.snapshot_id.unwrap_or_else(Uuid::new_v4);
    let actor_authority = job_authority::capture(&state, &mut tx, &principal, org).await?;
    let plan = json!({"format":1,"id":id,"organisation_id":org,"instance_id":instance,"actor_user_id":principal.user_id,"actor_authority":actor_authority,"instance":current,"backend_profile":profile,"executor_host_binding":profile["executor_host_binding"],"action":request.action,"snapshot_id":snapshot_id,"snapshot":snapshot,"challenge":Uuid::new_v4()});
    authority(&state, &mut tx, &plan).await?;
    let sha = hash(&plan)?;
    sqlx::query("INSERT INTO management_vm_jobs(id,organisation_id,instance_id,actor_user_id,request_id,request_sha256,action,plan,plan_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(id).bind(org).bind(instance).bind(principal.user_id).bind(request.request_id).bind(digest).bind(&request.action).bind(plan).bind(&sha).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "vm.snapshot.plan",
        id,
        json!({"action":request.action,"snapshot_id":snapshot_id,"plan_sha256":sha}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":"queued","plan_sha256":sha})))
}
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    instance_access(&state, &principal, org, instance, false).await?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(s) FROM management_vm_snapshots s WHERE organisation_id=$1 AND instance_id=$2 ORDER BY created_at DESC").bind(org).bind(instance).fetch_all(&state.pool).await?))
}
pub(super) async fn jobs(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    instance_access(&state, &principal, org, instance, false).await?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(j)-'lease_token'-'plan' FROM management_vm_jobs j WHERE organisation_id=$1 AND instance_id=$2 ORDER BY created_at DESC LIMIT 100").bind(org).bind(instance).fetch_all(&state.pool).await?))
}
pub(super) async fn claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Operator>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let row:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(j) FROM management_vm_jobs j WHERE state='queued' AND plan->'executor_host_binding'=$1 ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1").bind(serde_json::to_value(&request.executor_host_binding)?).fetch_optional(&mut *tx).await?;
    let Some(mut row) = row else {
        return Ok(Json(Value::Null));
    };
    authority(&state, &mut tx, &row["plan"]).await?;
    let lease = Uuid::new_v4();
    sqlx::query("UPDATE management_vm_jobs SET state='running',lease_token=$2 WHERE id=$1")
        .bind(serde_json::from_value::<Uuid>(row["id"].clone())?)
        .bind(lease)
        .execute(&mut *tx)
        .await?;
    row["lease_token"] = json!(lease);
    row["state"] = json!("running");
    tx.commit().await?;
    Ok(Json(row))
}
pub(super) async fn pending(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Operator>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let rows=sqlx::query_scalar("SELECT to_jsonb(j) FROM management_vm_jobs j WHERE state IN ('running','held') AND plan->'executor_host_binding'=$1 ORDER BY created_at LIMIT 100").bind(serde_json::to_value(request.executor_host_binding)?).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(rows))
}
async fn bound<'a>(
    state: &'a AppState,
    headers: &HeaderMap,
    id: Uuid,
    request: &Completion,
) -> Result<(Transaction<'a, Postgres>, Value), ApiError> {
    let operator = executor_hosts::authenticate(state, headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let row: Value =
        sqlx::query_scalar("SELECT to_jsonb(j) FROM management_vm_jobs j WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(denied)?;
    operator.verify_operation(&row["plan"]["executor_host_binding"])?;
    if row["lease_token"] != json!(request.lease_token) || row["plan_sha256"] != request.plan_sha256
    {
        return Err(conflict());
    }
    Ok((tx, row))
}
pub(super) async fn authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Completion>,
) -> Result<Json<Value>, ApiError> {
    let (mut tx, row) = bound(&state, &headers, id, &request).await?;
    if row["state"] != "running" {
        return Err(conflict());
    }
    authority(&state, &mut tx, &row["plan"]).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"plan_sha256":request.plan_sha256,"authorized":true}),
    ))
}
pub(super) async fn hold(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Completion>,
) -> Result<Json<Value>, ApiError> {
    let (mut tx, row) = bound(&state, &headers, id, &request).await?;
    if row["state"] == "completed" {
        return Err(conflict());
    }
    sqlx::query("UPDATE management_vm_jobs SET state='held' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":"held"})))
}
pub(super) async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Completion>,
) -> Result<Json<Value>, ApiError> {
    let (mut tx, row) = bound(&state, &headers, id, &request).await?;
    if row["state"] == "completed" {
        if row["receipt"] != request.receipt {
            return Err(conflict());
        }
        return Ok(Json(row["receipt"].clone()));
    }
    let plan = &row["plan"];
    // Delivery of a past physical receipt does not authorize another VM IO.
    // A revoked/expired original login therefore cannot strand a proven ACK.
    let instance_now:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE organisation_id=$1 AND id=$2 FOR SHARE").bind(serde_json::from_value::<Uuid>(row["organisation_id"].clone())?).bind(serde_json::from_value::<Uuid>(row["instance_id"].clone())?).fetch_one(&mut *tx).await?;
    if instance_now["state"] != "stopped"
        || instance_now["revision"] != plan["instance"]["revision"]
        || instance_now["execution_epoch"] != plan["instance"]["execution_epoch"]
        || backend_profiles::current(
            &mut tx,
            serde_json::from_value(row["organisation_id"].clone())?,
            serde_json::from_value(row["instance_id"].clone())?,
        )
        .await?
        .as_ref()
            != Some(&plan["backend_profile"])
    {
        return Err(conflict());
    }
    let receipt = &request.receipt;
    if receipt["format"] != "cowork-vm-snapshot-completion-v1"
        || receipt["job_id"] != json!(id)
        || receipt["plan_sha256"] != request.plan_sha256
        || receipt["snapshot_id"] != plan["snapshot_id"]
        || receipt["action"] != plan["action"]
        || receipt["vm_id"] != plan["backend_profile"]["specification"]["vm_id"]
        || receipt["backend_profile_sha256"] != plan["backend_profile"]["specification_sha256"]
        || receipt["original_assignment_verified"] != true
        || receipt["physical_quiescence_verified"] != true
        || receipt["actual_vm_running_verified"] != true
        || (plan["action"] != "delete_snapshot" && receipt["actual_vm_restart_verified"] != true)
    {
        return Err(conflict());
    }
    let org = serde_json::from_value::<Uuid>(row["organisation_id"].clone())?;
    let instance = serde_json::from_value::<Uuid>(row["instance_id"].clone())?;
    let snapshot = serde_json::from_value::<Uuid>(plan["snapshot_id"].clone())?;
    if plan["action"] == "snapshot" {
        for key in [
            "archive_sha256",
            "source_disk_sha256",
            "parent_configuration_sha256",
        ] {
            if receipt[key].as_str().is_none_or(|v| {
                v.len() != 64
                    || !v
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }) {
                return Err(conflict());
            }
        }
        if receipt["archive_size_bytes"]
            .as_u64()
            .is_none_or(|v| v == 0)
            || receipt["disk_content_comparison_verified"] != true
            || receipt["encrypted_archive_verified"] != true
        {
            return Err(conflict());
        }
        sqlx::query("INSERT INTO management_vm_snapshots(id,organisation_id,instance_id,source_job_id,receipt) VALUES($1,$2,$3,$4,$5)").bind(snapshot).bind(org).bind(instance).bind(id).bind(receipt).execute(&mut *tx).await?;
    } else {
        let current: Value = sqlx::query_scalar(
            "SELECT to_jsonb(s) FROM management_vm_snapshots s WHERE id=$1 FOR UPDATE",
        )
        .bind(snapshot)
        .fetch_one(&mut *tx)
        .await?;
        if current != plan["snapshot"] {
            return Err(conflict());
        }
        if plan["action"] == "restore_snapshot"
            && (receipt["restored_disk_sha256"]
                != plan["snapshot"]["receipt"]["source_disk_sha256"]
                || receipt["disk_content_comparison_verified"] != true)
        {
            return Err(conflict());
        }
        if plan["action"] == "delete_snapshot" {
            if receipt["original_archive_deleted"] != true
                || receipt["original_inode_unlinked"] != true
            {
                return Err(conflict());
            }
            sqlx::query("UPDATE management_vm_snapshots SET state='deleted',revision=revision+1 WHERE id=$1").bind(snapshot).execute(&mut *tx).await?;
        }
    }
    sqlx::query(
        "UPDATE management_vm_jobs SET state='completed',receipt=$2,completed_at=now() WHERE id=$1",
    )
    .bind(id)
    .bind(receipt)
    .execute(&mut *tx)
    .await?;
    append_audit(
        &mut tx,
        org,
        serde_json::from_value(row["actor_user_id"].clone())?,
        "vm.snapshot.complete",
        id,
        json!({"action":plan["action"],"snapshot_id":snapshot,"receipt_sha256":hash(receipt)?}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(request.receipt))
}
