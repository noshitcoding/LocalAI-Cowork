//! Session extraction precedes bounded bodies; all current authority is checked again inside SQL.
use super::*;
use axum::http::HeaderValue;
use cowork_contracts::{
    AndroidAdministrationRevision, AndroidExecutorPoolProjectWrite, AndroidExecutorPoolWrite,
    AndroidMobileCacheActivate, AndroidMobileCacheBegin, AndroidPersonalStorageQuotaWrite,
    AndroidSupportGrantRevoke, AndroidSupportGrantWrite,
};
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/mobile-cache/imports", get(cache_list).post(cache_begin))
        .route("/mobile-cache/imports/{id}", get(cache_status))
        .route(
            "/mobile-cache/imports/{id}/parts/{ordinal}",
            axum::routing::put(cache_part).layer(DefaultBodyLimit::max(1048576)),
        )
        .route("/mobile-cache/imports/{id}/index", post(cache_index))
        .route("/mobile-cache/imports/{id}/entries", get(cache_entries))
        .route("/mobile-cache/imports/{id}/bytes", get(cache_bytes))
        .route("/mobile-cache/imports/{id}/activate", post(cache_activate))
        .route("/mobile-cache/imports/{id}/retain", post(cache_retain))
        .route("/administration/operations/{key}", get(original))
        .route("/support-grants", get(support_list).post(support_create))
        .route("/support-grants/{id}/revoke", post(support_revoke))
        .route("/storage-quotas/{actor}", get(quota).put(quota_set))
        .route("/executor-pools", get(pools).post(pool_write))
        .route(
            "/executor-pools/{pool}/projects",
            get(pool_projects).post(pool_project_write),
        )
        .route("/operations/status", get(status))
        .route("/operations/support-bundle", get(bundle))
        .layer(DefaultBodyLimit::max(16384))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UuidPage {
    after: Option<Uuid>,
    limit: Option<u32>,
    project_id: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entries {
    after: Option<i64>,
    limit: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Original {
    purpose: String,
    request_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BytePage {
    stream: String,
    ordinal: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    project_id: Option<Uuid>,
}
async fn cache_list(
    State(s): State<CompatState>,
    Session(a): Session,
    Query(q): Query<UuidPage>,
) -> Answer {
    if q.project_id.is_some() {
        return Err(invalid().into());
    }
    Ok(Json(
        s.service
            .store()
            .android_cache_list(&a, q.after, q.limit.unwrap_or(50))
            .await?,
    ))
}
async fn cache_begin(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(v): Json<AndroidMobileCacheBegin>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_cache_begin(&a, v.request_id, v.archive_size, &v.archive_sha256)
            .await?,
    ))
}
async fn cache_status(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
) -> Answer {
    Ok(Json(s.service.store().android_cache_status(&a, id).await?))
}
async fn cache_part(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((id, ordinal)): Path<(Uuid, u64)>,
    raw: axum::body::Bytes,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_cache_upload(&a, id, ordinal, &raw, s.keys.as_ref(), s.key_id)
            .await?,
    ))
}
async fn cache_index(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(v): Json<AndroidAdministrationRevision>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_cache_index(&a, id, v.expected_revision, s.keys.as_ref(), s.key_id)
            .await?,
    ))
}
async fn cache_entries(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Query(q): Query<Entries>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_cache_entries(&a, id, q.after.unwrap_or(0), q.limit.unwrap_or(50))
            .await?,
    ))
}
async fn cache_bytes(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Query(q): Query<BytePage>,
) -> std::result::Result<Response, Failure> {
    let (pin, raw) = s
        .service
        .store()
        .android_cache_bytes(&a, id, &q.stream, q.ordinal, s.keys.as_ref())
        .await?;
    let mut h = HeaderMap::new();
    h.insert(
        "content-type",
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert("cache-control", HeaderValue::from_static("no-store"));
    h.insert(
        "x-content-sha256",
        HeaderValue::from_str(pin["sha256"].as_str().ok_or_else(invalid)?)
            .map_err(|_| invalid())?,
    );
    h.insert(
        "x-content-size",
        HeaderValue::from_str(&raw.len().to_string()).map_err(|_| invalid())?,
    );
    Ok((h, raw).into_response())
}
async fn cache_activate(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(v): Json<AndroidMobileCacheActivate>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_cache_activate(
                &a,
                id,
                v.request_id,
                v.expected_revision,
                v.thread_id,
                &v.ordinals,
                &v.remainder_history_only_reason,
                s.keys.as_ref(),
            )
            .await?,
    ))
}
async fn cache_retain(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(v): Json<AndroidAdministrationRevision>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_cache_retained(&a, id, v.expected_revision)
            .await?,
    ))
}
async fn original(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(key): Path<String>,
    Query(q): Query<Original>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_administration_original(&a, &key, &q.purpose, &q.request_sha256)
            .await?,
    ))
}
async fn support_list(
    State(s): State<CompatState>,
    Session(a): Session,
    Query(q): Query<UuidPage>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_support_grants(&a, q.project_id, q.after, q.limit.unwrap_or(50))
            .await?,
    ))
}
async fn support_create(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(v): Json<AndroidSupportGrantWrite>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_support_create(
                &a,
                v.project_id,
                v.user_id,
                v.expected_revision,
                v.expires_at,
                &v.reason,
                v.request_id,
            )
            .await?,
    ))
}
async fn support_revoke(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(v): Json<AndroidSupportGrantRevoke>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_support_revoke(&a, id, v.expected_revision, v.request_id)
            .await?,
    ))
}
async fn quota(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(target): Path<Uuid>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_personal_storage_quota(&a, target)
            .await?,
    ))
}
async fn quota_set(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(target): Path<Uuid>,
    Json(v): Json<AndroidPersonalStorageQuotaWrite>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_personal_storage_quota_set(
                &a,
                target,
                v.expected_revision,
                v.limit_bytes,
                v.request_id,
            )
            .await?,
    ))
}
async fn status(
    State(s): State<CompatState>,
    Session(a): Session,
    Query(q): Query<Status>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_operational_status(&a, q.project_id)
            .await?,
    ))
}
async fn bundle(
    State(s): State<CompatState>,
    Session(a): Session,
    Query(q): Query<Status>,
) -> Answer {
    let v = s
        .service
        .store()
        .android_operational_status(&a, q.project_id)
        .await?;
    Ok(Json(
        json!({"format":"localai-cowork-support-bundle/1","status":v,"secrets_included":false,"original_content_included":false,"execution_grants_included":false}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PoolPage {
    organization_id: Uuid,
    after: Option<Uuid>,
    limit: Option<u32>,
}
async fn pools(
    State(s): State<CompatState>,
    Session(a): Session,
    Query(q): Query<PoolPage>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_executor_pools(&a, q.organization_id, q.after, q.limit.unwrap_or(50))
            .await?,
    ))
}
async fn pool_write(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(v): Json<AndroidExecutorPoolWrite>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_executor_pool_write(
                &a,
                v.organization_id,
                v.id,
                v.expected_revision,
                &v.name,
                v.active,
                v.policy,
                v.request_id,
            )
            .await?,
    ))
}
async fn pool_projects(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
    Query(q): Query<PoolPage>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_executor_pool_projects(
                &a,
                q.organization_id,
                p,
                q.after,
                q.limit.unwrap_or(50),
            )
            .await?,
    ))
}
async fn pool_project_write(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
    Json(v): Json<AndroidExecutorPoolProjectWrite>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_executor_pool_project_write(
                &a,
                v.organization_id,
                p,
                v.project_id,
                v.expected_revision,
                v.active,
                v.request_id,
            )
            .await?,
    ))
}
