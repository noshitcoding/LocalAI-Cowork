//! Explicit encrypted, original-grant transfers. A destination acknowledges an
//! exact source digest and its original CAS baseline; conflicts retain bytes.
use crate::{
    auth::{ExecutorPrincipal, Principal},
    enterprise_grants as grants,
    error::ApiError,
    storage::SealedValue,
    AppState,
};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Extension, Path, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Utc};
use cowork_contracts::enterprise::{safe_relative_path, ExecutionDestination, TransferGrant};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPin {
    revision: i64,
    profile_sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    request_id: Uuid,
    grant: TransferGrant,
    expires_at: DateTime<Utc>,
    #[serde(default)]
    environment_pins: BTreeMap<Uuid, EnvironmentPin>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Begin {
    request_id: Uuid,
    grant_id: Uuid,
    expected_revision: i64,
    relative_path: String,
    size: i64,
    sha256: String,
    original_destination_sha256: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acknowledge {
    request_id: Uuid,
    source_sha256: String,
    destination_before_sha256: Option<String>,
    destination_after_sha256: String,
    conflict: bool,
    local_receipt_sha256: String,
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/enterprise/transfer-grants",
            post(create_grant).get(list_grants),
        )
        .route("/enterprise/transfers", post(begin).get(list))
        .route("/enterprise/transfers/{id}", get(read))
        .route(
            "/enterprise/transfers/{id}/reconcile",
            post(reconcile_environment),
        )
        .route("/enterprise/transfer-grants/{id}/revoke", post(revoke))
}
pub fn private_router() -> Router<AppState> {
    Router::new().route(
        "/enterprise/transfers/{id}/environment-authority",
        post(environment_authority),
    )
}
pub fn agent_router() -> Router<AppState> {
    Router::new()
        .route(
            "/agent/executors/{device}/enterprise/transfers",
            get(agent_list),
        )
        .route(
            "/agent/executors/{device}/enterprise/transfers/{id}",
            get(agent_read),
        )
        .route(
            "/agent/executors/{device}/enterprise/transfers/{id}/bytes",
            get(download)
                .put(upload)
                .layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route(
            "/agent/executors/{device}/enterprise/transfers/{id}/complete",
            post(complete),
        )
}
async fn endpoint(
    tx: &mut Transaction<'_, Postgres>,
    destination: &ExecutionDestination,
    owner: Uuid,
    chat: Uuid,
) -> Result<Option<Uuid>, ApiError> {
    match destination {
        ExecutionDestination::Device { id, grant_id } => {
            let rev=sqlx::query_scalar("SELECT revision FROM enterprise_device_grants WHERE id=$1 AND owner_user_id=$2 AND chat_id=$3 AND device_id=$4 AND state='active'").bind(grant_id).bind(owner).bind(chat).bind(id).fetch_optional(&mut **tx).await?.ok_or_else(grants::denied)?;
            grants::active(tx, *grant_id, owner, chat, *id, rev).await?;
            Ok(Some(*grant_id))
        }
        ExecutionDestination::Environment { id } if !id.is_nil() && crate::fleet::enabled() => {
            Ok(None)
        }
        _ => Err(grants::denied()),
    }
}
async fn create_grant(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(r): Json<Write>,
) -> Result<Json<Value>, ApiError> {
    let project = grants::chat(&state, &p, r.grant.chat_id).await?;
    if r.request_id.is_nil()
        || r.grant.id.is_nil()
        || r.grant.source == r.grant.destination
        || r.grant.relative_paths.is_empty()
        || r.grant.relative_paths.len() > 128
        || r.grant
            .relative_paths
            .iter()
            .any(|v| !safe_relative_path(v) || v.len() > 1024)
        || r.grant.max_bytes < 1
        || r.grant.max_bytes > 64 * 1024 * 1024
        || r.expires_at <= Utc::now()
        || r.expires_at > Utc::now() + chrono::Duration::days(1)
    {
        return Err(ApiError::Unprocessable(
            "bounded explicit transfer paths and lifetime required".into(),
        ));
    }
    let actual = |key| std::env::var(key).ok().and_then(|v| v.parse::<Uuid>().ok());
    if actual("COWORK_ORGANISATION_ID") != Some(r.grant.organisation_id)
        || actual("COWORK_INSTANCE_ID") != Some(r.grant.instance_id)
    {
        return Err(grants::denied());
    }
    let request_sha = grants::digest(&r)?;
    let environment_ids: Vec<Uuid> = [&r.grant.source, &r.grant.destination]
        .into_iter()
        .filter_map(|v| {
            if let ExecutionDestination::Environment { id } = v {
                Some(*id)
            } else {
                None
            }
        })
        .collect();
    if r.environment_pins.len() != environment_ids.len() {
        return Err(grants::conflict());
    }
    if !environment_ids.is_empty() {
        let Json(context) =
            crate::enterprise_workflows::context(State(state.clone()), Extension(p.clone()))
                .await?;
        for id in &environment_ids {
            let pin = r.environment_pins.get(id).ok_or_else(grants::denied)?;
            if pin.revision < 1
                || !grants::sha(&pin.profile_sha256)
                || !context["environments"]
                    .as_array()
                    .ok_or_else(grants::denied)?
                    .iter()
                    .any(|e| {
                        e["id"] == json!(id)
                            && e["revision"] == pin.revision
                            && e["profile_sha256"] == pin.profile_sha256
                    })
            {
                return Err(grants::denied());
            }
        }
    }
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "enterprise-transfer-grant:{}:{}",
            p.user_id, r.request_id
        ))
        .execute(&mut *tx)
        .await?;
    if let Some(row)=sqlx::query("SELECT request_sha256,initial_receipt FROM enterprise_transfer_grants WHERE owner_user_id=$1 AND request_id=$2").bind(p.user_id).bind(r.request_id).fetch_optional(&mut *tx).await?{
  if row.get::<String,_>("request_sha256")!=request_sha{return Err(grants::conflict())}let receipt=row.get("initial_receipt");tx.commit().await?;return Ok(Json(receipt))}
    grants::lock_chat(&mut tx, p.user_id, r.grant.chat_id, project).await?;
    let source = endpoint(&mut tx, &r.grant.source, p.user_id, r.grant.chat_id).await?;
    let destination = endpoint(&mut tx, &r.grant.destination, p.user_id, r.grant.chat_id).await?;
    let receipt = json!({"format":1,"id":r.grant.id,"revision":1,"request_id":r.request_id,"request_sha256":request_sha,"state":"active"});
    grants::admit_request(
        &mut tx,
        p.user_id,
        r.request_id,
        "transfer_grant_create",
        r.grant.id,
        &request_sha,
    )
    .await?;
    sqlx::query("INSERT INTO enterprise_transfer_grants(id,owner_user_id,auth_session_id,chat_id,revision,request_id,request_sha256,initial_receipt,policy,source_grant_id,destination_grant_id,state,expires_at,environment_pins) VALUES($1,$2,$3,$4,1,$5,$6,$7,$8,$9,$10,'active',$11,$12)")
  .bind(r.grant.id).bind(p.user_id).bind(p.session_id).bind(r.grant.chat_id).bind(r.request_id).bind(request_sha).bind(&receipt).bind(serde_json::to_value(&r.grant)?).bind(source).bind(destination).bind(r.expires_at).bind(serde_json::to_value(r.environment_pins)?).execute(&mut *tx).await?;
    grants::actor(&state, &mut tx, &p).await?;
    grants::record_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "transfer_grant_create",
        r.grant.id,
        &receipt["request_sha256"]
            .as_str()
            .ok_or_else(grants::conflict)?
            .to_owned(),
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
async fn current(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    owner: Uuid,
    revision: i64,
) -> Result<TransferGrant, ApiError> {
    let row=sqlx::query("SELECT g.policy FROM enterprise_transfer_grants g JOIN auth_sessions s ON s.id=g.auth_session_id AND s.user_id=g.owner_user_id WHERE g.id=$1 AND g.owner_user_id=$2 AND g.revision=$3 AND g.state='active' AND g.expires_at>clock_timestamp() AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() FOR SHARE OF g,s")
  .bind(id).bind(owner).bind(revision).fetch_optional(&mut **tx).await?.ok_or_else(grants::denied)?;
    let policy: TransferGrant = serde_json::from_value(row.get("policy"))?;
    endpoint(tx, &policy.source, owner, policy.chat_id).await?;
    endpoint(tx, &policy.destination, owner, policy.chat_id).await?;
    Ok(policy)
}
async fn begin(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(r): Json<Begin>,
) -> Result<Json<Value>, ApiError> {
    if r.request_id.is_nil()
        || r.size < 0
        || r.size > 16 * 1024 * 1024
        || !safe_relative_path(&r.relative_path)
        || !grants::sha(&r.sha256)
        || r.original_destination_sha256
            .as_ref()
            .map_or(false, |v| !grants::sha(v))
    {
        return Err(ApiError::Unprocessable(
            "bounded original file size, digest and destination baseline required".into(),
        ));
    }
    let request_sha = grants::digest(&r)?;
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "enterprise-transfer:{}:{}",
            p.user_id, r.request_id
        ))
        .execute(&mut *tx)
        .await?;
    if let Some(row)=sqlx::query("SELECT request_sha256,initial_receipt FROM enterprise_transfers WHERE owner_user_id=$1 AND request_id=$2").bind(p.user_id).bind(r.request_id).fetch_optional(&mut *tx).await?{
  if row.get::<String,_>("request_sha256")!=request_sha{return Err(grants::conflict())}let receipt=row.get("initial_receipt");tx.commit().await?;return Ok(Json(receipt))}
    sqlx::query(
        "SELECT id FROM enterprise_transfer_grants WHERE id=$1 AND owner_user_id=$2 FOR UPDATE",
    )
    .bind(r.grant_id)
    .bind(p.user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(grants::denied)?;
    let grant = current(&mut tx, r.grant_id, p.user_id, r.expected_revision).await?;
    if (matches!(grant.source, ExecutionDestination::Environment { .. })
        || matches!(grant.destination, ExecutionDestination::Environment { .. }))
        && r.size > 256 * 1024
    {
        return Err(ApiError::Unprocessable(
            "environment transfer file exceeds 256 KiB".into(),
        ));
    }
    if !grant
        .relative_paths
        .iter()
        .any(|root| r.relative_path == *root || r.relative_path.starts_with(&(root.clone() + "/")))
        || r.original_destination_sha256.is_some() && !grant.writable
    {
        return Err(grants::denied());
    }
    let project = grants::chat(&state, &p, grant.chat_id).await?;
    grants::lock_chat(&mut tx, p.user_id, grant.chat_id, project).await?;
    crate::governance::enforce_storage_quota_tx(&mut tx, "user", p.user_id, r.size as u64).await?;
    if sqlx::query("UPDATE enterprise_transfer_grants SET reserved_bytes=reserved_bytes+$3 WHERE id=$1 AND revision=$2 AND reserved_bytes+$3<=$4").bind(r.grant_id).bind(r.expected_revision).bind(r.size).bind(grant.max_bytes as i64).execute(&mut *tx).await?.rows_affected()!=1{return Err(ApiError::Quota("transfer grant byte reservation exhausted".into()))}
    let id = Uuid::new_v4();
    let receipt = json!({"format":1,"id":id,"request_id":r.request_id,"request_sha256":request_sha,"state":"receiving","size":r.size,"sha256":r.sha256});
    grants::admit_request(
        &mut tx,
        p.user_id,
        r.request_id,
        "transfer_begin",
        id,
        &request_sha,
    )
    .await?;
    sqlx::query("INSERT INTO enterprise_transfers(id,grant_id,grant_revision,owner_user_id,request_id,request_sha256,initial_receipt,relative_path,original_destination_sha256,size,sha256,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'receiving')")
  .bind(id).bind(r.grant_id).bind(r.expected_revision).bind(p.user_id).bind(r.request_id).bind(request_sha).bind(&receipt).bind(r.relative_path).bind(r.original_destination_sha256).bind(r.size).bind(r.sha256).execute(&mut *tx).await?;
    grants::actor(&state, &mut tx, &p).await?;
    grants::record_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "transfer_begin",
        id,
        &receipt["request_sha256"]
            .as_str()
            .ok_or_else(grants::conflict)?
            .to_owned(),
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
async fn list_grants(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    let rows=sqlx::query_scalar("SELECT to_jsonb(g)-'auth_session_id' FROM enterprise_transfer_grants g WHERE owner_user_id=$1 ORDER BY id LIMIT 200").bind(p.user_id).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(rows))
}
async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    let rows=sqlx::query_scalar("SELECT to_jsonb(t)-'ciphertext'-'encrypted_data_key'-'nonce'-'wrap_nonce' FROM enterprise_transfers t WHERE owner_user_id=$1 ORDER BY created_at DESC LIMIT 200").bind(p.user_id).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(rows))
}
async fn read(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    let value=sqlx::query_scalar("SELECT to_jsonb(t)-'ciphertext'-'encrypted_data_key'-'nonce'-'wrap_nonce' FROM enterprise_transfers t WHERE id=$1 AND owner_user_id=$2").bind(id).bind(p.user_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    tx.commit().await?;
    Ok(Json(value))
}
async fn agent_list(
    State(state): State<AppState>,
    Path(device): Path<Uuid>,
    Extension(p): Extension<ExecutorPrincipal>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let owner = grants::credential(&mut tx, &p, device).await?;
    let mut rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(t)-'ciphertext'-'encrypted_data_key'-'nonce'-'wrap_nonce'||jsonb_build_object('grant',g.policy) FROM enterprise_transfers t JOIN enterprise_transfer_grants g ON g.id=t.grant_id WHERE t.owner_user_id=$1 AND (g.policy->'source'->>'id'=$2 OR g.policy->'destination'->>'id'=$2) AND g.state='active' AND g.expires_at>clock_timestamp() AND t.state IN ('receiving','ready','conflict') ORDER BY t.created_at LIMIT 100")
  .bind(owner).bind(device.to_string()).fetch_all(&mut *tx).await?;
    for value in &mut rows {
        value["current_authorized"] =
            json!(agent_authorized(&state, &mut tx, &p, value).await.is_ok());
    }
    tx.commit().await?;
    Ok(Json(rows))
}
async fn agent_authorized(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    p: &ExecutorPrincipal,
    value: &Value,
) -> Result<(), ApiError> {
    let id = serde_json::from_value(value["grant_id"].clone())?;
    let revision = value["grant_revision"]
        .as_i64()
        .ok_or_else(grants::denied)?;
    let owner = grants::credential(tx, p, p.executor_id).await?;
    let grant = current(tx, id, owner, revision).await?;
    let endpoint = [&grant.source, &grant.destination]
        .into_iter()
        .find_map(|endpoint| {
            if let ExecutionDestination::Device { id, grant_id } = endpoint {
                if *id == p.executor_id {
                    Some(*grant_id)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .ok_or_else(grants::denied)?;
    let issuer: Uuid =
        sqlx::query_scalar("SELECT auth_session_id FROM enterprise_transfer_grants WHERE id=$1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    let actor = Principal {
        user_id: owner,
        session_id: Some(issuer),
        bootstrap: false,
    };
    grants::actor(state, tx, &actor).await?;
    let project = grants::chat(state, &actor, grant.chat_id).await?;
    grants::lock_chat(tx, owner, grant.chat_id, project).await?;
    let original:Uuid=sqlx::query_scalar("SELECT confirmation_credential_id FROM enterprise_device_grants WHERE id=$1 AND state='active' FOR SHARE").bind(endpoint).fetch_one(&mut **tx).await?;
    if original != p.credential_id {
        return Err(grants::denied());
    }
    Ok(())
}
async fn agent_read(
    State(state): State<AppState>,
    Path((device, id)): Path<(Uuid, Uuid)>,
    Extension(p): Extension<ExecutorPrincipal>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let owner = grants::credential(&mut tx, &p, device).await?;
    let mut value:Value=sqlx::query_scalar("SELECT to_jsonb(t)-'ciphertext'-'encrypted_data_key'-'nonce'-'wrap_nonce'||jsonb_build_object('grant',g.policy) FROM enterprise_transfers t JOIN enterprise_transfer_grants g ON g.id=t.grant_id WHERE t.id=$1 AND t.owner_user_id=$2").bind(id).bind(owner).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    let policy: TransferGrant = serde_json::from_value(value["grant"].clone())?;
    if ![&policy.source, &policy.destination]
        .into_iter()
        .any(|v| matches!(v,ExecutionDestination::Device{id,..} if *id==device))
    {
        return Err(grants::denied());
    }
    let authorized = agent_authorized(&state, &mut tx, &p, &value).await.is_ok();
    value["current_authorized"] = json!(authorized);
    grants::credential(&mut tx, &p, device).await?;
    tx.commit().await?;
    Ok(Json(value))
}
async fn upload(
    State(state): State<AppState>,
    Path((device, id)): Path<(Uuid, Uuid)>,
    Extension(p): Extension<ExecutorPrincipal>,
    bytes: Bytes,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let owner = grants::credential(&mut tx, &p, device).await?;
    let row = sqlx::query(
        "SELECT * FROM enterprise_transfers WHERE id=$1 AND owner_user_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(grants::denied)?;
    agent_authorized(&state,&mut tx,&p,&json!({"grant_id":row.get::<Uuid,_>("grant_id"),"grant_revision":row.get::<i64,_>("grant_revision")})).await?;
    let grant = current(
        &mut tx,
        row.get("grant_id"),
        owner,
        row.get("grant_revision"),
    )
    .await?;
    if !matches!(grant.source,ExecutionDestination::Device{id,..} if id==device)
        || bytes.len() as i64 != row.get::<i64, _>("size")
        || hex::encode(sha2::Sha256::digest(&bytes)) != row.get::<String, _>("sha256")
    {
        return Err(grants::conflict());
    }
    if row.get::<Option<Vec<u8>>, _>("ciphertext").is_none() {
        let key = state.auth_secrets.as_ref().ok_or_else(|| {
            ApiError::Unavailable("encrypted transfer storage is unavailable".into())
        })?;
        let sealed = key.seal_for_user(owner, &bytes)?;
        sqlx::query("UPDATE enterprise_transfers SET ciphertext=$2,encrypted_data_key=$3,nonce=$4,wrap_nonce=$5,state='ready' WHERE id=$1 AND state='receiving'").bind(id).bind(sealed.ciphertext).bind(sealed.encrypted_data_key).bind(sealed.nonce.as_slice()).bind(sealed.wrap_nonce.as_slice()).execute(&mut *tx).await?;
    }
    let receipt = json!({"format":1,"id":id,"sha256":row.get::<String,_>("sha256"),"size":bytes.len(),"source_device_id":device,"source_bytes_retained":true});
    grants::credential(&mut tx, &p, device).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
async fn download(
    State(state): State<AppState>,
    Path((device, id)): Path<(Uuid, Uuid)>,
    Extension(p): Extension<ExecutorPrincipal>,
) -> Result<Response, ApiError> {
    let mut tx = state.pool.begin().await?;
    let owner = grants::credential(&mut tx, &p, device).await?;
    let row=sqlx::query("SELECT * FROM enterprise_transfers WHERE id=$1 AND owner_user_id=$2 AND ciphertext IS NOT NULL FOR SHARE").bind(id).bind(owner).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    agent_authorized(&state,&mut tx,&p,&json!({"grant_id":row.get::<Uuid,_>("grant_id"),"grant_revision":row.get::<i64,_>("grant_revision")})).await?;
    let grant = current(
        &mut tx,
        row.get("grant_id"),
        owner,
        row.get("grant_revision"),
    )
    .await?;
    if !matches!(grant.destination,ExecutionDestination::Device{id,..} if id==device) {
        return Err(grants::denied());
    }
    let sealed = SealedValue {
        ciphertext: row.get("ciphertext"),
        encrypted_data_key: row.get("encrypted_data_key"),
        nonce: row
            .get::<Vec<u8>, _>("nonce")
            .try_into()
            .map_err(|_| grants::conflict())?,
        wrap_nonce: row
            .get::<Vec<u8>, _>("wrap_nonce")
            .try_into()
            .map_err(|_| grants::conflict())?,
    };
    let bytes = state
        .auth_secrets
        .as_ref()
        .ok_or_else(grants::denied)?
        .open_for_user(owner, &sealed)?;
    let digest: String = row.get("sha256");
    if bytes.len() as i64 != row.get::<i64, _>("size")
        || hex::encode(sha2::Sha256::digest(&bytes)) != digest
    {
        return Err(grants::conflict());
    }
    grants::credential(&mut tx, &p, device).await?;
    tx.commit().await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    headers.insert(
        "x-content-sha256",
        digest.parse().map_err(|_| grants::conflict())?,
    );
    Ok((headers, bytes).into_response())
}
async fn complete(
    State(state): State<AppState>,
    Path((device, id)): Path<(Uuid, Uuid)>,
    Extension(p): Extension<ExecutorPrincipal>,
    Json(r): Json<Acknowledge>,
) -> Result<Json<Value>, ApiError> {
    if !grants::sha(&r.source_sha256)
        || !grants::sha(&r.destination_after_sha256)
        || !grants::sha(&r.local_receipt_sha256)
    {
        return Err(grants::conflict());
    }
    let request_sha = grants::digest(&r)?;
    let mut tx = state.pool.begin().await?;
    let owner = grants::credential(&mut tx, &p, device).await?;
    let row = sqlx::query(
        "SELECT * FROM enterprise_transfers WHERE id=$1 AND owner_user_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(grants::denied)?;
    if let Some(original) = grants::original_operation(
        &mut tx,
        owner,
        r.request_id,
        "transfer_complete",
        id,
        &request_sha,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(Json(original));
    }
    agent_authorized(&state,&mut tx,&p,&json!({"grant_id":row.get::<Uuid,_>("grant_id"),"grant_revision":row.get::<i64,_>("grant_revision")})).await?;
    let grant = current(
        &mut tx,
        row.get("grant_id"),
        owner,
        row.get("grant_revision"),
    )
    .await?;
    if !matches!(grant.destination,ExecutionDestination::Device{id,..} if id==device)
        || row.get::<String, _>("state") != "ready"
        || row.get::<String, _>("sha256") != r.source_sha256
        || row.get::<Option<String>, _>("original_destination_sha256")
            != r.destination_before_sha256
        || !r.conflict && r.destination_after_sha256 != r.source_sha256
    {
        return Err(grants::conflict());
    }
    let next = if r.conflict { "conflict" } else { "completed" };
    let receipt = json!({"format":1,"id":id,"request_id":r.request_id,"request_sha256":request_sha,"state":next,"source_sha256":r.source_sha256,"destination_before_sha256":r.destination_before_sha256,"destination_after_sha256":r.destination_after_sha256,"local_receipt_sha256":r.local_receipt_sha256,"destination_device_id":device});
    sqlx::query(
        "UPDATE enterprise_transfers SET state=$2,receipt=$3 WHERE id=$1 AND receipt IS NULL",
    )
    .bind(id)
    .bind(next)
    .bind(&receipt)
    .execute(&mut *tx)
    .await?;
    grants::record_operation(
        &mut tx,
        owner,
        r.request_id,
        "transfer_complete",
        id,
        &request_sha,
        &receipt,
    )
    .await?;
    grants::credential(&mut tx, &p, device).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
async fn revoke(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(p): Extension<Principal>,
    Json(r): Json<grants::Stop>,
) -> Result<Json<Value>, ApiError> {
    let request_sha = grants::digest(&(id, &r))?;
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    grants::admit_request(
        &mut tx,
        p.user_id,
        r.request_id,
        "transfer_revoke",
        id,
        &request_sha,
    )
    .await?;
    sqlx::query(
        "SELECT id FROM enterprise_transfer_grants WHERE id=$1 AND owner_user_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(p.user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(grants::denied)?;
    if let Some(original) = grants::original_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "transfer_revoke",
        id,
        &request_sha,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(Json(original));
    }
    if sqlx::query("UPDATE enterprise_transfer_grants SET state='revoked',revision=revision+1 WHERE id=$1 AND revision=$2 AND state='active'").bind(id).bind(r.expected_revision).execute(&mut *tx).await?.rows_affected()!=1{return Err(grants::conflict())}
    let receipt = json!({"format":1,"id":id,"request_id":r.request_id,"state":"revoked","revision":r.expected_revision+1,"original_transfer_bytes_retained":true});
    grants::record_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "transfer_revoke",
        id,
        &request_sha,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(receipt))
}

use sha2::Digest;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvironmentAuthority {
    organisation_id: Uuid,
    instance_id: Uuid,
    execution_epoch: Uuid,
    user_id: Uuid,
    management_session_id: Uuid,
    environment_id: Uuid,
    transfer_request_id: Uuid,
    transfer_grant_id: Uuid,
    expected_transfer_revision: i64,
    operation: String,
    relative_path: String,
}
async fn environment_authority(
    State(state): State<AppState>,
    Path(request): Path<Uuid>,
    headers: HeaderMap,
    Json(r): Json<EnvironmentAuthority>,
) -> Result<Json<Value>, ApiError> {
    use subtle::ConstantTimeEq;
    if state.management_mode || !crate::fleet::enabled() {
        return Err(grants::denied());
    }
    state
        .ensure_api_active()
        .await
        .map_err(ApiError::Internal)?;
    let expected = crate::fleet::secret("COWORK_FLEET_KEY_FILE")?;
    let supplied = headers
        .get("x-cowork-instance-key")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !bool::from(expected.as_bytes().ct_eq(supplied.as_bytes())) {
        return Err(grants::denied());
    }
    for (name, id) in [
        ("COWORK_ORGANISATION_ID", r.organisation_id),
        ("COWORK_INSTANCE_ID", r.instance_id),
        ("COWORK_EXECUTION_EPOCH", r.execution_epoch),
    ] {
        if id.is_nil()
            || std::env::var(name)
                .ok()
                .and_then(|s| s.parse::<Uuid>().ok())
                != Some(id)
        {
            return Err(grants::denied());
        }
    }
    if request != r.transfer_request_id || !safe_relative_path(&r.relative_path) {
        return Err(grants::denied());
    }
    let mut tx = state.pool.begin().await?;
    let row=sqlx::query("SELECT t.*,g.auth_session_id FROM enterprise_transfers t JOIN enterprise_transfer_grants g ON g.id=t.grant_id WHERE t.request_id=$1 AND t.owner_user_id=$2 FOR SHARE OF t,g").bind(request).bind(r.user_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    if row.get::<Uuid, _>("grant_id") != r.transfer_grant_id
        || row.get::<i64, _>("grant_revision") != r.expected_transfer_revision
        || row.get::<String, _>("relative_path") != r.relative_path
    {
        return Err(grants::denied());
    }
    let grant = current(
        &mut tx,
        r.transfer_grant_id,
        r.user_id,
        r.expected_transfer_revision,
    )
    .await?;
    let (leg, endpoint, required_state) = match r.operation.as_str() {
        "file_read" => ("source", &grant.source, "receiving"),
        "file_write" => ("destination", &grant.destination, "ready"),
        _ => return Err(grants::denied()),
    };
    if !matches!(endpoint,ExecutionDestination::Environment{id} if *id==r.environment_id)
        || row.get::<String, _>("state") != required_state
        || !grant.relative_paths.iter().any(|root| {
            r.relative_path == *root || r.relative_path.starts_with(&(root.clone() + "/"))
        })
    {
        return Err(grants::denied());
    }
    if r.operation == "file_write"
        && !grant.writable
        && row
            .get::<Option<String>, _>("original_destination_sha256")
            .is_some()
    {
        return Err(grants::denied());
    }
    let original:Value=sqlx::query_scalar("SELECT original_request FROM enterprise_transfer_environment_operations WHERE transfer_id=$1 AND leg=$2 AND state='admitted' FOR SHARE").bind(row.get::<Uuid,_>("id")).bind(leg).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    if original["management_session_id"] != json!(r.management_session_id)
        || original["environment_id"] != json!(r.environment_id)
        || original["task_input"]["transfer_request_id"] != json!(request)
    {
        return Err(grants::denied());
    }
    let p = Principal {
        user_id: r.user_id,
        session_id: Some(row.get("auth_session_id")),
        bootstrap: false,
    };
    grants::actor(&state, &mut tx, &p).await?;
    let project = grants::chat(&state, &p, grant.chat_id).await?;
    grants::lock_chat(&mut tx, p.user_id, grant.chat_id, project).await?;
    let origin:Option<Value>=sqlx::query_scalar("SELECT auth_origin FROM auth_sessions WHERE user_id=$1 AND revoked_at IS NULL AND expires_at>clock_timestamp() AND auth_origin->>'kind'='instance_launch' AND auth_origin->>'management_session_id'=$2 LIMIT 1 FOR SHARE").bind(r.user_id).bind(r.management_session_id.to_string()).fetch_optional(&mut *tx).await?.flatten();
    if origin.is_none() {
        return Err(grants::denied());
    }
    tx.commit().await?;
    Ok(Json(json!({"format":1,"authorized":true,"original":r})))
}

async fn reconcile_environment(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(p): Extension<Principal>,
    Json(r): Json<grants::Stop>,
) -> Result<Json<Value>, ApiError> {
    if r.request_id.is_nil() {
        return Err(grants::conflict());
    }
    let request_sha = grants::digest(&(id, &r))?;
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    grants::admit_request(
        &mut tx,
        p.user_id,
        r.request_id,
        "transfer_environment_reconcile",
        id,
        &request_sha,
    )
    .await?;
    let row=sqlx::query("SELECT t.*,g.environment_pins,g.policy FROM enterprise_transfers t JOIN enterprise_transfer_grants g ON g.id=t.grant_id WHERE t.id=$1 AND t.owner_user_id=$2 FOR UPDATE OF t").bind(id).bind(p.user_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    if let Some(receipt) = grants::original_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "transfer_environment_reconcile",
        id,
        &request_sha,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(Json(receipt));
    }
    if row.get::<i64, _>("grant_revision") != r.expected_revision {
        return Err(grants::conflict());
    }
    let grant: TransferGrant = serde_json::from_value(row.get("policy"))?;
    let project = grants::chat(&state, &p, grant.chat_id).await?;
    grants::lock_chat(&mut tx, p.user_id, grant.chat_id, project).await?;
    let state_name: String = row.get("state");
    let (leg, destination, operation) = if state_name == "receiving" {
        ("source", &grant.source, "file_read")
    } else if state_name == "ready" {
        ("destination", &grant.destination, "file_write")
    } else {
        return Err(grants::conflict());
    };
    let ExecutionDestination::Environment { id: environment } = destination else {
        return Err(ApiError::Conflict(
            "this transfer leg is handled by its original local device".into(),
        ));
    };
    let original=sqlx::query("SELECT * FROM enterprise_transfer_environment_operations WHERE transfer_id=$1 AND leg=$2 FOR UPDATE").bind(id).bind(leg).fetch_optional(&mut *tx).await?;
    let mut submit = false;
    let operation_id: Uuid;
    let mut task;
    let pins: BTreeMap<Uuid, EnvironmentPin> = serde_json::from_value(row.get("environment_pins"))?;
    let pin = pins.get(environment).ok_or_else(grants::denied)?;
    if let Some(original) = original {
        operation_id = original.get("request_id");
        let request: Value = original.get("original_request");
        task = request["task_input"].clone();
    } else {
        current(&mut tx, row.get("grant_id"), p.user_id, r.expected_revision).await?;
        operation_id = Uuid::new_v4();
        submit = true;
        task = json!({"operation":operation,"relative_path":row.get::<String,_>("relative_path"),"transfer_request_id":row.get::<Uuid,_>("request_id"),"transfer_grant_id":row.get::<Uuid,_>("grant_id"),"expected_transfer_revision":r.expected_revision,"expected_sha256":if leg=="source"{Some(row.get::<String,_>("sha256"))}else{row.get::<Option<String>,_>("original_destination_sha256")}});
        if leg == "destination" {
            let sealed = sealed_from_row(&row)?;
            let bytes = state
                .auth_secrets
                .as_ref()
                .ok_or_else(grants::denied)?
                .open_for_user(p.user_id, &sealed)?;
            if bytes.len() > 256 * 1024
                || bytes.len() as i64 != row.get::<i64, _>("size")
                || hex::encode(sha2::Sha256::digest(&bytes)) != row.get::<String, _>("sha256")
            {
                return Err(grants::conflict());
            }
            task["content_base64"] = json!(STANDARD.encode(bytes));
        }
        let session = crate::organisation_mfa::session_in_tx(
            &state,
            &mut tx,
            p.user_id,
            p.session_id.ok_or_else(grants::denied)?,
            false,
        )
        .await?;
        let origin: Option<Value> = session.get("auth_origin");
        let management = origin
            .as_ref()
            .filter(|o| o["kind"] == "instance_launch")
            .and_then(|o| o["management_session_id"].as_str())
            .ok_or_else(grants::denied)?;
        let mut metadata = json!({"environment_id":environment,"management_session_id":management,"task_input":task});
        metadata["task_input"]
            .as_object_mut()
            .ok_or_else(grants::conflict)?
            .remove("content_base64");
        sqlx::query("INSERT INTO enterprise_transfer_environment_operations(transfer_id,leg,request_id,original_request,request_sha256,state) VALUES($1,$2,$3,$4,$5,'admitted')").bind(id).bind(leg).bind(operation_id).bind(metadata).bind(grants::digest(&task)?).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let payload = crate::enterprise_workflows::Payload::Environment {
        expected_environment_revision: pin.revision,
        profile_sha256: pin.profile_sha256.clone(),
        task_input: task,
    };
    let full = crate::enterprise_workflows::remote(
        &state,
        &p,
        id,
        operation_id,
        destination,
        &payload,
        submit,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    grants::lock_chat(&mut tx, p.user_id, grant.chat_id, project).await?;
    let row = sqlx::query(
        "SELECT * FROM enterprise_transfers WHERE id=$1 AND owner_user_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(p.user_id)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(original) = grants::original_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "transfer_environment_reconcile",
        id,
        &request_sha,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(Json(original));
    }
    if full["request_id"] != json!(operation_id)
        || full["workflow_id"] != json!(id)
        || full["step_id"] != json!(operation_id)
    {
        return Err(grants::conflict());
    }
    let terminal = matches!(
        full["state"].as_str(),
        Some("completed" | "failed" | "halted")
    );
    let envelope = &full["result"];
    let result = &envelope["file_result"];
    let mut public = full.clone();
    if let Some(result) = public["result"]["file_result"].as_object_mut() {
        result.remove("content_base64");
    }
    if full["state"] == "completed" {
        if envelope["original_runtime_verified"] != true
            || envelope["delivery_complete"] != true
            || envelope["exit_code"] != 0
            || result["operation"] != operation
            || result["relative_path"] != row.get::<String, _>("relative_path")
            || result["transfer_request_id"] != json!(row.get::<Uuid, _>("request_id"))
            || result["transfer_grant_id"] != json!(row.get::<Uuid, _>("grant_id"))
            || result["expected_transfer_revision"] != r.expected_revision
        {
            return Err(grants::conflict());
        }
        if leg == "source" {
            let bytes = STANDARD
                .decode(
                    result["content_base64"]
                        .as_str()
                        .ok_or_else(grants::conflict)?,
                )
                .map_err(|_| grants::conflict())?;
            if bytes.len() > 256 * 1024
                || bytes.len() as i64 != row.get::<i64, _>("size")
                || hex::encode(sha2::Sha256::digest(&bytes)) != row.get::<String, _>("sha256")
            {
                return Err(grants::conflict());
            }
            let sealed = state
                .auth_secrets
                .as_ref()
                .ok_or_else(grants::denied)?
                .seal_for_user(p.user_id, &bytes)?;
            sqlx::query("UPDATE enterprise_transfers SET ciphertext=$2,encrypted_data_key=$3,nonce=$4,wrap_nonce=$5,state='ready' WHERE id=$1 AND state='receiving'").bind(id).bind(sealed.ciphertext).bind(sealed.encrypted_data_key).bind(sealed.nonce.as_slice()).bind(sealed.wrap_nonce.as_slice()).execute(&mut *tx).await?;
        } else {
            let conflict = result["conflict"].as_bool().ok_or_else(grants::conflict)?;
            let after = result["destination_after_sha256"]
                .as_str()
                .ok_or_else(grants::conflict)?;
            if !grants::sha(after) || !conflict && after != row.get::<String, _>("sha256") {
                return Err(grants::conflict());
            }
            sqlx::query("UPDATE enterprise_transfers SET state=$2,receipt=$3 WHERE id=$1 AND state='ready' AND receipt IS NULL").bind(id).bind(if conflict{"conflict"}else{"completed"}).bind(&public).execute(&mut *tx).await?;
        }
    }
    if terminal {
        let raw = serde_json::to_vec(&full)?;
        if raw.len() > 2 * 1024 * 1024 {
            return Err(grants::conflict());
        }
        let original_bytes:i64=sqlx::query_scalar("SELECT receipt_bytes FROM enterprise_transfer_environment_operations WHERE transfer_id=$1 AND leg=$2 FOR UPDATE").bind(id).bind(leg).fetch_one(&mut *tx).await?;
        if original_bytes == 0 {
            crate::governance::enforce_storage_quota_tx(
                &mut tx,
                "user",
                p.user_id,
                raw.len() as u64,
            )
            .await?;
            let sealed = state
                .auth_secrets
                .as_ref()
                .ok_or_else(grants::denied)?
                .seal_for_user(p.user_id, &raw)?;
            sqlx::query("UPDATE enterprise_transfer_environment_operations SET state='completed',receipt=$3,receipt_ciphertext=$4,receipt_encrypted_data_key=$5,receipt_nonce=$6,receipt_wrap_nonce=$7,receipt_bytes=$8 WHERE transfer_id=$1 AND leg=$2 AND receipt IS NULL").bind(id).bind(leg).bind(&public).bind(sealed.ciphertext).bind(sealed.encrypted_data_key).bind(sealed.nonce.as_slice()).bind(sealed.wrap_nonce.as_slice()).bind(raw.len() as i64).execute(&mut *tx).await?;
        }
    }
    let receipt = json!({"format":1,"id":id,"request_id":r.request_id,"leg":leg,"original_operation_id":operation_id,"original":public,"execution_replayed":false});
    grants::record_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "transfer_environment_reconcile",
        id,
        &request_sha,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
fn sealed_from_row(row: &sqlx::postgres::PgRow) -> Result<SealedValue, ApiError> {
    Ok(SealedValue {
        ciphertext: row.get("ciphertext"),
        encrypted_data_key: row.get("encrypted_data_key"),
        nonce: row
            .get::<Vec<u8>, _>("nonce")
            .try_into()
            .map_err(|_| grants::conflict())?,
        wrap_nonce: row
            .get::<Vec<u8>, _>("wrap_nonce")
            .try_into()
            .map_err(|_| grants::conflict())?,
    })
}
