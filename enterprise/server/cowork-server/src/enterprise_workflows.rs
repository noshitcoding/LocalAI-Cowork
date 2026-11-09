//! Durable DAG coordination over original device runs and original environment
//! jobs. The coordinator never repeats a remotely admitted process operation.
use crate::{auth::Principal, enterprise_grants as grants, error::ApiError, AppState};
use axum::http::HeaderMap;
use axum::{
    extract::{Extension, Path, State},
    routing::{get, post},
    Json, Router,
};
use cowork_contracts::{
    enterprise::{ExecutionDestination, WorkflowRun},
    CreateRunRequest, ExecutorTarget,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Postgres, Row, Transaction};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Payload {
    Device {
        request: CreateRunRequest,
        grant_revision: i64,
    },
    Environment {
        expected_environment_revision: i64,
        profile_sha256: String,
        task_input: Value,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
    request_id: Uuid,
    project_id: Uuid,
    workflow: WorkflowRun,
    payloads: BTreeMap<Uuid, Payload>,
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/enterprise/context", get(context))
        .route("/enterprise/workflows", post(create).get(list))
        .route("/enterprise/workflows/{id}", get(read))
        .route("/enterprise/workflows/{id}/steps/{step}", get(read_step))
        .route("/enterprise/workflows/{id}/reconcile", post(reconcile))
        .route("/enterprise/workflows/{id}/stop", post(stop))
        .route("/enterprise/workflows/{id}/continue", post(resume))
        .route("/enterprise/workflows/{id}/reassign", post(reassign))
}
pub fn private_router() -> Router<AppState> {
    Router::new().route(
        "/enterprise/workflows/{id}/environment-authority",
        post(environment_authority),
    )
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvironmentAuthority {
    organisation_id: Uuid,
    instance_id: Uuid,
    execution_epoch: Uuid,
    user_id: Uuid,
    management_session_id: Uuid,
    environment_id: Uuid,
    workflow_id: Uuid,
    step_id: Uuid,
    request_id: Uuid,
}
async fn environment_authority(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(r): Json<EnvironmentAuthority>,
) -> Result<Json<Value>, ApiError> {
    use subtle::ConstantTimeEq;
    let (org, instance, epoch) = scope()?;
    let key = crate::fleet::secret("COWORK_FLEET_KEY_FILE")?;
    let supplied = headers
        .get("x-cowork-instance-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !bool::from(key.as_bytes().ct_eq(supplied.as_bytes()))
        || r.organisation_id != org
        || r.instance_id != instance
        || r.execution_epoch != epoch
        || r.workflow_id != id
        || r.request_id != r.step_id
    {
        return Err(grants::denied());
    }
    state
        .ensure_api_active()
        .await
        .map_err(ApiError::Internal)?;
    let mut tx = state.pool.begin().await?;
    let row=sqlx::query("SELECT w.chat_id,w.project_id,s.destination,s.dispatch_auth_session_id FROM enterprise_workflows w JOIN enterprise_workflow_steps s ON s.workflow_id=w.id WHERE w.id=$1 AND w.owner_user_id=$2 AND w.state='active' AND s.id=$3 AND s.remote_request_id=$3 AND s.state IN ('admitted','running') AND NOT s.stop_admitted FOR SHARE OF w,s").bind(id).bind(r.user_id).bind(r.step_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    let destination: ExecutionDestination = serde_json::from_value(row.get("destination"))?;
    if !matches!(destination,ExecutionDestination::Environment{id} if id==r.environment_id) {
        return Err(grants::denied());
    }
    let p = Principal {
        user_id: r.user_id,
        session_id: row.get("dispatch_auth_session_id"),
        bootstrap: false,
    };
    grants::actor(&state, &mut tx, &p).await?;
    grants::lock_chat(
        &mut tx,
        p.user_id,
        row.get("chat_id"),
        row.get("project_id"),
    )
    .await?;
    let session = crate::organisation_mfa::session_in_tx(
        &state,
        &mut tx,
        p.user_id,
        p.session_id.ok_or_else(grants::denied)?,
        false,
    )
    .await?;
    let origin: Option<Value> = session.get("auth_origin");
    if origin.as_ref().is_none_or(|o| {
        o["kind"] != "instance_launch"
            || o["management_session_id"] != json!(r.management_session_id)
    }) {
        return Err(grants::denied());
    }
    tx.commit().await?;
    Ok(Json(json!({"format":1,"authorized":true,"original":r})))
}
pub(crate) async fn context(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let (org, instance, epoch) = scope()?;
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
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
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or_else(grants::denied)?;
    tx.commit().await?;
    let base = std::env::var("COWORK_FLEET_MANAGEMENT_URL").map_err(|_| grants::denied())?;
    let client = crate::management_transport::client(&base, Duration::from_secs(10))?;
    let response = client
        .get(format!(
            "{}/management/runner/v1/environment-execution/destinations",
            base.trim_end_matches('/')
        ))
        .query(&[
            ("organisation_id", org.to_string()),
            ("instance_id", instance.to_string()),
            ("user_id", p.user_id.to_string()),
            ("management_session_id", management.to_string()),
        ])
        .header(
            "x-cowork-instance-key",
            crate::fleet::secret("COWORK_FLEET_KEY_FILE")?,
        )
        .send()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    if !response.status().is_success() {
        return Err(ApiError::Unavailable(
            "current environment destinations are unavailable".into(),
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    if bytes.len() > 128 * 1024 {
        return Err(grants::conflict());
    }
    let environments: Value = serde_json::from_slice(&bytes)?;
    if environments.as_array().map_or(true, |v| v.len() > 100) {
        return Err(grants::conflict());
    }
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"format":1,"organisation_id":org,"instance_id":instance,"execution_epoch":epoch,"environments":environments}),
    ))
}
fn scope() -> Result<(Uuid, Uuid, Uuid), ApiError> {
    let parse = |key| {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<Uuid>().ok())
            .filter(|id| !id.is_nil())
            .ok_or_else(grants::denied)
    };
    if !crate::fleet::enabled() {
        return Err(ApiError::Unavailable(
            "distributed workflows require the configured original enterprise instance".into(),
        ));
    }
    Ok((
        parse("COWORK_ORGANISATION_ID")?,
        parse("COWORK_INSTANCE_ID")?,
        parse("COWORK_EXECUTION_EPOCH")?,
    ))
}
pub(crate) async fn original_actor(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Principal, ApiError> {
    let row=sqlx::query("SELECT w.owner_user_id,COALESCE((SELECT c.auth_session_id FROM enterprise_workflow_continuations c WHERE c.workflow_id=w.id AND c.revision<=w.revision ORDER BY c.revision DESC LIMIT 1),w.auth_session_id) AS auth_session_id,w.chat_id,w.project_id FROM enterprise_workflows w WHERE w.id=$1 FOR SHARE OF w").bind(id).fetch_optional(&mut **tx).await?.ok_or_else(grants::denied)?;
    let p = Principal {
        user_id: row.get("owner_user_id"),
        session_id: Some(row.get("auth_session_id")),
        bootstrap: false,
    };
    grants::actor(state, tx, &p).await?;
    crate::organisation_mfa::optional_scope(
        p.user_id,
        p.session_id,
        crate::organization::ensure_thread_role(
            &state.pool,
            p.user_id,
            row.get("project_id"),
            row.get("chat_id"),
            cowork_contracts::ProjectRole::Runner,
        ),
    )
    .await?;
    grants::lock_chat(tx, p.user_id, row.get("chat_id"), row.get("project_id")).await?;
    Ok(p)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reassign {
    request_id: Uuid,
    expected_revision: i64,
    step_id: Uuid,
    destination: ExecutionDestination,
    #[serde(default)]
    payload: Option<Payload>,
    #[serde(default)]
    depends_on: Option<BTreeSet<Uuid>>,
}
async fn reassign(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(p): Extension<Principal>,
    Json(r): Json<Reassign>,
) -> Result<Json<Value>, ApiError> {
    let sha = grants::digest(&(id, &r))?;
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    grants::admit_request(
        &mut tx,
        p.user_id,
        r.request_id,
        "workflow_reassign",
        id,
        &sha,
    )
    .await?;
    let w=sqlx::query("SELECT revision,state,project_id,chat_id FROM enterprise_workflows WHERE id=$1 AND owner_user_id=$2 FOR UPDATE").bind(id).bind(p.user_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    if let Some(receipt) = grants::original_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "workflow_reassign",
        id,
        &sha,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(Json(receipt));
    }
    if w.get::<i64, _>("revision") != r.expected_revision || w.get::<String, _>("state") != "held" {
        return Err(grants::conflict());
    }
    let chat = w.get("chat_id");
    let project = w.get("project_id");
    grants::lock_chat(&mut tx, p.user_id, chat, project).await?;
    let original:Value=sqlx::query_scalar("SELECT to_jsonb(s) FROM enterprise_workflow_steps s WHERE workflow_id=$1 AND id=$2 AND state='pending' AND child_run_id IS NULL AND remote_request_id IS NULL AND NOT original_effects_unknown FOR UPDATE").bind(id).bind(r.step_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    let mut payload: Payload = if let Some(payload) = r.payload {
        payload
    } else {
        serde_json::from_value(original["request"].clone())?
    };
    let (grant_id, grant_revision) = match (&r.destination, &mut payload) {
        (
            ExecutionDestination::Device {
                id: device,
                grant_id,
            },
            Payload::Device {
                request,
                grant_revision,
            },
        ) => {
            let revision:i64=sqlx::query_scalar("SELECT revision FROM enterprise_device_grants WHERE id=$1 AND owner_user_id=$2 AND device_id=$3 AND chat_id=$4 AND state='active' FOR SHARE").bind(grant_id).bind(p.user_id).bind(device).bind(chat).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
            let grant =
                grants::active(&mut tx, *grant_id, p.user_id, chat, *device, revision).await?;
            if request.project_id != project
                || request.thread_id != chat
                || request.required_capabilities.iter().any(|c| {
                    !grant["capabilities"]
                        .as_array()
                        .is_some_and(|v| v.contains(&json!(c.0)))
                })
            {
                return Err(grants::denied());
            }
            request.executor_target = ExecutorTarget::PersonalDevice { device_id: *device };
            request.idempotency_key = format!("enterprise:{id}:{}", r.step_id);
            request
                .input
                .as_object_mut()
                .ok_or_else(grants::conflict)?
                .insert("_enterprise_grant".into(), grant);
            *grant_revision = revision;
            (Some(*grant_id), Some(revision))
        }
        (
            ExecutionDestination::Environment { id: environment },
            Payload::Environment {
                expected_environment_revision,
                profile_sha256,
                task_input,
            },
        ) => {
            if task_input["operation"] != "process" {
                return Err(grants::denied());
            }
            tx.commit().await?;
            let Json(context) = context(State(state.clone()), Extension(p.clone())).await?;
            let actual = context["environments"]
                .as_array()
                .ok_or_else(grants::denied)?
                .iter()
                .find(|v| v["id"] == json!(environment))
                .ok_or_else(grants::denied)?;
            *expected_environment_revision =
                actual["revision"].as_i64().ok_or_else(grants::denied)?;
            *profile_sha256 = actual["profile_sha256"]
                .as_str()
                .ok_or_else(grants::denied)?
                .to_owned();
            tx = state.pool.begin().await?;
            grants::actor(&state, &mut tx, &p).await?;
            grants::admit_request(
                &mut tx,
                p.user_id,
                r.request_id,
                "workflow_reassign",
                id,
                &sha,
            )
            .await?;
            let revision:i64=sqlx::query_scalar("SELECT revision FROM enterprise_workflows WHERE id=$1 AND owner_user_id=$2 AND state='held' FOR UPDATE").bind(id).bind(p.user_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
            if revision != r.expected_revision {
                return Err(grants::conflict());
            }
            grants::lock_chat(&mut tx, p.user_id, chat, project).await?;
            (None, None)
        }
        _ => return Err(grants::conflict()),
    };
    let dependencies = r
        .depends_on
        .unwrap_or(serde_json::from_value(original["dependencies"].clone())?);
    let mut graph: BTreeMap<Uuid, BTreeSet<Uuid>> = sqlx::query(
        "SELECT id,dependencies FROM enterprise_workflow_steps WHERE workflow_id=$1 FOR SHARE",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|row| {
        Ok((
            row.get("id"),
            serde_json::from_value(row.get("dependencies"))?,
        ))
    })
    .collect::<Result<_, serde_json::Error>>()?;
    if dependencies.contains(&r.step_id) || dependencies.iter().any(|d| !graph.contains_key(d)) {
        return Err(grants::conflict());
    }
    graph.insert(r.step_id, dependencies.clone());
    let mut visited = BTreeSet::new();
    while visited.len() < graph.len() {
        let old = visited.len();
        for (node, deps) in &graph {
            if deps.is_subset(&visited) {
                visited.insert(*node);
            }
        }
        if old == visited.len() {
            return Err(grants::conflict());
        }
    }
    let payload = serde_json::to_value(payload)?;
    let replacement =
        json!({"destination":r.destination,"request":payload,"dependencies":dependencies});
    let receipt = json!({"format":1,"id":id,"step_id":r.step_id,"request_id":r.request_id,"revision":r.expected_revision+1,"original_step_sha256":grants::digest(&original)?,"replacement_sha256":grants::digest(&replacement)?,"original_step_never_dispatched":true});
    sqlx::query("INSERT INTO enterprise_workflow_reassignments(workflow_id,revision,step_id,request_id,auth_session_id,original_step,replacement,receipt) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(id).bind(r.expected_revision+1).bind(r.step_id).bind(r.request_id).bind(p.session_id).bind(original).bind(&replacement).bind(&receipt).execute(&mut *tx).await?;
    if sqlx::query("UPDATE enterprise_workflow_steps SET destination=$3,request=$4,request_sha256=$5,device_grant_id=$6,device_grant_revision=$7,dependencies=$8 WHERE workflow_id=$1 AND id=$2 AND state='pending' AND child_run_id IS NULL AND remote_request_id IS NULL AND NOT original_effects_unknown").bind(id).bind(r.step_id).bind(&replacement["destination"]).bind(&payload).bind(grants::digest(&payload)?).bind(grant_id).bind(grant_revision).bind(serde_json::to_value(dependencies)?).execute(&mut *tx).await?.rows_affected()!=1{return Err(grants::conflict())}
    sqlx::query("UPDATE enterprise_workflows SET revision=revision+1 WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    grants::record_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "workflow_reassign",
        id,
        &sha,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
async fn create(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(r): Json<Create>,
) -> Result<Json<Value>, ApiError> {
    let (org, instance, _) = scope()?;
    if state.management_mode
        || r.request_id.is_nil()
        || r.workflow.id.is_nil()
        || r.workflow.organisation_id != org
        || r.workflow.instance_id != instance
    {
        return Err(grants::denied());
    }
    r.workflow.validate().map_err(ApiError::Unprocessable)?;
    if r.payloads.len() != r.workflow.steps.len()
        || r.workflow
            .steps
            .iter()
            .any(|s| !r.payloads.contains_key(&s.id))
    {
        return Err(ApiError::Unprocessable(
            "each original step needs exactly one matching execution payload".into(),
        ));
    }
    let project = grants::chat(&state, &p, r.workflow.chat_id).await?;
    if project != r.project_id {
        return Err(grants::denied());
    }
    let request_sha = grants::digest(&r)?;
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "enterprise-workflow:{}:{}",
            p.user_id, r.request_id
        ))
        .execute(&mut *tx)
        .await?;
    if let Some(row)=sqlx::query("SELECT request_sha256,initial_receipt FROM enterprise_workflows WHERE owner_user_id=$1 AND request_id=$2").bind(p.user_id).bind(r.request_id).fetch_optional(&mut *tx).await?{
  if row.get::<String,_>("request_sha256")!=request_sha{return Err(grants::conflict())}let receipt=row.get("initial_receipt");tx.commit().await?;return Ok(Json(receipt))}
    grants::lock_chat(&mut tx, p.user_id, r.workflow.chat_id, project).await?;
    let mut approved: BTreeMap<Uuid, Value> = BTreeMap::new();
    for step in &r.workflow.steps {
        match (&step.destination, &r.payloads[&step.id]) {
            (
                ExecutionDestination::Device { id, grant_id },
                Payload::Device {
                    request,
                    grant_revision,
                },
            ) => {
                if request.project_id != project
                    || request.thread_id != r.workflow.chat_id
                    || request.executor_target
                        != (ExecutorTarget::PersonalDevice { device_id: *id })
                    || request.input.get("_enterprise_grant").is_some()
                {
                    return Err(grants::conflict());
                }
                let grant = grants::active(
                    &mut tx,
                    *grant_id,
                    p.user_id,
                    r.workflow.chat_id,
                    *id,
                    *grant_revision,
                )
                .await?;
                let caps: BTreeSet<String> = grant["capabilities"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                if !step.required_capabilities.is_subset(&caps)
                    || request
                        .required_capabilities
                        .iter()
                        .any(|c| !caps.contains(&c.0))
                {
                    return Err(grants::denied());
                }
                approved.insert(step.id, grant);
            }
            (
                ExecutionDestination::Environment { .. },
                Payload::Environment {
                    expected_environment_revision,
                    profile_sha256,
                    task_input,
                },
            ) => {
                if *expected_environment_revision < 1
                    || !grants::sha(profile_sha256)
                    || task_input["operation"] != "process"
                    || serde_json::to_vec(task_input)?.len() > 256 * 1024
                {
                    return Err(ApiError::Unprocessable("an original qualified environment and bounded process operation are required".into()));
                }
            }
            _ => {
                return Err(ApiError::Unprocessable(
                    "step payload and original destination differ".into(),
                ))
            }
        }
    }
    let initial = json!({"format":1,"id":r.workflow.id,"request_id":r.request_id,"request_sha256":request_sha,"revision":1,"state":"active","steps":r.workflow.steps.len()});
    grants::admit_request(
        &mut tx,
        p.user_id,
        r.request_id,
        "workflow_create",
        r.workflow.id,
        &request_sha,
    )
    .await?;
    sqlx::query("INSERT INTO enterprise_workflows(id,owner_user_id,auth_session_id,chat_id,project_id,request_id,request_sha256,original,initial_receipt,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'active')")
  .bind(r.workflow.id).bind(p.user_id).bind(p.session_id).bind(r.workflow.chat_id).bind(project).bind(r.request_id).bind(request_sha).bind(serde_json::to_value(&r)?).bind(&initial).execute(&mut *tx).await?;
    // Stable topological order also respects the existing per-chat queue order.
    let mut placed = BTreeSet::new();
    let mut ordinal = 0i64;
    while placed.len() < r.workflow.steps.len() {
        let old = placed.len();
        for step in &r.workflow.steps {
            if placed.contains(&step.id) || !step.depends_on.is_subset(&placed) {
                continue;
            }
            let mut payload = r.payloads[&step.id].clone();
            let (grant_id, grant_revision) = if let Payload::Device {
                request,
                grant_revision,
            } = &mut payload
            {
                request.idempotency_key = format!("enterprise:{}:{}", r.workflow.id, step.id);
                let object = request.input.as_object_mut().ok_or_else(|| {
                    ApiError::Unprocessable("device task input must be an object".into())
                })?;
                object.insert("_enterprise_grant".into(), approved[&step.id].clone());
                if let ExecutionDestination::Device { grant_id, .. } = step.destination {
                    (Some(grant_id), Some(*grant_revision))
                } else {
                    return Err(grants::conflict());
                }
            } else {
                (None, None)
            };
            let payload_value = serde_json::to_value(&payload)?;
            sqlx::query("INSERT INTO enterprise_workflow_steps(workflow_id,id,ordinal,dependencies,destination,request,request_sha256,state,device_grant_id,device_grant_revision) VALUES($1,$2,$3,$4,$5,$6,$7,'pending',$8,$9)")
   .bind(r.workflow.id).bind(step.id).bind(ordinal).bind(serde_json::to_value(&step.depends_on)?).bind(serde_json::to_value(&step.destination)?).bind(&payload_value).bind(grants::digest(&payload_value)?).bind(grant_id).bind(grant_revision).execute(&mut *tx).await?;
            placed.insert(step.id);
            ordinal += 1;
        }
        if old == placed.len() {
            return Err(grants::conflict());
        }
    }
    grants::actor(&state, &mut tx, &p).await?;
    grants::record_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "workflow_create",
        r.workflow.id,
        &initial["request_sha256"]
            .as_str()
            .ok_or_else(grants::conflict)?
            .to_owned(),
        &initial,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(initial))
}
async fn read(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    let header:Value=sqlx::query_scalar("SELECT to_jsonb(w)-'auth_session_id'-'original' FROM enterprise_workflows w WHERE id=$1 AND owner_user_id=$2").bind(id).bind(p.user_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    let project: Uuid = serde_json::from_value(header["project_id"].clone())?;
    let chat: Uuid = serde_json::from_value(header["chat_id"].clone())?;
    grants::lock_chat(&mut tx, p.user_id, chat, project).await?;
    let steps:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(s)-'request'-'receipt'||jsonb_build_object('has_original_receipt',s.receipt IS NOT NULL) FROM enterprise_workflow_steps s WHERE workflow_id=$1 ORDER BY ordinal").bind(id).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"workflow":header,"steps":steps})))
}
async fn read_step(
    State(state): State<AppState>,
    Path((id, step)): Path<(Uuid, Uuid)>,
    Extension(p): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    let w=sqlx::query("SELECT chat_id,project_id FROM enterprise_workflows WHERE id=$1 AND owner_user_id=$2 FOR SHARE").bind(id).bind(p.user_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    grants::lock_chat(&mut tx, p.user_id, w.get("chat_id"), w.get("project_id")).await?;
    let value=sqlx::query_scalar("SELECT to_jsonb(s)-'request' FROM enterprise_workflow_steps s WHERE workflow_id=$1 AND id=$2").bind(id).bind(step).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    tx.commit().await?;
    Ok(Json(value))
}
async fn reconcile(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(p): Extension<Principal>,
    Json(r): Json<grants::Stop>,
) -> Result<Json<Value>, ApiError> {
    let sha = grants::digest(&(id, &r))?;
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    grants::admit_request(
        &mut tx,
        p.user_id,
        r.request_id,
        "workflow_reconcile",
        id,
        &sha,
    )
    .await?;
    let w=sqlx::query("SELECT revision,state,chat_id,project_id FROM enterprise_workflows WHERE id=$1 AND owner_user_id=$2 FOR UPDATE").bind(id).bind(p.user_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    if let Some(original) = grants::original_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "workflow_reconcile",
        id,
        &sha,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(Json(original));
    }
    grants::lock_chat(&mut tx, p.user_id, w.get("chat_id"), w.get("project_id")).await?;
    if w.get::<i64, _>("revision") != r.expected_revision
        || !matches!(w.get::<String, _>("state").as_str(), "held" | "stopping")
    {
        return Err(grants::conflict());
    }
    let step=sqlx::query("SELECT id,destination,request FROM enterprise_workflow_steps WHERE workflow_id=$1 AND remote_request_id IS NOT NULL AND state IN ('admitted','running','held') ORDER BY ordinal LIMIT 1").bind(id).fetch_optional(&mut *tx).await?;
    tx.commit().await?;
    tick_one(&state, id).await?;
    if let Some(step) = step {
        let step_id = step.get("id");
        let destination = serde_json::from_value(step.get("destination"))?;
        let payload = serde_json::from_value(step.get("request"))?;
        let receipt = remote(&state, &p, id, step_id, &destination, &payload, false).await?;
        store_remote(&state, id, step_id, receipt).await?;
    }
    let mut tx = state.pool.begin().await?;
    grants::actor(&state, &mut tx, &p).await?;
    let w=sqlx::query("SELECT chat_id,project_id,revision FROM enterprise_workflows WHERE id=$1 AND owner_user_id=$2 FOR UPDATE").bind(id).bind(p.user_id).fetch_one(&mut *tx).await?;
    grants::lock_chat(&mut tx, p.user_id, w.get("chat_id"), w.get("project_id")).await?;
    if let Some(original) = grants::original_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "workflow_reconcile",
        id,
        &sha,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(Json(original));
    }
    let unsettled:i64=sqlx::query_scalar("SELECT COUNT(*) FROM enterprise_workflow_steps WHERE workflow_id=$1 AND (state IN ('admitted','running','offline_running','held') OR original_effects_unknown)").bind(id).fetch_one(&mut *tx).await?;
    sqlx::query("UPDATE enterprise_workflows SET revision=revision+1 WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let receipt = json!({"format":1,"id":id,"request_id":r.request_id,"revision":w.get::<i64,_>("revision")+1,"original_operations_unsettled":unsettled,"execution_replayed":false});
    grants::record_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        "workflow_reconcile",
        id,
        &sha,
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
    grants::actor(&state, &mut tx, &p).await?;
    let rows=sqlx::query_scalar("SELECT to_jsonb(w)-'auth_session_id'-'original' FROM enterprise_workflows w WHERE owner_user_id=$1 ORDER BY created_at DESC LIMIT 100").bind(p.user_id).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(rows))
}
async fn control(
    state: &AppState,
    p: &Principal,
    id: Uuid,
    r: grants::Stop,
    stop: bool,
) -> Result<Value, ApiError> {
    let request_sha = grants::digest(&(id, stop, &r))?;
    let mut tx = state.pool.begin().await?;
    grants::actor(state, &mut tx, p).await?;
    grants::admit_request(
        &mut tx,
        p.user_id,
        r.request_id,
        if stop {
            "workflow_stop"
        } else {
            "workflow_continue"
        },
        id,
        &request_sha,
    )
    .await?;
    let current=sqlx::query("SELECT revision,state,chat_id,project_id FROM enterprise_workflows WHERE id=$1 AND owner_user_id=$2 FOR UPDATE").bind(id).bind(p.user_id).fetch_optional(&mut *tx).await?.ok_or_else(grants::denied)?;
    let kind = if stop {
        "workflow_stop"
    } else {
        "workflow_continue"
    };
    if let Some(original) =
        grants::original_operation(&mut tx, p.user_id, r.request_id, kind, id, &request_sha).await?
    {
        tx.commit().await?;
        return Ok(original);
    }
    grants::lock_chat(
        &mut tx,
        p.user_id,
        current.get("chat_id"),
        current.get("project_id"),
    )
    .await?;
    if current.get::<i64, _>("revision") != r.expected_revision {
        return Err(grants::conflict());
    }
    if !stop {
        // A continuation selects only never-dispatched remaining steps. Original
        // admitted/held operations keep their identifiers and uncertain effects.
        if current.get::<String, _>("state") != "held" {
            return Err(grants::conflict());
        }
        let unsafe_steps:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM enterprise_workflow_steps WHERE workflow_id=$1 AND (state IN ('admitted','running','offline_running','held') OR original_effects_unknown))").bind(id).fetch_one(&mut *tx).await?;
        if unsafe_steps {
            return Err(ApiError::Conflict(
                "reconcile original admitted operations before continuing remaining work".into(),
            ));
        }
    }
    let next = if stop { "stopping" } else { "active" };
    if stop {
        request_stop_in_tx(&mut tx, id).await?;
    }
    // A fresh explicit continuation is additive; initial receipt and original
    // creator/request bytes remain immutable.
    sqlx::query("UPDATE enterprise_workflows SET state=$2,revision=revision+1,stop_auth_session_id=CASE WHEN $2='stopping' THEN $3 ELSE stop_auth_session_id END WHERE id=$1")
        .bind(id)
        .bind(next)
        .bind(p.session_id)
        .execute(&mut *tx)
        .await?;
    let receipt = json!({"format":1,"id":id,"request_id":r.request_id,"revision":r.expected_revision+1,"state":next,"physical_stop_confirmed":false});
    if !stop {
        sqlx::query("INSERT INTO enterprise_workflow_continuations(workflow_id,revision,request_id,auth_session_id,receipt) VALUES($1,$2,$3,$4,$5)").bind(id).bind(r.expected_revision+1).bind(r.request_id).bind(p.session_id).bind(&receipt).execute(&mut *tx).await?;
    }
    grants::record_operation(
        &mut tx,
        p.user_id,
        r.request_id,
        kind,
        id,
        &request_sha,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(receipt)
}
pub(crate) async fn request_stop_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workflow: Uuid,
) -> Result<(), ApiError> {
    grants::writer(tx).await?;
    // Revocation requests a local stop without claiming that its process stopped
    // or releasing the original executor capacity, lease or workspace fence.
    let runs=sqlx::query_scalar::<_,Uuid>("UPDATE runs SET state='interrupted',revision=revision+1,safe_to_resume=false,updated_at=clock_timestamp() WHERE id IN (SELECT child_run_id FROM enterprise_workflow_steps WHERE workflow_id=$1) AND state IN ('running','waiting_approval','waiting_input') RETURNING id").bind(workflow).fetch_all(&mut **tx).await?;
    for run in runs {
        crate::db::append_event_tx(tx,run,cowork_contracts::RunEventKind::StateChanged,json!({"to":"interrupted","reason":"enterprise_stop_requested","physical_stop_confirmed":false,"safe_to_resume":false})).await?;
    }
    sqlx::query("UPDATE enterprise_workflow_steps SET state='canceled' WHERE workflow_id=$1 AND state='pending' AND child_run_id IS NULL AND remote_request_id IS NULL").bind(workflow).execute(&mut **tx).await?;
    Ok(())
}
async fn stop(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(p): Extension<Principal>,
    Json(r): Json<grants::Stop>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(control(&state, &p, id, r, true).await?))
}
async fn resume(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(p): Extension<Principal>,
    Json(r): Json<grants::Stop>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(control(&state, &p, id, r, false).await?))
}
pub(crate) async fn remote(
    state: &AppState,
    p: &Principal,
    workflow: Uuid,
    step: Uuid,
    destination: &ExecutionDestination,
    payload: &Payload,
    submit: bool,
) -> Result<Value, ApiError> {
    let (org, instance, epoch) = scope()?;
    let Payload::Environment {
        expected_environment_revision,
        profile_sha256,
        task_input,
    } = payload
    else {
        return Err(grants::conflict());
    };
    let ExecutionDestination::Environment { id } = destination else {
        return Err(grants::conflict());
    };
    let mut tx = state.pool.begin().await?;
    let session = crate::organisation_mfa::session_in_tx(
        state,
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
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or_else(grants::denied)?;
    grants::actor(state, &mut tx, p).await?;
    tx.commit().await?;
    let capability = match task_input["operation"].as_str() {
        Some("process") => "process.execute",
        Some("file_read") => "file.read",
        Some("file_write") => "file.write",
        _ => return Err(grants::denied()),
    };
    let body = json!({"format":1,"request_id":step,"workflow_id":workflow,"step_id":step,"organisation_id":org,"instance_id":instance,"execution_epoch":epoch,"user_id":p.user_id,"management_session_id":management,"environment_id":id,"expected_environment_revision":expected_environment_revision,"profile_sha256":profile_sha256,"task_input":task_input,"capabilities":[capability]});
    let base = std::env::var("COWORK_FLEET_MANAGEMENT_URL").map_err(|_| grants::denied())?;
    let client = crate::management_transport::client(&base, Duration::from_secs(15))?;
    let path = if submit {
        "environment-execution".into()
    } else {
        format!("environment-execution/{step}")
    };
    let request = if submit {
        client
            .post(format!(
                "{}/management/runner/v1/{path}",
                base.trim_end_matches('/')
            ))
            .json(&body)
    } else {
        client
            .get(format!(
                "{}/management/runner/v1/{path}",
                base.trim_end_matches('/')
            ))
            .query(&[
                ("organisation_id", org.to_string()),
                ("instance_id", instance.to_string()),
                ("user_id", p.user_id.to_string()),
                ("management_session_id", management.to_string()),
            ])
    };
    let response = request
        .header(
            "x-cowork-instance-key",
            crate::fleet::secret("COWORK_FLEET_KEY_FILE")?,
        )
        .send()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    if !response.status().is_success() {
        return Err(ApiError::Unavailable(format!(
            "original environment operation unconfirmed ({})",
            response.status()
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(ApiError::Unavailable(
            "environment receipt exceeds bound".into(),
        ));
    }
    Ok(serde_json::from_slice(&bytes)?)
}
async fn remote_stop(state: &AppState, p: &Principal, step: Uuid) -> Result<Value, ApiError> {
    let (org, instance, _) = scope()?;
    let mut tx = state.pool.begin().await?;
    grants::actor(state, &mut tx, p).await?;
    let session = crate::organisation_mfa::session_in_tx(
        state,
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
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or_else(grants::denied)?;
    tx.commit().await?;
    let base = std::env::var("COWORK_FLEET_MANAGEMENT_URL").map_err(|_| grants::denied())?;
    let client = crate::management_transport::client(&base, Duration::from_secs(15))?;
    let response = client
        .post(format!(
            "{}/management/runner/v1/environment-execution/{step}/stop",
            base.trim_end_matches('/')
        ))
        .query(&[
            ("organisation_id", org.to_string()),
            ("instance_id", instance.to_string()),
            ("user_id", p.user_id.to_string()),
            ("management_session_id", management.to_string()),
        ])
        .header(
            "x-cowork-instance-key",
            crate::fleet::secret("COWORK_FLEET_KEY_FILE")?,
        )
        .send()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    if !response.status().is_success() {
        return Err(ApiError::Unavailable(
            "original environment stop remains unconfirmed".into(),
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(grants::conflict());
    }
    Ok(serde_json::from_slice(&bytes)?)
}
async fn tick_one(state: &AppState, id: Uuid) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await?;
    let row =
        sqlx::query("SELECT state,owner_user_id,stop_auth_session_id FROM enterprise_workflows WHERE id=$1 FOR UPDATE SKIP LOCKED")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(row) = row else { return Ok(()) };
    let running = row.get::<String, _>("state") == "active";
    let steps = sqlx::query(
        "SELECT * FROM enterprise_workflow_steps WHERE workflow_id=$1 ORDER BY ordinal FOR UPDATE",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    // Original child responses are facts even if authority subsequently expired.
    for step in &steps {
        if let Some(child) = step.get::<Option<Uuid>, _>("child_run_id") {
            let run = sqlx::query("SELECT state,result,error,revision FROM runs WHERE id=$1")
                .bind(child)
                .fetch_one(&mut *tx)
                .await?;
            let state_name: String = run.get("state");
            let next = match state_name.as_str() {
                "completed" => Some("completed"),
                "failed" | "expired" => Some("failed"),
                "canceled" => Some("canceled"),
                "interrupted" => Some("held"),
                _ => None,
            };
            if let Some(next) = next {
                let receipt = json!({"source":"original_device_run","run_id":child,"revision":run.get::<i64,_>("revision"),"state":state_name,"result":run.get::<Option<Value>,_>("result"),"error":run.get::<Option<Value>,_>("error")});
                sqlx::query("UPDATE enterprise_workflow_steps SET state=$3,receipt=COALESCE(receipt,$4),original_effects_unknown=original_effects_unknown OR $5 WHERE workflow_id=$1 AND id=$2 AND state NOT IN ('completed','failed','canceled')")
    .bind(id).bind(step.get::<Uuid,_>("id")).bind(next).bind(receipt).bind(next=="held").execute(&mut *tx).await?;
            }
        }
    }
    if row.get::<String, _>("state") == "stopping" {
        let stop = steps.iter().find(|s| {
            s.get::<Option<Uuid>, _>("remote_request_id").is_some()
                && !s.get::<bool, _>("stop_admitted")
                && !matches!(
                    s.get::<String, _>("state").as_str(),
                    "completed" | "failed" | "halted" | "canceled"
                )
        });
        if let Some(step) = stop {
            let p = Principal {
                user_id: row.get("owner_user_id"),
                session_id: row.get("stop_auth_session_id"),
                bootstrap: false,
            };
            if grants::actor(state, &mut tx, &p).await.is_ok() {
                let step_id = step.get("id");
                sqlx::query("UPDATE enterprise_workflow_steps SET stop_admitted=true WHERE workflow_id=$1 AND id=$2 AND NOT stop_admitted").bind(id).bind(step_id).execute(&mut *tx).await?;
                tx.commit().await?;
                if let Ok(receipt) = remote_stop(state, &p, step_id).await {
                    store_remote(state, id, step_id, receipt).await?
                }
                return Ok(());
            }
        }
    }
    if !running {
        tx.commit().await?;
        return Ok(());
    }
    let p = match original_actor(state, &mut tx, id).await {
        Ok(p) => p,
        Err(_) => {
            sqlx::query("UPDATE enterprise_workflows SET state='held',revision=revision+1 WHERE id=$1 AND state='active'").bind(id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(());
        }
    };
    let states: BTreeMap<Uuid, String> =
        sqlx::query("SELECT id,state FROM enterprise_workflow_steps WHERE workflow_id=$1")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|r| (r.get("id"), r.get("state")))
            .collect();
    if states.values().all(|s| s == "completed") {
        sqlx::query(
            "UPDATE enterprise_workflows SET state='completed',revision=revision+1 WHERE id=$1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(());
    }
    if states.values().any(|s| matches!(s.as_str(), "held")) {
        sqlx::query("UPDATE enterprise_workflows SET state='held',revision=revision+1 WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(());
    }
    if states
        .values()
        .all(|s| matches!(s.as_str(), "completed" | "failed" | "halted" | "canceled"))
    {
        sqlx::query(
            "UPDATE enterprise_workflows SET state='failed',revision=revision+1 WHERE id=$1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(());
    }
    let pending_ready = steps.iter().any(|step| {
        states[&step.get::<Uuid, _>("id")] == "pending"
            && serde_json::from_value::<BTreeSet<Uuid>>(step.get("dependencies")).is_ok_and(
                |deps| {
                    deps.iter()
                        .all(|d| states.get(d).map(String::as_str) == Some("completed"))
                },
            )
    });
    if !pending_ready
        && !states
            .values()
            .any(|s| matches!(s.as_str(), "admitted" | "running" | "offline_running"))
    {
        sqlx::query("UPDATE enterprise_workflows SET state='held',revision=revision+1 WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(());
    }
    for step in steps {
        let step_id: Uuid = step.get("id");
        let step_state = &states[&step_id];
        let dependencies: BTreeSet<Uuid> = serde_json::from_value(step.get("dependencies"))?;
        let destination: ExecutionDestination = serde_json::from_value(step.get("destination"))?;
        let payload: Payload = serde_json::from_value(step.get("request"))?;
        if step_state == "pending"
            && dependencies
                .iter()
                .all(|d| states.get(d).map(String::as_str) == Some("completed"))
        {
            if let Payload::Device { request, .. } = payload {
                // Prepare through the same released-task/provider/content/Crew path as an
                // ordinary real run, retaining the original session through its writer.
                tx.commit().await?;
                let (spec, initial) = crate::organisation_mfa::optional_scope(
                    p.user_id,
                    p.session_id,
                    crate::routes::prepare_enterprise_run(state, &p, request),
                )
                .await?;
                let mut tx = state.pool.begin().await?;
                original_actor(state, &mut tx, id).await?;
                let locked=sqlx::query("SELECT state FROM enterprise_workflow_steps WHERE workflow_id=$1 AND id=$2 FOR UPDATE").bind(id).bind(step_id).fetch_one(&mut *tx).await?;
                if locked.get::<String, _>("state") != "pending" {
                    tx.commit().await?;
                    return Ok(());
                }
                if let ExecutionDestination::Device {
                    id: device,
                    grant_id,
                } = destination
                {
                    let revision = spec.input["_enterprise_grant"]["revision"]
                        .as_i64()
                        .ok_or_else(grants::denied)?;
                    let expected = grants::active(
                        &mut tx,
                        grant_id,
                        p.user_id,
                        spec.thread_id,
                        device,
                        revision,
                    )
                    .await?;
                    if spec.input.get("_enterprise_grant") != Some(&expected) {
                        return Err(grants::conflict());
                    }
                } else {
                    return Err(grants::conflict());
                }
                let (child, _) = crate::organisation_mfa::optional_scope(
                    p.user_id,
                    p.session_id,
                    crate::db::create_run_tx(&mut tx, &spec, initial),
                )
                .await?;
                sqlx::query("UPDATE enterprise_workflow_steps SET state='running',child_run_id=$3,dispatch_auth_session_id=$4 WHERE workflow_id=$1 AND id=$2 AND state='pending'").bind(id).bind(step_id).bind(child.spec.id).bind(p.session_id).execute(&mut *tx).await?;
                grants::actor(state, &mut tx, &p).await?;
                tx.commit().await?;
                return Ok(());
            }
            sqlx::query("UPDATE enterprise_workflow_steps SET state='admitted',remote_request_id=$3,original_effects_unknown=true,dispatch_auth_session_id=$4 WHERE workflow_id=$1 AND id=$2 AND state='pending'").bind(id).bind(step_id).bind(step_id).bind(p.session_id).execute(&mut *tx).await?;
            tx.commit().await?;
            // This is the only automatic POST. After this committed intent, every
            // later iteration reads the original operation instead of resending it.
            if let Ok(receipt) = remote(state, &p, id, step_id, &destination, &payload, true).await
            {
                store_remote(state, id, step_id, receipt).await?
            }
            return Ok(());
        }
        if matches!(step_state.as_str(), "admitted" | "running")
            && step.get::<Option<Uuid>, _>("remote_request_id").is_some()
        {
            tx.commit().await?;
            if let Ok(receipt) = remote(state, &p, id, step_id, &destination, &payload, false).await
            {
                store_remote(state, id, step_id, receipt).await?
            }
            return Ok(());
        }
    }
    tx.commit().await?;
    Ok(())
}
async fn store_remote(
    state: &AppState,
    workflow: Uuid,
    step: Uuid,
    receipt: Value,
) -> Result<(), ApiError> {
    if receipt["request_id"] != json!(step)
        || receipt["workflow_id"] != json!(workflow)
        || receipt["step_id"] != json!(step)
    {
        return Err(grants::conflict());
    }
    let received = receipt["state"].as_str().ok_or_else(grants::conflict)?;
    let next = match received {
        "queued" | "running" => "running",
        "held" => "held",
        "completed" => "completed",
        "failed" => "failed",
        "halted" => "halted",
        _ => return Err(grants::conflict()),
    };
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE enterprise_workflow_steps SET state=$3,receipt=CASE WHEN $3 IN ('completed','failed','halted') THEN COALESCE(receipt,$4) ELSE receipt END,original_effects_unknown=$5 WHERE workflow_id=$1 AND id=$2 AND remote_request_id=$2 AND state IN ('admitted','running','held')")
  .bind(workflow).bind(step).bind(next).bind(receipt).bind(!matches!(next,"completed"|"failed"|"halted")).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn run(state: AppState, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {_=shutdown.changed()=>if *shutdown.borrow(){break},_=interval.tick()=>{
         if state.ensure_api_active().await.is_err(){continue}
         let ids=sqlx::query_scalar::<_,Uuid>("SELECT id FROM enterprise_workflows WHERE state IN ('active','held','stopping') ORDER BY created_at LIMIT 100").fetch_all(&state.pool).await;
         if let Ok(ids)=ids{for id in ids{if let Err(error)=tick_one(&state,id).await{tracing::warn!(?error,%id,"distributed workflow retains original pending effects")}}}
        }}
    }
}
