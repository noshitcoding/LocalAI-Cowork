//! Per-chat device authority. A user proposes a grant; only the original
//! authenticated executor can confirm its independently retained local policy.
use crate::{
    auth::{ExecutorPrincipal, Principal},
    error::ApiError,
    AppState,
};
use axum::{
    extract::{Extension, Path, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use cowork_contracts::{
    enterprise::{safe_relative_path, LaptopAccess},
    ProjectRole, RunRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    pub access: LaptopAccess,
    pub read_roots: Vec<String>,
    pub write_roots: Vec<String>,
    pub capabilities: BTreeSet<String>,
    pub offline_seconds: u32,
    pub max_offline_events: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteGrant {
    request_id: Uuid,
    chat_id: Uuid,
    device_id: Uuid,
    access: LaptopAccess,
    read_roots: Vec<String>,
    write_roots: Vec<String>,
    capabilities: BTreeSet<String>,
    expires_at: DateTime<Utc>,
    offline_seconds: u32,
    max_offline_events: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confirm {
    expected_revision: i64,
    policy_sha256: String,
    local_custody_sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stop {
    pub request_id: Uuid,
    pub expected_revision: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectRequest {
    expected_request_sha256: String,
    purpose: String,
}
const MUTATIONS: &[&str] = &[
    "device_grant_create",
    "workflow_create",
    "transfer_grant_create",
    "transfer_begin",
    "workflow_stop",
    "workflow_continue",
    "workflow_reconcile",
    "workflow_reassign",
    "transfer_environment_reconcile",
    "transfer_revoke",
    "device_stop",
];
pub(crate) async fn admit_request(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    request: Uuid,
    kind: &str,
    target: Uuid,
    sha: &str,
) -> Result<(), ApiError> {
    if request.is_nil() {
        return Err(conflict());
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("enterprise-request:{owner}:{request}"))
        .execute(&mut **tx)
        .await?;
    let rejected:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM enterprise_execution_operations WHERE owner_user_id=$1 AND request_id=$2 AND kind='request_rejected')").bind(owner).bind(request).fetch_one(&mut **tx).await?;
    if rejected {
        return Err(ApiError::Conflict(
            "the original request was explicitly rejected before admission".into(),
        ));
    }
    if let Some(row)=sqlx::query("SELECT kind,target_id,request_sha256 FROM enterprise_execution_requests WHERE owner_user_id=$1 AND request_id=$2").bind(owner).bind(request).fetch_optional(&mut **tx).await?{
        if row.get::<String,_>("kind")!=kind||row.get::<Uuid,_>("target_id")!=target||row.get::<String,_>("request_sha256")!=sha{return Err(conflict())}
    }else{sqlx::query("INSERT INTO enterprise_execution_requests(owner_user_id,request_id,kind,target_id,request_sha256) VALUES($1,$2,$3,$4,$5)").bind(owner).bind(request).bind(kind).bind(target).bind(sha).execute(&mut **tx).await?;}
    Ok(())
}
async fn reject_request(
    State(state): State<AppState>,
    Path(request): Path<Uuid>,
    Extension(p): Extension<Principal>,
    Json(r): Json<RejectRequest>,
) -> Result<Json<Value>, ApiError> {
    if !sha(&r.expected_request_sha256) || !MUTATIONS.contains(&r.purpose.as_str()) {
        return Err(conflict());
    }
    let mut tx = state.pool.begin().await?;
    actor(&state, &mut tx, &p).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("enterprise-request:{}:{request}", p.user_id))
        .execute(&mut *tx)
        .await?;
    let admitted:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM enterprise_execution_requests WHERE owner_user_id=$1 AND request_id=$2)").bind(p.user_id).bind(request).fetch_one(&mut *tx).await?;
    if admitted {
        return Err(ApiError::Conflict(
            "the original request was admitted; recover or settle that same request".into(),
        ));
    }
    if let Some(row)=sqlx::query("SELECT kind,receipt,request_sha256 FROM enterprise_execution_operations WHERE owner_user_id=$1 AND request_id=$2").bind(p.user_id).bind(request).fetch_optional(&mut *tx).await?{
        if row.get::<String,_>("kind")!="request_rejected"||row.get::<String,_>("request_sha256")!=r.expected_request_sha256{return Err(ApiError::Conflict("the original request was admitted; recover its original receipt".into()))}
        let receipt=row.get("receipt");tx.commit().await?;return Ok(Json(receipt))
    }
    let receipt = json!({"format":1,"request_id":request,"request_sha256":r.expected_request_sha256,"purpose":r.purpose,"state":"rejected","admitted":false});
    record_operation(
        &mut tx,
        p.user_id,
        request,
        "request_rejected",
        request,
        &r.expected_request_sha256,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
pub(crate) fn digest(value: &impl Serialize) -> Result<String, ApiError> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}
pub(crate) fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn denied() -> ApiError {
    ApiError::Unauthorized("current original chat/device grant is unavailable".into())
}
pub(crate) fn conflict() -> ApiError {
    ApiError::Conflict("original enterprise request or revision changed".into())
}
pub(crate) async fn original_actor_boundary(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, ApiError> {
    if let Some(expected) = request.headers().get("x-cowork-enterprise-user") {
        let expected = expected
            .to_str()
            .ok()
            .and_then(|s| s.parse::<Uuid>().ok())
            .ok_or_else(denied)?;
        if request.extensions().get::<Principal>().map(|p| p.user_id) != Some(expected) {
            return Err(denied());
        }
    } else if request.method() == axum::http::Method::POST {
        return Err(ApiError::Unprocessable(
            "the original enterprise actor must be pinned".into(),
        ));
    }
    Ok(next.run(request).await)
}

pub(crate) async fn actor(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
) -> Result<(), ApiError> {
    if principal.bootstrap {
        return Err(denied());
    }
    crate::organisation_mfa::session_in_tx(
        state,
        tx,
        principal.user_id,
        principal.session_id.ok_or_else(denied)?,
        false,
    )
    .await?;
    crate::organisation_mfa::session_tx(tx, principal.user_id, principal.session_id).await?;
    Ok(())
}
pub(crate) async fn chat(
    state: &AppState,
    principal: &Principal,
    id: Uuid,
) -> Result<Uuid, ApiError> {
    let project: Uuid =
        sqlx::query_scalar("SELECT project_id FROM threads WHERE id=$1 AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(denied)?;
    crate::organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        project,
        id,
        ProjectRole::Runner,
    )
    .await?;
    Ok(project)
}
pub(crate) async fn lock_chat(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    chat: Uuid,
    project: Uuid,
) -> Result<(), ApiError> {
    // The source rows stay locked through commit; a support/viewer role cannot
    // become an execution grant through this path.
    let permitted:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM threads t JOIN projects p ON p.id=t.project_id LEFT JOIN project_members pm ON pm.project_id=p.id AND pm.user_id=$1 LEFT JOIN team_members tm ON tm.team_id=p.team_id AND tm.user_id=$1 WHERE t.id=$2 AND p.id=$3 AND t.deleted_at IS NULL AND p.deleted_at IS NULL AND (p.owner_user_id=$1 OR pm.role IN ('runner','editor') OR tm.role IN ('owner','admin')))")
  .bind(user).bind(chat).bind(project).fetch_one(&mut **tx).await?;
    if !permitted {
        return Err(denied());
    }
    sqlx::query("SELECT id FROM projects WHERE id=$1 AND deleted_at IS NULL FOR SHARE")
        .bind(project)
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query(
        "SELECT id FROM threads WHERE id=$1 AND project_id=$2 AND deleted_at IS NULL FOR SHARE",
    )
    .bind(chat)
    .bind(project)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("SELECT user_id FROM project_members WHERE project_id=$1 AND user_id=$2 FOR SHARE")
        .bind(project)
        .bind(user)
        .fetch_all(&mut **tx)
        .await?;
    sqlx::query("SELECT tm.user_id FROM team_members tm JOIN projects p ON p.team_id=tm.team_id WHERE p.id=$1 AND tm.user_id=$2 FOR SHARE OF tm").bind(project).bind(user).fetch_all(&mut **tx).await?;
    let fresh:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects p LEFT JOIN project_members pm ON pm.project_id=p.id AND pm.user_id=$1 LEFT JOIN team_members tm ON tm.team_id=p.team_id AND tm.user_id=$1 WHERE p.id=$2 AND p.deleted_at IS NULL AND (p.owner_user_id=$1 OR pm.role IN ('runner','editor') OR tm.role IN ('owner','admin')))").bind(user).bind(project).fetch_one(&mut **tx).await?;
    if !fresh {
        return Err(denied());
    }
    Ok(())
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/enterprise/device-grants", post(create).get(list))
        .route("/enterprise/device-grants/{id}/stop", post(stop))
        .route("/enterprise/operations/{id}", get(operation))
        .route("/enterprise/operations/{id}/reject", post(reject_request))
}
pub fn agent_router() -> Router<AppState> {
    Router::new()
        .route(
            "/agent/executors/{device}/enterprise/device-grants",
            get(pending),
        )
        .route(
            "/agent/executors/{device}/enterprise/device-grants/{id}/confirm",
            post(confirm),
        )
        .route(
            "/agent/executors/{device}/enterprise/device-grants/{id}",
            get(agent_status),
        )
}
async fn create(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(r): Json<WriteGrant>,
) -> Result<Json<Value>, ApiError> {
    let project = chat(&state, &p, r.chat_id).await?;
    if r.request_id.is_nil()
        || r.device_id.is_nil()
        || r.expires_at <= Utc::now()
        || r.expires_at > Utc::now() + chrono::Duration::days(1)
        || r.offline_seconds > 3600
        || r.max_offline_events < 1
        || r.max_offline_events > 10000
        || r.read_roots.len() > 64
        || r.write_roots.len() > 64
        || r.read_roots
            .iter()
            .chain(&r.write_roots)
            .any(|v| !safe_relative_path(v) || v.len() > 1024)
        || r.capabilities.len() > 128
        || r.capabilities.iter().any(|v| v.is_empty() || v.len() > 128)
        || matches!(r.access, LaptopAccess::None)
    {
        return Err(ApiError::Unprocessable(
            "bounded current device policy required".into(),
        ));
    }
    let policy = Policy {
        access: r.access,
        read_roots: r.read_roots.clone(),
        write_roots: r.write_roots.clone(),
        capabilities: r.capabilities.clone(),
        offline_seconds: r.offline_seconds,
        max_offline_events: r.max_offline_events,
    };
    let request_sha = digest(&r)?;
    let policy_sha = digest(&policy)?;
    let mut tx = state.pool.begin().await?;
    actor(&state, &mut tx, &p).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("enterprise-grant:{}:{}", p.user_id, r.request_id))
        .execute(&mut *tx)
        .await?;
    if let Some(row)=sqlx::query("SELECT request_sha256,initial_receipt FROM enterprise_device_grants WHERE owner_user_id=$1 AND request_id=$2").bind(p.user_id).bind(r.request_id).fetch_optional(&mut *tx).await?{
  if row.get::<String,_>("request_sha256")!=request_sha{return Err(conflict())}let receipt=row.get("initial_receipt");tx.commit().await?;return Ok(Json(receipt))}
    lock_chat(&mut tx, p.user_id, r.chat_id, project).await?;
    let device=sqlx::query("SELECT registration FROM executors WHERE id=$1 AND kind='personal_device' AND owner_user_id=$2 AND NOT draining FOR SHARE").bind(r.device_id).bind(p.user_id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    let registration: Value = device.get("registration");
    let available: BTreeSet<String> = registration["capabilities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v["name"].as_str().map(str::to_owned))
        .collect();
    if !r.capabilities.is_subset(&available) {
        return Err(ApiError::Unprocessable(
            "device does not advertise every selected capability".into(),
        ));
    }
    let id = Uuid::new_v4();
    let receipt = json!({"format":1,"id":id,"revision":1,"request_id":r.request_id,"request_sha256":request_sha,"policy_sha256":policy_sha,"state":"pending","chat_id":r.chat_id,"device_id":r.device_id});
    admit_request(
        &mut tx,
        p.user_id,
        r.request_id,
        "device_grant_create",
        id,
        receipt["request_sha256"].as_str().ok_or_else(conflict)?,
    )
    .await?;
    sqlx::query("INSERT INTO enterprise_device_grants(id,owner_user_id,auth_session_id,device_id,chat_id,revision,request_id,request_sha256,initial_receipt,policy,policy_sha256,state,expires_at) VALUES($1,$2,$3,$4,$5,1,$6,$7,$8,$9,$10,'pending',$11)")
  .bind(id).bind(p.user_id).bind(p.session_id).bind(r.device_id).bind(r.chat_id).bind(r.request_id).bind(request_sha).bind(&receipt).bind(serde_json::to_value(policy)?).bind(policy_sha).bind(r.expires_at).execute(&mut *tx).await?;
    actor(&state, &mut tx, &p).await?;
    record_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "device_grant_create",
        id,
        receipt["request_sha256"].as_str().ok_or_else(conflict)?,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let mut tx = state.pool.begin().await?;
    actor(&state, &mut tx, &p).await?;
    let rows=sqlx::query_scalar("SELECT to_jsonb(g)-'auth_session_id'-'confirmation_credential_id' FROM enterprise_device_grants g WHERE owner_user_id=$1 ORDER BY created_at DESC LIMIT 200").bind(p.user_id).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(rows))
}
pub(crate) async fn credential(
    tx: &mut Transaction<'_, Postgres>,
    p: &ExecutorPrincipal,
    device: Uuid,
) -> Result<Uuid, ApiError> {
    if p.executor_id != device {
        return Err(denied());
    }
    sqlx::query_scalar("SELECT e.owner_user_id FROM executors e JOIN executor_credentials c ON c.executor_id=e.id JOIN users u ON u.id=e.owner_user_id WHERE e.id=$1 AND e.kind='personal_device' AND c.id=$2 AND c.revoked_at IS NULL AND (c.expires_at IS NULL OR c.expires_at>clock_timestamp()) AND u.deleted_at IS NULL AND NOT e.draining FOR SHARE OF e,c,u")
  .bind(device).bind(p.credential_id).fetch_optional(&mut **tx).await?.ok_or_else(denied)
}
async fn pending(
    State(state): State<AppState>,
    Path(device): Path<Uuid>,
    Extension(p): Extension<ExecutorPrincipal>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let owner = credential(&mut tx, &p, device).await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(g) FROM enterprise_device_grants g WHERE device_id=$1 AND owner_user_id=$2 AND state IN ('pending','active') AND expires_at>clock_timestamp() ORDER BY created_at LIMIT 200")
  .bind(device).bind(owner).fetch_all(&mut *tx).await?;
    let mut current = Vec::new();
    for mut value in rows {
        let authorized = current_status(&state, &mut tx, &p, &value).await.is_ok();
        if authorized {
            if let Some(object) = value.as_object_mut() {
                object.remove("auth_session_id");
                object.remove("confirmation_credential_id");
                object.insert("current_authorized".into(), json!(true));
            }
            current.push(value);
        }
    }
    tx.commit().await?;
    Ok(Json(current))
}
async fn current_status(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    p: &ExecutorPrincipal,
    value: &Value,
) -> Result<(), ApiError> {
    let owner: Uuid = serde_json::from_value(value["owner_user_id"].clone())?;
    let source = Principal {
        user_id: owner,
        session_id: Some(serde_json::from_value(value["auth_session_id"].clone())?),
        bootstrap: false,
    };
    actor(state, tx, &source).await?;
    let chat_id: Uuid = serde_json::from_value(value["chat_id"].clone())?;
    let project = crate::organisation_mfa::optional_scope(
        owner,
        source.session_id,
        chat(state, &source, chat_id),
    )
    .await?;
    lock_chat(tx, owner, chat_id, project).await?;
    if value["state"] == "active" {
        if value["confirmation_credential_id"] != json!(p.credential_id) {
            return Err(denied());
        }
        active(
            tx,
            serde_json::from_value(value["id"].clone())?,
            owner,
            chat_id,
            p.executor_id,
            value["revision"].as_i64().ok_or_else(conflict)?,
        )
        .await?;
    } else if value["state"] != "pending" {
        return Err(denied());
    }
    credential(tx, p, p.executor_id).await?;
    Ok(())
}
async fn agent_status(
    State(state): State<AppState>,
    Path((device, id)): Path<(Uuid, Uuid)>,
    Extension(p): Extension<ExecutorPrincipal>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let owner = credential(&mut tx, &p, device).await?;
    let mut value:Value=sqlx::query_scalar("SELECT to_jsonb(g) FROM enterprise_device_grants g WHERE id=$1 AND device_id=$2 AND owner_user_id=$3").bind(id).bind(device).bind(owner).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    let authorized = current_status(&state, &mut tx, &p, &value).await.is_ok();
    if let Some(object) = value.as_object_mut() {
        object.remove("auth_session_id");
        object.remove("confirmation_credential_id");
        object.insert("current_authorized".into(), json!(authorized));
    }
    credential(&mut tx, &p, device).await?;
    tx.commit().await?;
    Ok(Json(value))
}
async fn confirm(
    State(state): State<AppState>,
    Path((device, id)): Path<(Uuid, Uuid)>,
    Extension(p): Extension<ExecutorPrincipal>,
    Json(r): Json<Confirm>,
) -> Result<Json<Value>, ApiError> {
    if r.expected_revision < 1 || !sha(&r.policy_sha256) || !sha(&r.local_custody_sha256) {
        return Err(conflict());
    }
    let mut tx = state.pool.begin().await?;
    let owner = credential(&mut tx, &p, device).await?;
    let row=sqlx::query("SELECT * FROM enterprise_device_grants WHERE id=$1 AND device_id=$2 AND owner_user_id=$3 AND expires_at>clock_timestamp() FOR UPDATE").bind(id).bind(device).bind(owner).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    let principal = Principal {
        user_id: owner,
        session_id: Some(row.get("auth_session_id")),
        bootstrap: false,
    };
    actor(&state, &mut tx, &principal).await?;
    let project = chat(&state, &principal, row.get("chat_id")).await?;
    lock_chat(&mut tx, owner, row.get("chat_id"), project).await?;
    if row.get::<String, _>("policy_sha256") != r.policy_sha256 {
        return Err(conflict());
    }
    let receipt = json!({"format":1,"id":id,"revision":r.expected_revision+1,"policy_sha256":r.policy_sha256,"local_custody_sha256":r.local_custody_sha256,"device_id":device,"state":"active"});
    if let Some(original) = row.get::<Option<Value>, _>("confirmation") {
        if original != receipt
            || row.get::<Option<Uuid>, _>("confirmation_credential_id") != Some(p.credential_id)
        {
            return Err(conflict());
        }
        tx.commit().await?;
        return Ok(Json(original));
    }
    if row.get::<i64, _>("revision") != r.expected_revision
        || row.get::<String, _>("state") != "pending"
    {
        return Err(conflict());
    }
    sqlx::query("UPDATE enterprise_device_grants SET revision=revision+1,state='active',confirmation_credential_id=$2,confirmation=$3,confirmed_at=clock_timestamp() WHERE id=$1")
  .bind(id).bind(p.credential_id).bind(&receipt).execute(&mut *tx).await?;
    credential(&mut tx, &p, device).await?;
    actor(&state, &mut tx, &principal).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
pub(crate) async fn original_operation(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    request: Uuid,
    kind: &str,
    target: Uuid,
    sha: &str,
) -> Result<Option<Value>, ApiError> {
    if let Some(row)=sqlx::query("SELECT kind,target_id,request_sha256,receipt FROM enterprise_execution_operations WHERE owner_user_id=$1 AND request_id=$2").bind(owner).bind(request).fetch_optional(&mut **tx).await?{
  if row.get::<String,_>("kind")!=kind||row.get::<Uuid,_>("target_id")!=target||row.get::<String,_>("request_sha256")!=sha{return Err(conflict())}return Ok(Some(row.get("receipt")))}
    Ok(None)
}
pub(crate) async fn record_operation(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    request: Uuid,
    kind: &str,
    target: Uuid,
    sha: &str,
    receipt: &Value,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO enterprise_execution_operations(owner_user_id,request_id,kind,target_id,request_sha256,receipt) VALUES($1,$2,$3,$4,$5,$6)").bind(owner).bind(request).bind(kind).bind(target).bind(sha).bind(receipt).execute(&mut **tx).await?;
    Ok(())
}
async fn operation(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    actor(&state, &mut tx, &p).await?;
    let receipt=sqlx::query_scalar("SELECT receipt FROM enterprise_execution_operations WHERE owner_user_id=$1 AND request_id=$2").bind(p.user_id).bind(id).fetch_optional(&mut *tx).await?.unwrap_or(Value::Null);
    let receipt: Value = if receipt == Value::Null {
        sqlx::query_scalar("SELECT jsonb_build_object('format',1,'request_id',request_id,'kind',kind,'target_id',target_id,'request_sha256',request_sha256,'state','admitted','receipt_pending',true) FROM enterprise_execution_requests WHERE owner_user_id=$1 AND request_id=$2").bind(p.user_id).bind(id).fetch_optional(&mut *tx).await?.unwrap_or(Value::Null)
    } else {
        receipt
    };
    tx.commit().await?;
    Ok(Json(receipt))
}
async fn stop(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(p): Extension<Principal>,
    Json(r): Json<Stop>,
) -> Result<Json<Value>, ApiError> {
    let digest = digest(&(id, &r))?;
    let mut tx = state.pool.begin().await?;
    actor(&state, &mut tx, &p).await?;
    admit_request(&mut tx, p.user_id, r.request_id, "device_stop", id, &digest).await?;
    sqlx::query(
        "SELECT id FROM enterprise_device_grants WHERE id=$1 AND owner_user_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(p.user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    if let Some(original) =
        original_operation(&mut tx, p.user_id, r.request_id, "device_stop", id, &digest).await?
    {
        tx.commit().await?;
        return Ok(Json(original));
    }
    if sqlx::query("UPDATE enterprise_device_grants SET revision=revision+1,state='stopped' WHERE id=$1 AND owner_user_id=$2 AND revision=$3 AND state IN ('pending','active')").bind(id).bind(p.user_id).bind(r.expected_revision).execute(&mut *tx).await?.rows_affected()!=1{return Err(conflict())}
    let receipt = json!({"format":1,"id":id,"request_id":r.request_id,"revision":r.expected_revision+1,"state":"stopped","physical_stop_confirmed":false});
    record_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "device_stop",
        id,
        &digest,
        &receipt,
    )
    .await?;
    sqlx::query("UPDATE enterprise_workflows SET state='stopping',revision=revision+1,stop_auth_session_id=$2 WHERE id IN(SELECT workflow_id FROM enterprise_workflow_steps WHERE device_grant_id=$1) AND state IN ('active','held')").bind(id).bind(p.session_id).execute(&mut *tx).await?;
    let workflows = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT workflow_id FROM enterprise_workflow_steps WHERE device_grant_id=$1",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    for workflow in workflows {
        crate::enterprise_workflows::request_stop_in_tx(&mut tx, workflow).await?;
    }
    tx.commit().await?;
    Ok(Json(receipt))
}
pub(crate) async fn active(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    owner: Uuid,
    chat: Uuid,
    device: Uuid,
    revision: i64,
) -> Result<Value, ApiError> {
    let row=sqlx::query("SELECT g.* FROM enterprise_device_grants g JOIN executor_credentials c ON c.id=g.confirmation_credential_id JOIN executors e ON e.id=g.device_id JOIN auth_sessions s ON s.id=g.auth_session_id AND s.user_id=g.owner_user_id WHERE g.id=$1 AND g.owner_user_id=$2 AND g.chat_id=$3 AND g.device_id=$4 AND g.revision=$5 AND g.state='active' AND g.expires_at>clock_timestamp() AND c.executor_id=e.id AND c.revoked_at IS NULL AND (c.expires_at IS NULL OR c.expires_at>clock_timestamp()) AND e.owner_user_id=$2 AND e.kind='personal_device' AND NOT e.draining AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() FOR SHARE OF g,c,e,s")
  .bind(id).bind(owner).bind(chat).bind(device).bind(revision).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let policy: Value = row.get("policy");
    if digest(&serde_json::from_value::<Policy>(policy.clone())?)?
        != row.get::<String, _>("policy_sha256")
    {
        return Err(conflict());
    }
    Ok(
        json!({"grant_id":id,"revision":revision,"policy_sha256":row.get::<String,_>("policy_sha256"),"access":policy["access"],"read_roots":policy["read_roots"],"write_roots":policy["write_roots"],"capabilities":policy["capabilities"],"offline_seconds":policy["offline_seconds"],"max_offline_events":policy["max_offline_events"]}),
    )
}
pub(crate) async fn writer(tx: &mut Transaction<'_, Postgres>) -> Result<(), ApiError> {
    sqlx::query("SELECT set_config('cowork.enterprise_execution_version','1',true)")
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub(crate) async fn execution_credential(
    tx: &mut Transaction<'_, Postgres>,
    run: Uuid,
    credential: Uuid,
) -> Result<(), ApiError> {
    let original:Option<Uuid>=sqlx::query_scalar("SELECT g.confirmation_credential_id FROM enterprise_workflow_steps s JOIN enterprise_device_grants g ON g.id=s.device_grant_id WHERE s.child_run_id=$1 FOR SHARE OF s,g").bind(run).fetch_optional(&mut **tx).await?.flatten();
    if original.is_some_and(|id| id != credential) {
        return Err(denied());
    }
    Ok(())
}
pub(crate) async fn execution(
    tx: &mut Transaction<'_, Postgres>,
    run: &RunRecord,
) -> Result<(), ApiError> {
    let row=sqlx::query("SELECT s.device_grant_id,s.device_grant_revision,s.request,w.state,COALESCE(s.dispatch_auth_session_id,w.auth_session_id) AS auth_session_id FROM enterprise_workflow_steps s JOIN enterprise_workflows w ON w.id=s.workflow_id WHERE s.child_run_id=$1 FOR SHARE OF s,w").bind(run.spec.id).fetch_optional(&mut **tx).await?;
    if let Some(row) = row {
        writer(tx).await?;
        if row.get::<String, _>("state") != "active" {
            return Err(denied());
        }
        if let cowork_contracts::ExecutorTarget::PersonalDevice { device_id } =
            run.spec.executor_target
        {
            let expected = active(
                tx,
                row.get("device_grant_id"),
                run.spec.creator_user_id,
                run.spec.thread_id,
                device_id,
                row.get("device_grant_revision"),
            )
            .await?;
            if run.spec.input.get("_enterprise_grant") != Some(&expected) {
                return Err(conflict());
            }
            let caps: BTreeSet<String> = expected["capabilities"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            if run
                .spec
                .required_capabilities
                .iter()
                .any(|c| !caps.contains(&c.0))
            {
                return Err(denied());
            }
        } else {
            return Err(denied());
        }
        crate::organisation_mfa::session_tx(
            tx,
            run.spec.creator_user_id,
            Some(row.get("auth_session_id")),
        )
        .await?;
    }
    Ok(())
}
