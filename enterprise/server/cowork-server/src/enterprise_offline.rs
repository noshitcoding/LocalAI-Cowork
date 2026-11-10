//! Bounded original-device offline leases and immutable reconnect receipts.
//! Late/revoked outputs are retained as facts and never authorize another run.
use crate::{auth::ExecutorPrincipal, enterprise_grants as grants, error::ApiError, AppState};
use axum::{
    extract::{Extension, Path, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use cowork_contracts::{RunEventKind, RunSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permit {
    request_id: Uuid,
    lease_token: Uuid,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    sequence: i64,
    event_id: Uuid,
    kind: RunEventKind,
    payload: Value,
    created_at: DateTime<Utc>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reconcile {
    request_id: Uuid,
    events: Vec<Event>,
    #[serde(default)]
    finished: bool,
    #[serde(default)]
    result: Option<Value>,
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/agent/executors/{device}/runs/{run}/enterprise/offline-permit",
            post(issue),
        )
        .route(
            "/agent/executors/{device}/enterprise/offline-permits/{permit}",
            get(read),
        )
        .route(
            "/agent/executors/{device}/enterprise/offline-permits/{permit}/reconcile",
            post(reconcile),
        )
}
async fn issue(
    State(state): State<AppState>,
    Path((device, run)): Path<(Uuid, Uuid)>,
    Extension(p): Extension<ExecutorPrincipal>,
    Json(r): Json<Permit>,
) -> Result<Json<Value>, ApiError> {
    if r.request_id.is_nil() {
        return Err(grants::conflict());
    }
    let mut tx = state.pool.begin().await?;
    grants::credential(&mut tx, &p, device).await?;
    let current = crate::db::verify_lease(&mut tx, run, device, r.lease_token).await?;
    grants::execution(&mut tx, &current).await?;
    if let Some(receipt)=sqlx::query_scalar("SELECT receipt FROM enterprise_offline_permits WHERE run_id=$1 AND original_lease_token=$2 AND credential_id=$3").bind(run).bind(r.lease_token).bind(p.credential_id).fetch_optional(&mut *tx).await?{tx.commit().await?;return Ok(Json(receipt))}
    let grant = current
        .spec
        .input
        .get("_enterprise_grant")
        .ok_or_else(grants::denied)?;
    let grant_id: Uuid = serde_json::from_value(grant["grant_id"].clone())?;
    let revision = grant["revision"].as_i64().ok_or_else(grants::denied)?;
    let seconds = grant["offline_seconds"]
        .as_i64()
        .filter(|s| *s > 0 && *s <= 3600)
        .ok_or_else(grants::denied)?;
    let until:DateTime<Utc>=sqlx::query_scalar("SELECT LEAST(g.expires_at,s.expires_at,clock_timestamp()+make_interval(secs=>$2)) FROM enterprise_device_grants g JOIN auth_sessions s ON s.id=g.auth_session_id AND s.user_id=g.owner_user_id WHERE g.id=$1 AND g.state='active' AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp()")
  .bind(grant_id).bind(seconds as f64).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    if until <= Utc::now() + chrono::Duration::seconds(1) {
        return Err(grants::denied());
    }
    let spec_sha = grants::digest(&current.spec)?;
    let id = Uuid::new_v4();
    let receipt = json!({"format":1,"permit_id":id,"request_id":r.request_id,"run_id":run,"device_id":device,"grant_id":grant_id,"grant_revision":revision,"original_spec_sha256":spec_sha,"original_lease_token":r.lease_token,"valid_until":until,"max_events":grant["max_offline_events"]});
    sqlx::query("INSERT INTO enterprise_offline_permits(id,run_id,device_id,credential_id,grant_id,grant_revision,original_lease_token,original_spec_sha256,valid_until,receipt) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
  .bind(id).bind(run).bind(device).bind(p.credential_id).bind(grant_id).bind(revision).bind(r.lease_token).bind(spec_sha).bind(until).bind(&receipt).execute(&mut *tx).await?;
    grants::writer(&mut tx).await?;
    sqlx::query("UPDATE runs SET lease_expires_at=GREATEST(lease_expires_at,$4) WHERE id=$1 AND lease_owner=$2 AND lease_token=$3").bind(run).bind(device).bind(r.lease_token).bind(until).execute(&mut *tx).await?;
    sqlx::query("UPDATE enterprise_workflow_steps SET state='offline_running' WHERE child_run_id=$1 AND state='running'").bind(run).execute(&mut *tx).await?;
    grants::credential(&mut tx, &p, device).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
async fn read(
    State(state): State<AppState>,
    Path((device, id)): Path<(Uuid, Uuid)>,
    Extension(p): Extension<ExecutorPrincipal>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    grants::credential(&mut tx, &p, device).await?;
    let permit:Value=sqlx::query_scalar("SELECT receipt FROM enterprise_offline_permits WHERE id=$1 AND device_id=$2 AND credential_id=$3").bind(id).bind(device).bind(p.credential_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    let receipts:Vec<Value>=sqlx::query_scalar("SELECT receipt FROM enterprise_execution_operations WHERE kind='offline_reconcile' AND target_id=$1 ORDER BY created_at").bind(id).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"permit":permit,"reconciliations":receipts})))
}
async fn reconcile(
    State(state): State<AppState>,
    Path((device, id)): Path<(Uuid, Uuid)>,
    Extension(p): Extension<ExecutorPrincipal>,
    Json(r): Json<Reconcile>,
) -> Result<Json<Value>, ApiError> {
    if r.request_id.is_nil()
        || r.events.len() > 1000
        || serde_json::to_vec(&r)?.len() > 2 * 1024 * 1024
    {
        return Err(ApiError::Unprocessable(
            "bounded original offline event batch required".into(),
        ));
    }
    let request_sha = grants::digest(&r)?;
    let mut tx = state.pool.begin().await?;
    let owner = grants::credential(&mut tx, &p, device).await?;
    let row=sqlx::query("SELECT * FROM enterprise_offline_permits WHERE id=$1 AND device_id=$2 AND credential_id=$3 FOR UPDATE").bind(id).bind(device).bind(p.credential_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    if let Some(original) = grants::original_operation(
        &mut tx,
        owner,
        r.request_id,
        "offline_reconcile",
        id,
        &request_sha,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(Json(original));
    }
    let run: Uuid = row.get("run_id");
    if row
        .get::<Option<DateTime<Utc>>, _>("reconciled_at")
        .is_some()
    {
        return Err(ApiError::Conflict("the original offline completion is already immutable; recover its original request receipt".into()));
    }
    let spec: RunSpec = serde_json::from_value(
        sqlx::query_scalar("SELECT spec FROM runs WHERE id=$1 AND creator_user_id=$2")
            .bind(run)
            .bind(owner)
            .fetch_one(&mut *tx)
            .await?,
    )?;
    if grants::digest(&spec)? != row.get::<String, _>("original_spec_sha256") {
        return Err(grants::conflict());
    }
    let original: Value = row.get("receipt");
    let max = original["max_events"]
        .as_i64()
        .ok_or_else(grants::conflict)?;
    let until: DateTime<Utc> = row.get("valid_until");
    let mut sequence: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(sequence),0) FROM enterprise_offline_events WHERE permit_id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    for event in &r.events {
        if event.sequence < 1
            || event.sequence > max
            || event.event_id.is_nil()
            || event.created_at > until
            || serde_json::to_vec(&event.payload)?.len() > 128 * 1024
        {
            return Err(ApiError::Unprocessable(
                "offline event does not belong to the bounded original permit".into(),
            ));
        }
        let source = serde_json::to_value(event)?;
        let sha = grants::digest(&source)?;
        if let Some(old)=sqlx::query("SELECT event_id,source_sha256 FROM enterprise_offline_events WHERE permit_id=$1 AND sequence=$2").bind(id).bind(event.sequence).fetch_optional(&mut *tx).await?{
   if old.get::<Uuid,_>("event_id")!=event.event_id||old.get::<String,_>("source_sha256")!=sha{return Err(grants::conflict())}continue;
  }
        if event.sequence != sequence + 1
            || row
                .get::<Option<DateTime<Utc>>, _>("reconciled_at")
                .is_some()
        {
            return Err(grants::conflict());
        }
        let receipt = json!({"format":1,"permit_id":id,"sequence":event.sequence,"event_id":event.event_id,"source_sha256":sha,"retained_as_original_fact":true});
        sqlx::query("INSERT INTO enterprise_offline_events(permit_id,sequence,event_id,source_sha256,original,receipt) VALUES($1,$2,$3,$4,$5,$6)").bind(id).bind(event.sequence).bind(event.event_id).bind(sha).bind(source).bind(receipt).execute(&mut *tx).await?;
        sequence = event.sequence;
    }
    let current = grants::active(
        &mut tx,
        row.get("grant_id"),
        owner,
        spec.thread_id,
        device,
        row.get("grant_revision"),
    )
    .await
    .is_ok();
    let receipt = json!({"format":1,"permit_id":id,"request_id":r.request_id,"request_sha256":request_sha,"run_id":run,"last_sequence":sequence,"finished":r.finished,"original_result":r.result,"retained_as_original_fact":true,"current_grant":current,"run_replayed":false,"execution_stopped_claimed":false});
    grants::record_operation(
        &mut tx,
        owner,
        r.request_id,
        "offline_reconcile",
        id,
        &request_sha,
        &receipt,
    )
    .await?;
    if r.finished {
        sqlx::query("UPDATE enterprise_offline_permits SET reconciled_at=COALESCE(reconciled_at,clock_timestamp()) WHERE id=$1").bind(id).execute(&mut *tx).await?;
        // Reconnect never invents a physical stop or retries the old model/tool.
        // The original normal completion channel may settle a still-valid lease.
        sqlx::query("UPDATE enterprise_workflow_steps SET state=CASE WHEN $2 THEN 'running' ELSE 'held' END,original_effects_unknown=NOT $2 WHERE child_run_id=$1 AND state='offline_running'").bind(run).bind(current&&until>Utc::now()).execute(&mut *tx).await?;
    }
    grants::credential(&mut tx, &p, device).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
