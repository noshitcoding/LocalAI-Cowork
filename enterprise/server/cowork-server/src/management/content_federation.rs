//! Source-instance scoped copy permission, current original sessions and immutable receipts.
use super::*;
use chrono::{DateTime, Utc};
use serde::Serialize;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    request_id: Uuid,
    source_instance_id: Uuid,
    target_organisation_id: Uuid,
    target_instance_id: Uuid,
    recipient_user_id: Uuid,
    project_id: Uuid,
    version_id: Uuid,
    paths: Vec<String>,
    expires_at: DateTime<Utc>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    request_id: Uuid,
    expected_revision: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Proof {
    pub grant_id: Uuid,
    pub revision: i64,
    pub epoch: Uuid,
    pub source_organisation_id: Uuid,
    pub source_instance_id: Uuid,
    pub target_organisation_id: Uuid,
    pub target_instance_id: Uuid,
    pub publisher_user_id: Uuid,
    pub recipient_user_id: Uuid,
    pub source_epoch: Uuid,
    pub target_epoch: Uuid,
    pub project_id: Uuid,
    pub version_id: Uuid,
    pub paths: Vec<String>,
    pub expires_at: DateTime<Utc>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Callback {
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub user_id: Uuid,
    pub grant_id: Uuid,
    pub transfer_id: Uuid,
    pub action: String,
    pub bundle_sha256: Option<String>,
    pub receipt: Option<Value>,
}
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/organisations/{org}/content-sharing/grants",
            get(list).post(create),
        )
        .route(
            "/organisations/{org}/content-sharing/grants/{id}",
            get(read),
        )
        .route(
            "/organisations/{org}/content-sharing/grants/{id}/accept",
            post(accept),
        )
        .route(
            "/organisations/{org}/content-sharing/grants/{id}/revoke",
            post(revoke),
        )
        .route(
            "/organisations/{org}/content-sharing/transfers/{id}",
            get(transfer),
        )
}
pub(super) fn runner_router() -> Router<AppState> {
    Router::new()
        .route("/content-sharing/authorize", post(authorize))
        .route("/content-sharing/receipt", post(receipt))
}
fn hash(v: &impl Serialize) -> Result<String, ApiError> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(v)?)))
}
fn sha(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
async fn actor(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    user: Uuid,
    sid: Uuid,
) -> Result<Option<DateTime<Utc>>, ApiError> {
    let session = crate::organisation_mfa::session_in_tx(state, tx, user, sid, false).await?;
    let role:Option<String>=sqlx::query_scalar("SELECT role FROM management_members WHERE organisation_id=$1 AND user_id=$2 AND role IN ('admin','member') FOR SHARE").bind(org).bind(user).fetch_optional(&mut **tx).await?;
    if role.is_none() {
        return Err(denied());
    }
    let policy = mfa::policy(tx, org).await?;
    let until = if policy.required {
        let (assured, until) =
            crate::organisation_mfa::assured_in_tx(state, tx, user, &session, &policy).await?;
        if !assured || until.is_none() {
            return Err(ApiError::MfaRequired(
                "current organisation assurance required".into(),
            ));
        }
        until
    } else {
        None
    };
    let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth_sessions s JOIN users u ON u.id=s.user_id WHERE s.id=$1 AND s.user_id=$2 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL) AND ($3::timestamptz IS NULL OR $3>clock_timestamp())").bind(sid).bind(user).bind(until).fetch_one(&mut **tx).await?;
    if !live {
        return Err(denied());
    }
    Ok(until)
}
async fn instance(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    id: Uuid,
    user: Uuid,
    write: bool,
) -> Result<sqlx::postgres::PgRow, ApiError> {
    let row=sqlx::query("SELECT i.*,o.policy AS organisation_policy,m.role,m.content_admin FROM management_instances i JOIN management_organisations o ON o.id=i.organisation_id JOIN management_members m ON m.organisation_id=i.organisation_id AND m.user_id=$3 WHERE i.organisation_id=$1 AND i.id=$2 AND i.state='ready' AND o.archived_at IS NULL AND NOT EXISTS(SELECT 1 FROM management_instance_import_locks l WHERE l.instance_id=i.id) FOR SHARE OF i,o,m").bind(org).bind(id).bind(user).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let member = crate::fleet::allowed_on(
        &mut **tx,
        &crate::fleet::AccessRequest {
            organisation_id: org,
            instance_id: id,
            user_id: user,
        },
    )
    .await?;
    if !member {
        let a: PolicyRevision = serde_json::from_value(row.get("policy"))?;
        let b: PolicyRevision = serde_json::from_value(row.get("organisation_policy"))?;
        if write
            || row.get::<String, _>("role") != "admin"
            || !row.get::<bool, _>("content_admin")
            || a.restricted_by(&b)
                .restricted_by(platform_ceiling())
                .content_access
                != ContentAccess::OrganisationAdmin
        {
            return Err(denied());
        }
    }
    Ok(row)
}
fn policy(row: &sqlx::postgres::PgRow) -> Result<String, ApiError> {
    hash(
        &json!({"instance":row.get::<Value,_>("policy"),"organisation":row.get::<Value,_>("organisation_policy")}),
    )
}
fn proof(r: &sqlx::postgres::PgRow) -> Result<Proof, ApiError> {
    Ok(Proof {
        grant_id: r.get("id"),
        revision: r.get("revision"),
        epoch: r.get("epoch"),
        source_organisation_id: r.get("source_organisation_id"),
        source_instance_id: r.get("source_instance_id"),
        target_organisation_id: r.get("target_organisation_id"),
        target_instance_id: r.get("target_instance_id"),
        publisher_user_id: r.get("publisher_user_id"),
        recipient_user_id: r.get("recipient_user_id"),
        source_epoch: r.get("source_epoch"),
        target_epoch: r.get("target_epoch"),
        project_id: r.get("project_id"),
        version_id: r.get("version_id"),
        paths: serde_json::from_value(r.get("paths"))?,
        expires_at: r.get("expires_at"),
    })
}
async fn current(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    active: bool,
) -> Result<Proof, ApiError> {
    let row=sqlx::query("SELECT * FROM management_content_grants WHERE id=$1 AND state<> 'revoked' AND expires_at>clock_timestamp() FOR SHARE").bind(id).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let p = proof(&row)?;
    let publisher_until = actor(
        state,
        tx,
        p.source_organisation_id,
        p.publisher_user_id,
        row.get("publisher_session_id"),
    )
    .await?;
    let a = instance(
        tx,
        p.source_organisation_id,
        p.source_instance_id,
        p.publisher_user_id,
        false,
    )
    .await?;
    let b = instance(
        tx,
        p.target_organisation_id,
        p.target_instance_id,
        p.recipient_user_id,
        true,
    )
    .await?;
    if a.get::<i64, _>("revision") != row.get::<i64, _>("source_instance_revision")
        || b.get::<i64, _>("revision") != row.get::<i64, _>("target_instance_revision")
        || a.get::<Uuid, _>("execution_epoch") != p.source_epoch
        || b.get::<Uuid, _>("execution_epoch") != p.target_epoch
        || policy(&a)? != row.get::<String, _>("source_policy_sha256")
        || policy(&b)? != row.get::<String, _>("target_policy_sha256")
    {
        return Err(denied());
    }
    let recipient_until = if active {
        if row.get::<String, _>("state") != "active" {
            return Err(denied());
        }
        actor(
            state,
            tx,
            p.target_organisation_id,
            p.recipient_user_id,
            row.get::<Option<Uuid>, _>("recipient_session_id")
                .ok_or_else(denied)?,
        )
        .await?
    } else {
        None
    };
    // All authority rows above are locked. Recheck time after the final lock wait.
    let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_content_grants g JOIN auth_sessions ps ON ps.id=g.publisher_session_id AND ps.user_id=g.publisher_user_id LEFT JOIN auth_sessions rs ON rs.id=g.recipient_session_id AND rs.user_id=g.recipient_user_id WHERE g.id=$1 AND g.state<>'revoked' AND g.expires_at>clock_timestamp() AND ps.revoked_at IS NULL AND ps.expires_at>clock_timestamp() AND (NOT $2 OR (g.state='active' AND rs.revoked_at IS NULL AND rs.expires_at>clock_timestamp()))) AND ($3::timestamptz IS NULL OR $3>clock_timestamp()) AND ($4::timestamptz IS NULL OR $4>clock_timestamp())").bind(id).bind(active).bind(publisher_until).bind(recipient_until).fetch_one(&mut **tx).await?;
    if !live {
        return Err(denied());
    }
    Ok(p)
}
async fn original(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    request: Uuid,
    kind: &str,
    id: Uuid,
    sha: &str,
) -> Result<Option<Value>, ApiError> {
    if let Some(r) = sqlx::query(
        "SELECT * FROM management_content_operations WHERE actor_user_id=$1 AND request_id=$2",
    )
    .bind(user)
    .bind(request)
    .fetch_optional(&mut **tx)
    .await?
    {
        if r.get::<String, _>("kind") != kind
            || r.get::<Uuid, _>("grant_id") != id
            || r.get::<String, _>("request_sha256") != sha
        {
            return Err(ApiError::Conflict(
                "original content-sharing operation differs".into(),
            ));
        }
        return Ok(Some(r.get("receipt")));
    }
    Ok(None)
}
async fn operation(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    request: Uuid,
    kind: &str,
    id: Uuid,
    sha: &str,
    v: &Value,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO management_content_operations(actor_user_id,request_id,kind,grant_id,request_sha256,receipt) VALUES($1,$2,$3,$4,$5,$6)").bind(user).bind(request).bind(kind).bind(id).bind(sha).bind(v).execute(&mut **tx).await?;
    Ok(())
}
async fn create(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(r): Json<Create>,
) -> Result<Json<Value>, ApiError> {
    if p.bootstrap
        || r.request_id.is_nil()
        || r.project_id.is_nil()
        || r.version_id.is_nil()
        || r.recipient_user_id.is_nil()
        || r.paths
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != r.paths.len()
        || r.source_instance_id == r.target_instance_id
        || r.paths.is_empty()
        || r.paths.len() > 64
        || r.paths
            .iter()
            .any(|p| !safe_relative_path(p) || p.len() > 1024)
    {
        return Err(invalid(
            "explicit immutable content paths and bounded lifetime required",
        ));
    }
    let sid = p.session_id.ok_or_else(denied)?;
    let sha = hash(&json!({"source_organisation_id":org,"request":r}))?;
    let mut tx = state.pool.begin().await?;
    actor(&state, &mut tx, org, p.user_id, sid).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("content-sharing:{}:{}", p.user_id, r.request_id))
        .execute(&mut *tx)
        .await?;
    if let Some(old)=sqlx::query("SELECT o.kind,o.grant_id,o.request_sha256,o.receipt,g.source_organisation_id FROM management_content_operations o JOIN management_content_grants g ON g.id=o.grant_id WHERE o.actor_user_id=$1 AND o.request_id=$2").bind(p.user_id).bind(r.request_id).fetch_optional(&mut *tx).await?{if old.get::<String,_>("kind")!="create" || old.get::<Uuid,_>("source_organisation_id")!=org || old.get::<String,_>("request_sha256")!=sha{return Err(invalid("different original request"))}let v=old.get("receipt");actor(&state,&mut tx,org,p.user_id,sid).await?;tx.commit().await?;return Ok(Json(v))}
    if r.expires_at <= Utc::now() || r.expires_at > Utc::now() + chrono::Duration::hours(24) {
        return Err(invalid("bounded original content lifetime required"));
    }
    let a = instance(&mut tx, org, r.source_instance_id, p.user_id, false).await?;
    let b = instance(
        &mut tx,
        r.target_organisation_id,
        r.target_instance_id,
        r.recipient_user_id,
        true,
    )
    .await?;
    let id = Uuid::new_v4();
    let epoch = Uuid::new_v4();
    sqlx::query("INSERT INTO management_content_grants(id,source_organisation_id,source_instance_id,target_organisation_id,target_instance_id,publisher_user_id,publisher_session_id,recipient_user_id,revision,epoch,state,source_instance_revision,target_instance_revision,source_epoch,target_epoch,source_policy_sha256,target_policy_sha256,project_id,version_id,paths,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,1,$9,'pending',$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)").bind(id).bind(org).bind(r.source_instance_id).bind(r.target_organisation_id).bind(r.target_instance_id).bind(p.user_id).bind(sid).bind(r.recipient_user_id).bind(epoch).bind(a.get::<i64,_>("revision")).bind(b.get::<i64,_>("revision")).bind(a.get::<Uuid,_>("execution_epoch")).bind(b.get::<Uuid,_>("execution_epoch")).bind(policy(&a)?).bind(policy(&b)?).bind(r.project_id).bind(r.version_id).bind(json!(r.paths)).bind(r.expires_at).execute(&mut *tx).await?;
    let v = json!({"format":1,"id":id,"revision":1,"epoch":epoch,"state":"pending","request_id":r.request_id,"request_sha256":sha});
    operation(&mut tx, p.user_id, r.request_id, "create", id, &sha, &v).await?;
    current(&state, &mut tx, id, false).await?;
    append_audit(
        &mut tx,
        org,
        p.user_id,
        "content-sharing.created",
        id,
        json!({"target_instance_id":r.target_instance_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(v))
}
async fn control(
    state: &AppState,
    p: &Principal,
    org: Uuid,
    id: Uuid,
    r: Control,
    accepting: bool,
) -> Result<Value, ApiError> {
    let sid = p.session_id.ok_or_else(denied)?;
    if r.request_id.is_nil() || r.expected_revision < 1 {
        return Err(invalid("original request and revision required"));
    }
    let sha = hash(&r)?;
    let mut tx = state.pool.begin().await?;
    actor(state, &mut tx, org, p.user_id, sid).await?;
    let row = sqlx::query("SELECT * FROM management_content_grants WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(denied)?;
    let bound = if accepting {
        row.get::<Uuid, _>("recipient_user_id") == p.user_id
            && row.get::<Uuid, _>("target_organisation_id") == org
    } else {
        row.get::<Uuid, _>("publisher_user_id") == p.user_id
            && row.get::<Uuid, _>("source_organisation_id") == org
    };
    if !bound {
        return Err(denied());
    }
    let kind = if accepting { "accept" } else { "revoke" };
    if let Some(v) = original(&mut tx, p.user_id, r.request_id, kind, id, &sha).await? {
        tx.commit().await?;
        return Ok(v);
    }
    if row.get::<i64, _>("revision") != r.expected_revision {
        return Err(ApiError::Conflict("original grant revision changed".into()));
    }
    if accepting {
        current(state, &mut tx, id, false).await?;
        if row.get::<String, _>("state") != "pending" {
            return Err(denied());
        }
    }
    let revision = r.expected_revision.checked_add(1).ok_or_else(denied)?;
    let next = if accepting { "active" } else { "revoked" };
    sqlx::query("UPDATE management_content_grants SET revision=$2,state=$3,recipient_session_id=CASE WHEN $4 THEN $5 ELSE recipient_session_id END WHERE id=$1 AND revision=$6").bind(id).bind(revision).bind(next).bind(accepting).bind(sid).bind(r.expected_revision).execute(&mut *tx).await?;
    let v = json!({"format":1,"id":id,"revision":revision,"state":next,"request_id":r.request_id,"request_sha256":sha,"completed_copies_retained":true});
    operation(&mut tx, p.user_id, r.request_id, kind, id, &sha, &v).await?;
    actor(state, &mut tx, org, p.user_id, sid).await?;
    if accepting {
        current(state, &mut tx, id, true).await?;
    }
    append_audit(
        &mut tx,
        org,
        p.user_id,
        if accepting {
            "content-sharing.accepted"
        } else {
            "content-sharing.revoked"
        },
        id,
        json!({"revision":revision}),
    )
    .await?;
    tx.commit().await?;
    Ok(v)
}
async fn accept(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((o, id)): Path<(Uuid, Uuid)>,
    Json(r): Json<Control>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(control(&s, &p, o, id, r, true).await?))
}
async fn revoke(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((o, id)): Path<(Uuid, Uuid)>,
    Json(r): Json<Control>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(control(&s, &p, o, id, r, false).await?))
}
async fn list(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(o): Path<Uuid>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let mut tx = s.pool.begin().await?;
    actor(&s, &mut tx, o, p.user_id, p.session_id.ok_or_else(denied)?).await?;
    let rows=sqlx::query_scalar("SELECT to_jsonb(g)-'publisher_session_id'-'recipient_session_id' FROM management_content_grants g WHERE (source_organisation_id=$1 AND publisher_user_id=$2) OR (target_organisation_id=$1 AND recipient_user_id=$2) ORDER BY created_at DESC LIMIT 200").bind(o).bind(p.user_id).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(rows))
}
async fn read(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((o, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = s.pool.begin().await?;
    actor(&s, &mut tx, o, p.user_id, p.session_id.ok_or_else(denied)?).await?;
    let mut v:Value=sqlx::query_scalar("SELECT to_jsonb(g)-'publisher_session_id'-'recipient_session_id' FROM management_content_grants g WHERE id=$1 AND ((source_organisation_id=$2 AND publisher_user_id=$3) OR (target_organisation_id=$2 AND recipient_user_id=$3))").bind(id).bind(o).bind(p.user_id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    let active = current(&s, &mut tx, id, true).await.is_ok();
    v["current_authorized"] = json!(active);
    tx.commit().await?;
    Ok(Json(v))
}
async fn transfer(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((o, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = s.pool.begin().await?;
    actor(&s, &mut tx, o, p.user_id, p.session_id.ok_or_else(denied)?).await?;
    let v=sqlx::query_scalar("SELECT to_jsonb(e) FROM management_content_exports e JOIN management_content_grants g ON g.id=e.grant_id WHERE e.transfer_id=$1 AND ((g.source_organisation_id=$2 AND g.publisher_user_id=$3) OR (g.target_organisation_id=$2 AND g.recipient_user_id=$3))").bind(id).bind(o).bind(p.user_id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    tx.commit().await?;
    Ok(Json(v))
}
async fn authorized(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    q: &Callback,
) -> Result<Proof, ApiError> {
    crate::fleet::verify_instance_key(tx, headers, q.organisation_id, q.instance_id).await?;
    let p = current(state, tx, q.grant_id, true).await?;
    let source = matches!(q.action.as_str(), "export" | "publish");
    let (organisation, instance, user) = if source {
        (
            p.source_organisation_id,
            p.source_instance_id,
            p.publisher_user_id,
        )
    } else {
        (
            p.target_organisation_id,
            p.target_instance_id,
            p.recipient_user_id,
        )
    };
    if q.organisation_id != organisation
        || q.instance_id != instance
        || q.user_id != user
        || q.transfer_id.is_nil()
    {
        return Err(denied());
    }
    Ok(p)
}
async fn validate_source(
    state: &AppState,
    p: &Proof,
    transfer: Uuid,
    sha: &str,
) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await?;
    let key =
        crate::fleet::current_instance_key(&mut tx, p.source_organisation_id, p.source_instance_id)
            .await?;
    tx.commit().await?;
    let origin = crate::instance_gateway::origin(p.source_instance_id)?;
    let client =
        crate::management_transport::client(origin.as_str(), std::time::Duration::from_secs(8))?;
    let response = client
        .post(format!(
            "{}/api/v1/content-sharing/validate",
            origin.as_str().trim_end_matches('/')
        ))
        .header("x-cowork-instance-key", &key)
        .json(&json!({"transfer_id":transfer,"grant_id":p.grant_id,"bundle_sha256":sha}))
        .send()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    if !response.status().is_success() {
        return Err(denied());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    if bytes.len() > 8192 {
        return Err(denied());
    }
    let v: Value = serde_json::from_slice(&bytes)?;
    if v["current_authorized"] != true
        || v["transfer_id"] != json!(transfer)
        || v["grant_id"] != json!(p.grant_id)
        || v["bundle_sha256"] != json!(sha)
        || v["source_epoch"] != json!(p.source_epoch)
    {
        return Err(denied());
    }
    let mut tx = state.pool.begin().await?;
    if crate::fleet::current_instance_key(&mut tx, p.source_organisation_id, p.source_instance_id)
        .await?
        != key
    {
        return Err(denied());
    }
    tx.commit().await?;
    Ok(())
}
async fn authorize(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(q): Json<Callback>,
) -> Result<Json<Proof>, ApiError> {
    if !["export", "import"].contains(&q.action.as_str()) {
        return Err(denied());
    }
    let mut tx = s.pool.begin().await?;
    let p = authorized(&s, &mut tx, &headers, &q).await?;
    if q.action == "import" {
        let sha = q
            .bundle_sha256
            .as_ref()
            .filter(|v| sha(v))
            .ok_or_else(denied)?;
        let v:Value=sqlx::query_scalar("SELECT source_receipt FROM management_content_exports WHERE transfer_id=$1 AND grant_id=$2 AND grant_revision=$3 AND grant_epoch=$4 AND bundle_sha256=$5").bind(q.transfer_id).bind(p.grant_id).bind(p.revision).bind(p.epoch).bind(sha).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
        if v["source_epoch"] != json!(p.source_epoch) {
            return Err(denied());
        }
        tx.commit().await?;
        validate_source(&s, &p, q.transfer_id, sha).await?;
        let mut tx = s.pool.begin().await?;
        let final_p = authorized(&s, &mut tx, &headers, &q).await?;
        if hash(&p)? != hash(&final_p)? {
            return Err(denied());
        }
        tx.commit().await?;
        return Ok(Json(final_p));
    }
    tx.commit().await?;
    Ok(Json(p))
}
async fn receipt(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(q): Json<Callback>,
) -> Result<Json<Value>, ApiError> {
    if !["publish", "commit"].contains(&q.action.as_str()) {
        return Err(denied());
    }
    let r = q.receipt.as_ref().ok_or_else(denied)?;
    let sha = q
        .bundle_sha256
        .as_ref()
        .filter(|v| sha(v))
        .ok_or_else(denied)?;
    let mut tx = s.pool.begin().await?;
    let p = authorized(&s, &mut tx, &headers, &q).await?;
    if r["format"] != 1
        || r["transfer_id"] != json!(q.transfer_id)
        || r["grant_id"] != json!(p.grant_id)
        || r["grant_revision"] != json!(p.revision)
        || r["grant_epoch"] != json!(p.epoch)
        || r["bundle_sha256"] != json!(sha)
    {
        return Err(denied());
    }
    if q.action == "publish" {
        let size = r["size"]
            .as_i64()
            .filter(|n| *n >= 0 && *n <= 24 * 1024 * 1024)
            .ok_or_else(denied)?;
        if r["source_epoch"] != json!(p.source_epoch)
            || r["source_instance_id"] != json!(p.source_instance_id)
        {
            return Err(denied());
        }
        sqlx::query("INSERT INTO management_content_exports(transfer_id,grant_id,grant_revision,grant_epoch,bundle_sha256,size,source_receipt) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(transfer_id) DO NOTHING").bind(q.transfer_id).bind(p.grant_id).bind(p.revision).bind(p.epoch).bind(sha).bind(size).bind(r).execute(&mut *tx).await?;
        let old: Value = sqlx::query_scalar(
            "SELECT source_receipt FROM management_content_exports WHERE transfer_id=$1",
        )
        .bind(q.transfer_id)
        .fetch_one(&mut *tx)
        .await?;
        if old != *r {
            return Err(ApiError::Conflict(
                "different original source export receipt".into(),
            ));
        }
    } else {
        if r["target_epoch"] != json!(p.target_epoch)
            || r["target_instance_id"] != json!(p.target_instance_id)
            || r["execution_authority_copied"] != false
        {
            return Err(denied());
        }
        let row=sqlx::query("SELECT grant_id,bundle_sha256,target_receipt FROM management_content_exports WHERE transfer_id=$1 FOR UPDATE").bind(q.transfer_id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
        if row.get::<Uuid, _>("grant_id") != p.grant_id
            || row.get::<String, _>("bundle_sha256") != *sha
        {
            return Err(denied());
        }
        if let Some(old) = row.get::<Option<Value>, _>("target_receipt") {
            if old != *r {
                return Err(ApiError::Conflict(
                    "different original target import receipt".into(),
                ));
            }
        }
        sqlx::query("UPDATE management_content_exports SET target_receipt=$2 WHERE transfer_id=$1 AND target_receipt IS NULL").bind(q.transfer_id).bind(r).execute(&mut *tx).await?;
    }
    let v = json!({"format":1,"transfer_id":q.transfer_id,"grant_id":p.grant_id,"grant_revision":p.revision,"grant_epoch":p.epoch,"bundle_sha256":sha,"original_receipt":r,"state":if q.action=="publish"{"exported"}else{"committed"}});
    current(&s, &mut tx, p.grant_id, true).await?;
    tx.commit().await?;
    Ok(Json(v))
}
