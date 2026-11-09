//! Total original physical reservation, explicitly qualified by the enrolled
//! outside-guest operator. User JSON, bucket quotas and absent files are no proof.
use super::*;
use chrono::{Duration, Utc};

fn conflict() -> ApiError {
    ApiError::Conflict("original physical disk placement, authority or quota changed".into())
}
fn hash(v: &impl serde::Serialize) -> Result<String, ApiError> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(v)?)))
}
fn number(v: &Value) -> Result<i64, ApiError> {
    v.as_i64().filter(|n| *n >= 0).ok_or_else(conflict)
}
fn uuid(v: &Value) -> Result<Uuid, ApiError> {
    let n: Uuid = serde_json::from_value(v.clone())?;
    if n.is_nil() {
        return Err(conflict());
    }
    Ok(n)
}
fn digest(v: &Value) -> Result<&str, ApiError> {
    v.as_str()
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        .ok_or_else(conflict)
}

#[derive(serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Prepare {
    request_id: Uuid,
    expected_instance_revision: i64,
    expected_profile_revision: i64,
    purpose: String,
    specification: Value,
    confirmation: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Observation {
    executor_host_binding: executor_hosts::HostBinding,
    challenge: Uuid,
    specification_sha256: String,
    observation: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IoRequest {
    organisation_id: Uuid,
    instance_id: Uuid,
    execution_epoch: Uuid,
    profile_revision: i64,
    specification_sha256: String,
    request_id: Uuid,
    purpose: String,
    bytes: u64,
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/organisations/{org}/instances/{instance}/disk-quota",
            get(inspect).post(prepare),
        )
        .route(
            "/organisations/{org}/instances/{instance}/disk-quota/operations/{request}",
            get(original),
        )
        .route(
            "/organisations/{org}/instances/{instance}/disk-quota/commit/{plan}",
            post(commit),
        )
}
pub(super) fn operator_router() -> Router<AppState> {
    Router::new()
        .route("/disk-quota/{plan}/observe", post(observe))
        .route("/disk-quota/{plan}", get(operator_plan))
        .route(
            "/disk-quota/instances/{instance}/challenge",
            post(challenge),
        )
        .route("/disk-quota/instances/{instance}/refresh", post(refresh))
        .route(
            "/disk-quota/operations/{operation}/authorize",
            post(operation_authority),
        )
        .route("/disk-quota/io-authority", post(io_authority))
}

async fn original_authority(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    plan: &Value,
) -> Result<(), ApiError> {
    let org = uuid(&plan["organisation_id"])?;
    let user = uuid(&plan["actor_user_id"])?;
    let current:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u LEFT JOIN management_members m ON m.user_id=u.id AND m.organisation_id=$1 WHERE u.id=$2 AND u.deleted_at IS NULL AND (u.platform_admin OR m.role='admin'))").bind(org).bind(user).fetch_one(&mut **tx).await?;
    if !current {
        return Err(denied());
    }
    job_authority::validate(state, tx, &plan["source_authority"], org).await
}
async fn instance(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    id: Uuid,
    exclusive: bool,
) -> Result<Value, ApiError> {
    let sql = if exclusive {
        "SELECT to_jsonb(i) FROM management_instances i WHERE organisation_id=$1 AND id=$2 FOR UPDATE"
    } else {
        "SELECT to_jsonb(i) FROM management_instances i WHERE organisation_id=$1 AND id=$2 FOR SHARE"
    };
    sqlx::query_scalar(sql)
        .bind(org)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(conflict)
}
async fn effective_bytes(tx: &mut Transaction<'_, Postgres>, i: &Value) -> Result<i64, ApiError> {
    let org: Value =
        sqlx::query_scalar("SELECT policy FROM management_organisations WHERE id=$1 FOR SHARE")
            .bind(uuid(&i["organisation_id"])?)
            .fetch_one(&mut **tx)
            .await?;
    let p = serde_json::from_value::<PolicyRevision>(i["policy"].clone())?
        .restricted_by(&serde_json::from_value(org)?)
        .restricted_by(platform_ceiling());
    Ok(i64::from(p.limits.storage_gib) * 1073741824)
}
fn placements(spec: &Value) -> Result<&Vec<Value>, ApiError> {
    spec["placements"]
        .as_array()
        .filter(|a| (2..=4).contains(&a.len()))
        .ok_or_else(conflict)
}
fn specification(spec: &Value, i: &Value, backend: &Value) -> Result<i64, ApiError> {
    if spec["format"] != "cowork-total-disk-specification-v1"
        || spec["instance_id"] != i["id"]
        || spec["organisation_id"] != i["organisation_id"]
        || backend["specification"]["kind"] != "vm_compose"
        || spec["executor_host_binding"] != backend["executor_host_binding"]
        || spec["backend_profile_revision"] != backend["revision"]
        || spec["backend_specification_sha256"] != backend["specification_sha256"]
    {
        return Err(conflict());
    }
    digest(&spec["guest_helper_sha256"])?;
    digest(&spec["controller_helper_sha256"])?;
    uuid(&spec["execution_epoch"])?;
    let mut total = 0i64;
    let mut ids = std::collections::BTreeSet::new();
    let mut kinds = std::collections::BTreeSet::new();
    for p in placements(spec)? {
        let id = uuid(&p["id"])?;
        let kind = p["kind"].as_str().ok_or_else(conflict)?;
        if !ids.insert(id)
            || !kinds.insert(kind)
            || !matches!(kind, "guest" | "controller" | "environment" | "snapshot")
            || p["instance_id"] != i["id"]
            || p["organisation_id"] != i["organisation_id"]
        {
            return Err(conflict());
        }
        uuid(&p["filesystem_uuid"])?;
        uuid(&p["machine_id"])?;
        if kind == "guest"
            && (p["machine_id"] != backend["specification"]["vm_id"]
                || p["daemon_id"] != backend["specification"]["daemon_id"])
        {
            return Err(conflict());
        }
        let bytes = number(&p["size_bytes"])?;
        if !(1073741824..=1125899906842624).contains(&bytes) {
            return Err(conflict());
        }
        total = total.checked_add(bytes).ok_or_else(conflict)?;
    }
    if !kinds.contains("guest") || !kinds.contains("controller") {
        return Err(conflict());
    }
    if spec["topology"]["database"]["dedicated"] != true
        || spec["topology"]["objects"]["dedicated"] != true
        || spec["topology"]["workspace"]["docker_root_bounded"] != true
    {
        return Err(conflict());
    }
    Ok(total)
}
pub(super) async fn inspect(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &p, org, true).await?;
    let profile:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM management_disk_profiles p WHERE organisation_id=$1 AND instance_id=$2").bind(org).bind(id).fetch_optional(&state.pool).await?;
    let held:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(r) FROM management_disk_reservations r WHERE instance_id=$1 ORDER BY placement_id").bind(id).fetch_all(&state.pool).await?;
    Ok(Json(
        json!({"profile":profile,"reservations":held,"qualified":profile.as_ref().is_some_and(|p|p["state"]=="active"),"legacy_unqualified":profile.is_none()}),
    ))
}
pub(super) async fn original(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((org, id, request)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &p, org, true).await?;
    let plan:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM management_disk_plans p WHERE organisation_id=$1 AND instance_id=$2 AND actor_user_id=$3 AND request_id=$4").bind(org).bind(id).bind(p.user_id).bind(request).fetch_optional(&state.pool).await?;
    Ok(Json(json!({"operation":plan})))
}
pub(super) async fn prepare(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
    Json(r): Json<Prepare>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &p, org, true).await?;
    if r.request_id.is_nil()
        || r.confirmation != "RESERVE_ORIGINAL_TOTAL_PHYSICAL_DISK"
        || !matches!(r.purpose.as_str(), "activate" | "disable" | "release")
    {
        return Err(conflict());
    }
    let request_sha = hash(&r)?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    sso::authorize(&mut tx, org, &p).await?;
    let auth = job_authority::capture(&state, &mut tx, &p, org).await?;
    let i = instance(&mut tx, org, id, true).await?;
    if matches!(i["state"].as_str(), Some("deleted" | "deleting")) && r.purpose != "release" {
        return Err(conflict());
    }
    if let Some(prior)=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(p) FROM management_disk_plans p WHERE organisation_id=$1 AND actor_user_id=$2 AND request_id=$3").bind(org).bind(p.user_id).bind(r.request_id).fetch_optional(&mut *tx).await?{
  if prior["instance_id"]!=json!(id)||prior["request_sha256"]!=request_sha{return Err(conflict());}
  tx.commit().await?;return Ok(Json(prior));
 }
    if i["revision"].as_i64() != Some(r.expected_instance_revision) {
        return Err(conflict());
    }
    let backend = backend_profiles::current(&mut tx, org, id)
        .await?
        .ok_or_else(conflict)?;
    let amount = specification(&r.specification, &i, &backend)?;
    if amount > effective_bytes(&mut tx, &i).await? && r.purpose == "activate" {
        return Err(ApiError::Unavailable(
            "total physical disk reservation exceeds current storage policy".into(),
        ));
    }
    let prior: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM management_disk_profiles p WHERE instance_id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if prior
        .as_ref()
        .and_then(|v| v["revision"].as_i64())
        .unwrap_or(0)
        != r.expected_profile_revision
    {
        return Err(conflict());
    }
    // Placement retarget/resize is never an implicit move of existing data.
    if let Some(old) = &prior {
        if old["specification"] != r.specification || old["state"] == "released" {
            return Err(conflict());
        }
    } else if r.purpose != "activate"
        || r.specification["execution_epoch"] != i["execution_epoch"]
        || !matches!(i["state"].as_str(), Some("pending" | "stopped" | "failed"))
    {
        return Err(conflict());
    }
    let plan = Uuid::new_v4();
    let challenge = Uuid::new_v4();
    let spec_sha = hash(&r.specification)?;
    let result:Value=sqlx::query_scalar("INSERT INTO management_disk_plans(id,organisation_id,instance_id,host_id,actor_user_id,request_id,request_sha256,expected_instance_revision,expected_profile_revision,source_authority,specification,specification_sha256,executor_host_binding,execution_epoch,challenge,purpose) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) RETURNING to_jsonb(management_disk_plans)")
  .bind(plan).bind(org).bind(id).bind(uuid(&i["executor_host_id"])?).bind(p.user_id).bind(r.request_id).bind(request_sha).bind(r.expected_instance_revision).bind(r.expected_profile_revision).bind(auth).bind(&r.specification).bind(spec_sha).bind(&backend["executor_host_binding"]).bind(uuid(&i["execution_epoch"])?).bind(challenge).bind(&r.purpose).fetch_one(&mut *tx).await?;
    for placement in placements(&r.specification)? {
        let prior:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(r) FROM management_disk_reservations r WHERE placement_id=$1 FOR UPDATE").bind(uuid(&placement["id"])?).fetch_optional(&mut *tx).await?;
        if let Some(old) = prior {
            if old["instance_id"] != json!(id)
                || old["filesystem_uuid"] != placement["filesystem_uuid"]
                || old["size_bytes"] != placement["size_bytes"]
                || old["state"] == "released"
            {
                return Err(conflict());
            }
        } else {
            sqlx::query("INSERT INTO management_disk_reservations(placement_id,instance_id,host_id,kind,filesystem_uuid,machine_id,size_bytes,original_plan_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
   .bind(uuid(&placement["id"])?).bind(id).bind(uuid(&i["executor_host_id"])?).bind(placement["kind"].as_str().ok_or_else(conflict)?).bind(uuid(&placement["filesystem_uuid"])?).bind(uuid(&placement["machine_id"])?).bind(number(&placement["size_bytes"])?).bind(plan).execute(&mut *tx).await?;
        }
    }
    append_audit(
        &mut tx,
        org,
        p.user_id,
        "instance.disk.prepare",
        id,
        json!({"plan_id":plan,"reserved_bytes":amount,"purpose":r.purpose}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(result))
}
pub(super) async fn operator_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let op = executor_hosts::authenticate(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    if !op.current(&mut tx).await? {
        return Err(denied());
    }
    let plan: Value =
        sqlx::query_scalar("SELECT to_jsonb(p) FROM management_disk_plans p WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(denied)?;
    op.verify_operation(&plan["executor_host_binding"])?;
    original_authority(&state, &mut tx, &plan).await?;
    tx.commit().await?;
    Ok(Json(plan))
}
fn verify_observation(plan: &Value, observation: &Value) -> Result<(), ApiError> {
    if observation["format"] != "cowork-total-disk-observation-v1"
        || observation["challenge"] != plan["challenge"]
        || observation["specification_sha256"] != plan["specification_sha256"]
        || observation["executor_host_binding"] != plan["executor_host_binding"]
    {
        return Err(conflict());
    }
    let spec = &plan["specification"];
    let rows = observation["placements"].as_array().ok_or_else(conflict)?;
    if rows.len() != placements(spec)?.len() {
        return Err(conflict());
    }
    let mut seen = std::collections::BTreeSet::new();
    for p in placements(spec)? {
        let item = rows
            .iter()
            .find(|o| o["placement"] == *p)
            .ok_or_else(conflict)?;
        if !seen.insert(uuid(&p["id"])?) {
            return Err(conflict());
        }
        if plan["purpose"] == "release" {
            if item["state"] != "released"
                || item["physical_release_verified"] != true
                || item["placement"] != *p
            {
                return Err(conflict());
            }
            digest(&item["original_allocation_sha256"])?;
            continue;
        }
        if item["format"] != "cowork-disk-observation-v1"
            || item["challenge"] != observation["challenge"]
            || item["physical"]["filesystem_uuid"] != p["filesystem_uuid"]
            || item["physical"]["size_bytes"] != p["size_bytes"]
            || number(&item["physical"]["capacity_bytes"])? <= 0
            || number(&item["physical"]["capacity_bytes"])? > number(&p["size_bytes"])?
        {
            return Err(conflict());
        }
        digest(&item["allocation_sha256"])?;
        let expected = if p["kind"] == "guest" {
            &spec["guest_helper_sha256"]
        } else {
            &spec["controller_helper_sha256"]
        };
        if item["source_sha256"] != *expected {
            return Err(conflict());
        }
        if p["kind"] == "guest" {
            if item["topology"]["database_host"] != spec["topology"]["database"]["host"]
                || item["topology"]["s3_endpoint"] != spec["topology"]["objects"]["endpoint"]
            {
                return Err(conflict());
            }
            for k in ["postgres_container_id", "minio_container_id"] {
                digest(&item["topology"][k])?;
            }
            if item["topology"]["docker_root"].as_str().is_none() {
                return Err(conflict());
            }
        } else if p["kind"] == "controller"
            && item["topology"]["paths"] != spec["topology"]["controller_paths"]
        {
            return Err(conflict());
        }
    }
    Ok(())
}
pub(super) async fn observe(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(r): Json<Observation>,
) -> Result<Json<Value>, ApiError> {
    let op = executor_hosts::authenticate(&state, &headers).await?;
    op.verify_claim(Some(&r.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    if !op.current(&mut tx).await? {
        return Err(denied());
    }
    let plan: Value = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM management_disk_plans p WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    op.verify_operation(&plan["executor_host_binding"])?;
    if plan["challenge"] != json!(r.challenge)
        || plan["specification_sha256"] != r.specification_sha256
    {
        return Err(conflict());
    }
    verify_observation(&plan, &r.observation)?;
    // Physical fact capture precedes live authority checks: lost HTTP ACK and
    // subsequent logout must not discard a genuine already-created/deleted disk.
    if !plan["observation"].is_null() {
        if plan["observation"] != r.observation {
            return Err(conflict());
        }
    } else {
        sqlx::query("UPDATE management_disk_plans SET observation=$2,observation_received_at=clock_timestamp(),state='observed' WHERE id=$1 AND state='prepared'").bind(id).bind(&r.observation).execute(&mut *tx).await?;
        for item in r.observation["placements"]
            .as_array()
            .ok_or_else(conflict)?
        {
            let pid = uuid(&item["placement"]["id"])?;
            let original:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM management_disk_reservations r WHERE placement_id=$1 AND instance_id=$2 FOR UPDATE").bind(pid).bind(uuid(&plan["instance_id"])?).fetch_one(&mut *tx).await?;
            if plan["purpose"] == "release" {
                if original["allocation_sha256"] != item["original_allocation_sha256"] {
                    return Err(conflict());
                }
                sqlx::query("UPDATE management_disk_reservations SET state='released',release_receipt=$2 WHERE placement_id=$1 AND state='allocated'").bind(pid).bind(item).execute(&mut *tx).await?;
            } else {
                if !original["allocation_sha256"].is_null()
                    && original["allocation_sha256"] != item["allocation_sha256"]
                {
                    return Err(conflict());
                }
                sqlx::query("UPDATE management_disk_reservations SET state='allocated',allocation_sha256=$2,allocation=$3 WHERE placement_id=$1 AND state IN ('reserved','allocated')").bind(pid).bind(digest(&item["allocation_sha256"])?).bind(item).execute(&mut *tx).await?;
            }
        }
    }
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"observation_sha256":hash(&r.observation)?,"state":"observed"}),
    ))
}
pub(super) async fn commit(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((org, id, plan_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &p, org, true).await?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    sso::authorize(&mut tx, org, &p).await?;
    let plan:Value=sqlx::query_scalar("SELECT to_jsonb(p) FROM management_disk_plans p WHERE id=$1 AND organisation_id=$2 AND instance_id=$3 AND actor_user_id=$4 FOR UPDATE").bind(plan_id).bind(org).bind(id).bind(p.user_id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    original_authority(&state, &mut tx, &plan).await?;
    if !plan["receipt"].is_null() {
        tx.commit().await?;
        return Ok(Json(plan["receipt"].clone()));
    }
    if plan["state"] != "observed" {
        return Err(conflict());
    }
    let fresh:bool=sqlx::query_scalar("SELECT observation_received_at>clock_timestamp()-interval '20 seconds' FROM management_disk_plans WHERE id=$1").bind(plan_id).fetch_one(&mut *tx).await?;
    if !fresh && plan["purpose"] == "activate" {
        return Err(ApiError::Conflict(
            "original observation became stale; prepare a fresh read-only qualification plan"
                .into(),
        ));
    }
    let i = instance(&mut tx, org, id, true).await?;
    let backend = backend_profiles::current(&mut tx, org, id)
        .await?
        .ok_or_else(conflict)?;
    if i["revision"] != plan["expected_instance_revision"]
        || i["execution_epoch"] != plan["execution_epoch"]
    {
        return Err(conflict());
    }
    let amount = specification(&plan["specification"], &i, &backend)?;
    if amount > effective_bytes(&mut tx, &i).await? && plan["purpose"] == "activate" {
        return Err(conflict());
    }
    let prior: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM management_disk_profiles p WHERE instance_id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let old = prior
        .as_ref()
        .and_then(|v| v["revision"].as_i64())
        .unwrap_or(0);
    if old != number(&plan["expected_profile_revision"])? {
        return Err(conflict());
    }
    if plan["purpose"] == "release" && prior.as_ref().is_none_or(|v| v["state"] != "disabled") {
        return Err(conflict());
    }
    let revision = old.checked_add(1).ok_or_else(conflict)?;
    let state_name = match plan["purpose"].as_str() {
        Some("activate") => "active",
        Some("disable") => "disabled",
        Some("release") => "released",
        _ => return Err(conflict()),
    };
    let receipt = json!({"format":"cowork-total-disk-commit-v1","plan_id":plan_id,"request_id":plan["request_id"],"instance_id":id,"organisation_id":org,"execution_epoch":i["execution_epoch"],"profile_revision":revision,"specification_sha256":plan["specification_sha256"],"reserved_bytes":amount,"state":state_name,"observation_sha256":hash(&plan["observation"])?});
    sqlx::query("INSERT INTO management_disk_profiles(instance_id,organisation_id,host_id,revision,plan_id,execution_epoch,specification,specification_sha256,executor_host_binding,receipt,state,reserved_bytes,observed_at,observed_expires_at,observation) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,clock_timestamp(),clock_timestamp()+interval '30 seconds',$13) ON CONFLICT(instance_id) DO UPDATE SET revision=excluded.revision,plan_id=excluded.plan_id,receipt=excluded.receipt,state=excluded.state,observed_at=excluded.observed_at,observed_expires_at=excluded.observed_expires_at,observation=excluded.observation")
 .bind(id).bind(org).bind(uuid(&plan["host_id"])?).bind(revision).bind(plan_id).bind(uuid(&plan["execution_epoch"])?).bind(&plan["specification"]).bind(digest(&plan["specification_sha256"])?).bind(&plan["executor_host_binding"]).bind(&receipt).bind(state_name).bind(amount).bind(&plan["observation"]).execute(&mut *tx).await?;
    sqlx::query("UPDATE management_disk_plans SET receipt=$2,state='committed' WHERE id=$1")
        .bind(plan_id)
        .bind(&receipt)
        .execute(&mut *tx)
        .await?;
    append_audit(
        &mut tx,
        org,
        p.user_id,
        "instance.disk.commit",
        id,
        receipt.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(receipt))
}

pub(super) async fn current(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    id: Uuid,
) -> Result<Value, ApiError> {
    let i = instance(tx, org, id, false).await?;
    if matches!(i["state"].as_str(), Some("deleted" | "deleting")) {
        return Err(conflict());
    }
    let p:Value=sqlx::query_scalar("SELECT to_jsonb(p) FROM management_disk_profiles p WHERE instance_id=$1 AND organisation_id=$2 AND state='active' AND observed_expires_at>clock_timestamp() FOR SHARE").bind(id).bind(org).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::Unavailable("physical disk qualification is missing or expired".into()))?;
    let backend = backend_profiles::current(tx, org, id)
        .await?
        .ok_or_else(conflict)?;
    if p["executor_host_binding"] != backend["executor_host_binding"]
        || specification(&p["specification"], &i, &backend)? > effective_bytes(tx, &i).await?
    {
        return Err(conflict());
    }
    let held:i64=sqlx::query_scalar("SELECT coalesce(sum(size_bytes),0)::BIGINT FROM management_disk_reservations WHERE instance_id=$1 AND state<>'released'").bind(id).fetch_one(&mut **tx).await?;
    if held != number(&p["reserved_bytes"])? {
        return Err(conflict());
    }
    let live:bool=sqlx::query_scalar("SELECT observed_expires_at>clock_timestamp() AND state='active' FROM management_disk_profiles WHERE instance_id=$1").bind(id).fetch_one(&mut **tx).await?;
    if !live {
        return Err(conflict());
    }
    let mut p = p;
    p["current_execution_epoch"] = i["execution_epoch"].clone();
    p["current_instance_state"] = i["state"].clone();
    Ok(p)
}
pub(super) async fn io_authority(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(r): Json<IoRequest>,
) -> Result<Json<Value>, ApiError> {
    if r.request_id.is_nil()
        || r.bytes > 1024u64.pow(5)
        || !matches!(
            r.purpose.as_str(),
            "database" | "objects" | "workspace" | "backup" | "generation" | "snapshot"
        )
    {
        return Err(conflict());
    }
    let mut tx = state.pool.begin().await?;
    crate::fleet::verify_instance_key(&mut tx, &headers, r.organisation_id, r.instance_id).await?;
    let p = current(&mut tx, r.organisation_id, r.instance_id).await?;
    if p["current_instance_state"] != "ready" {
        return Err(conflict());
    }
    if p["current_execution_epoch"] != json!(r.execution_epoch)
        || p["revision"].as_i64() != Some(r.profile_revision)
        || p["specification_sha256"] != r.specification_sha256
    {
        return Err(conflict());
    }
    let request_sha = hash(
        &json!({"organisation_id":r.organisation_id,"instance_id":r.instance_id,"execution_epoch":r.execution_epoch,"profile_revision":r.profile_revision,"specification_sha256":r.specification_sha256,"request_id":r.request_id,"purpose":r.purpose,"bytes":r.bytes}),
    )?;
    let receipt = json!({"format":"cowork-disk-io-authority-v1","request_id":r.request_id,"instance_id":r.instance_id,"organisation_id":r.organisation_id,"execution_epoch":r.execution_epoch,"profile_revision":r.profile_revision,"specification_sha256":r.specification_sha256,"purpose":r.purpose,"bytes":r.bytes,"checked_at":Utc::now(),"expires_at":Utc::now()+Duration::seconds(5)});
    let old:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(r) FROM management_disk_io_receipts r WHERE instance_id=$1 AND request_id=$2").bind(r.instance_id).bind(r.request_id).fetch_optional(&mut *tx).await?;
    let receipt = if let Some(old) = old {
        if old["request_sha256"] != request_sha {
            return Err(conflict());
        }
        old["receipt"].clone()
    } else {
        sqlx::query("INSERT INTO management_disk_io_receipts(instance_id,request_id,purpose,request_sha256,profile_revision,execution_epoch,receipt) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(r.instance_id).bind(r.request_id).bind(&r.purpose).bind(request_sha).bind(r.profile_revision).bind(r.execution_epoch).bind(&receipt).execute(&mut *tx).await?;
        receipt
    };
    // Original expired IO permits never become fresh through response replay.
    tx.commit().await?;
    Ok(Json(receipt))
}
pub(super) async fn environment_admission(
    tx: &mut Transaction<'_, Postgres>,
    i: &Value,
    candidate: Option<&EnvironmentSpec>,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM management_disk_profiles WHERE instance_id=$1)",
    )
    .bind(uuid(&i["id"])?)
    .fetch_one(&mut **tx)
    .await?;
    if !exists {
        return Ok(());
    } // explicit legacy profile remains unqualified
    let p = current(tx, uuid(&i["organisation_id"])?, uuid(&i["id"])?).await?;
    let total: Vec<Value> = sqlx::query_scalar(
        "SELECT spec FROM management_environments WHERE instance_id=$1 AND state<>'deleted'",
    )
    .bind(uuid(&i["id"])?)
    .fetch_all(&mut **tx)
    .await?;
    let mut bytes = 0i64;
    for v in total
        .iter()
        .map(|v| serde_json::from_value::<EnvironmentSpec>(v.clone()))
        .chain(candidate.map(|v| Ok(v.clone())))
    {
        bytes = bytes
            .checked_add(i64::from(v?.limits.storage_gib) * 1073741824)
            .ok_or_else(conflict)?;
    }
    if bytes > number(&p["specification"]["environment_pool_bytes"])? {
        return Err(ApiError::Unavailable(
            "total physical environment disk reservation exhausted".into(),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Refresh {
    executor_host_binding: executor_hosts::HostBinding,
    expected_revision: i64,
    challenge: Option<Uuid>,
    observation: Option<Value>,
}
pub(super) async fn challenge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(r): Json<Refresh>,
) -> Result<Json<Value>, ApiError> {
    let op = executor_hosts::authenticate(&state, &headers).await?;
    op.verify_claim(Some(&r.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    if !op.current(&mut tx).await? {
        return Err(denied());
    }
    let p:Value=sqlx::query_scalar("SELECT to_jsonb(p) FROM management_disk_profiles p WHERE instance_id=$1 AND state='active' FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
    op.verify_operation(&p["executor_host_binding"])?;
    if p["revision"].as_i64() != Some(r.expected_revision) {
        return Err(conflict());
    }
    let nonce = Uuid::new_v4();
    sqlx::query("UPDATE management_disk_profiles SET observation_challenge=$2,challenge_expires_at=clock_timestamp()+interval '20 seconds' WHERE instance_id=$1").bind(id).bind(nonce).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"challenge":nonce,"profile":p})))
}
pub(super) async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(r): Json<Refresh>,
) -> Result<Json<Value>, ApiError> {
    let op = executor_hosts::authenticate(&state, &headers).await?;
    op.verify_claim(Some(&r.executor_host_binding))?;
    let nonce = r.challenge.ok_or_else(conflict)?;
    let observation = r.observation.ok_or_else(conflict)?;
    let mut tx = state.pool.begin().await?;
    if !op.current(&mut tx).await? {
        return Err(denied());
    }
    let p:Value=sqlx::query_scalar("SELECT to_jsonb(p) FROM management_disk_profiles p WHERE instance_id=$1 AND state='active' AND observation_challenge=$2 AND challenge_expires_at>clock_timestamp() FOR UPDATE").bind(id).bind(nonce).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
    op.verify_operation(&p["executor_host_binding"])?;
    if p["revision"].as_i64() != Some(r.expected_revision) {
        return Err(conflict());
    }
    let plan = json!({"specification":p["specification"],"specification_sha256":p["specification_sha256"],"executor_host_binding":p["executor_host_binding"],"purpose":"activate","challenge":nonce});
    verify_observation(&plan, &observation)?;
    for row in observation["placements"].as_array().ok_or_else(conflict)? {
        let known:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM management_disk_reservations r WHERE placement_id=$1 AND instance_id=$2 AND state='allocated' FOR SHARE").bind(uuid(&row["placement"]["id"])?).bind(id).fetch_one(&mut *tx).await?;
        if known["allocation_sha256"] != row["allocation_sha256"] {
            return Err(conflict());
        }
    }
    sqlx::query("UPDATE management_disk_profiles SET observation=$2,observed_at=clock_timestamp(),observed_expires_at=clock_timestamp()+interval '30 seconds',observation_challenge=NULL,challenge_expires_at=NULL WHERE instance_id=$1").bind(id).bind(&observation).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"qualified":true,"profile_revision":r.expected_revision,"challenge":nonce}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Operation {
    executor_host_binding: executor_hosts::HostBinding,
    lease_token: Uuid,
}
pub(super) async fn operation_authority(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(r): Json<Operation>,
) -> Result<Json<Value>, ApiError> {
    let op = executor_hosts::authenticate(&state, &headers).await?;
    op.verify_claim(Some(&r.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    if !op.current(&mut tx).await? {
        return Err(denied());
    }
    let row:Value=sqlx::query_scalar("SELECT to_jsonb(o) FROM management_operations o WHERE id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>clock_timestamp() FOR SHARE").bind(id).bind(r.lease_token).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
    let org = uuid(&row["organisation_id"])?;
    let instance = uuid(&row["instance_id"])?;
    let i = instance_value(&mut tx, org, instance).await?;
    let backend = backend_profiles::current(&mut tx, org, instance)
        .await?
        .ok_or_else(conflict)?;
    op.verify_operation(&backend["executor_host_binding"])?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM management_disk_profiles WHERE instance_id=$1)",
    )
    .bind(instance)
    .fetch_one(&mut *tx)
    .await?;
    let cleanup = matches!(row["action"].as_str(), Some("stop" | "suspend" | "delete"));
    if exists && !cleanup {
        let authority = &row["arguments"]["original_disk_source_authority"];
        if uuid(&authority["user_id"])? != uuid(&row["actor_user_id"])? {
            return Err(denied());
        }
        job_authority::validate(&state, &mut tx, authority, org).await?;
        let principal = Principal {
            user_id: uuid(&authority["user_id"])?,
            session_id: Some(uuid(&authority["session_id"])?),
            bootstrap: false,
        };
        instance_members::authorize(&mut tx, &principal, org, instance).await?;
    }
    let profile = if exists && !cleanup {
        Some(current(&mut tx, org, instance).await?)
    } else {
        None
    };
    let leased:bool=sqlx::query_scalar("SELECT state='running' AND lease_token=$2 AND lease_expires_at>clock_timestamp() FROM management_operations WHERE id=$1").bind(id).bind(r.lease_token).fetch_one(&mut *tx).await?;
    if !leased {
        return Err(conflict());
    }
    tx.commit().await?;
    Ok(Json(
        json!({"operation_id":id,"lease_token":r.lease_token,"instance_id":instance,"execution_epoch":i["execution_epoch"],"profile":profile,"cleanup_only":cleanup,"legacy_unqualified":!exists}),
    ))
}
async fn instance_value(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    id: Uuid,
) -> Result<Value, ApiError> {
    instance(tx, org, id, false).await
}

pub(super) async fn bind_operation(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    p: &Principal,
    org: Uuid,
    id: Uuid,
    args: &mut Value,
) -> Result<(), ApiError> {
    let qualified:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_disk_profiles WHERE instance_id=$1 AND organisation_id=$2)").bind(id).bind(org).fetch_one(&mut **tx).await?;
    if qualified {
        args["original_disk_source_authority"] = job_authority::capture(state, tx, p, org).await?;
    }
    Ok(())
}
