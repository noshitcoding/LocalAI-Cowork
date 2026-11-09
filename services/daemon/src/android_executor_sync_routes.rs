//! Separate device authentication and explicit current replication grant.
use super::*;
use cowork_contracts::{AndroidExecutorSyncBatch, AndroidExecutorSyncGrantWrite};
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/sync/device", get(sync_device))
        .route("/sync/operations/{operation}", get(sync_original))
        .route("/executors/{executor}/sync-grants", get(grants).post(grant))
        .route(
            "/agent/executors/{executor}/sync/changes",
            get(pull).post(push),
        )
        .route(
            "/agent/executors/{executor}/sync/entities/{kind}",
            get(entities),
        )
        .route("/agent/executors/{executor}/sync/events", get(events))
        .layer(DefaultBodyLimit::max(3 * 1024 * 1024))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    project_id: Uuid,
    after: Option<i64>,
    limit: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entities {
    project_id: Uuid,
    after: Option<Uuid>,
    limit: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantPage {
    project_id: Option<Uuid>,
    after: Option<Uuid>,
    limit: Option<u32>,
}
async fn grants(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(executor): Path<Uuid>,
    Query(q): Query<GrantPage>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_executor_sync_grants(
                &a,
                executor,
                q.project_id,
                q.after,
                q.limit.unwrap_or(50),
            )
            .await?,
    ))
}
async fn grant(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(executor): Path<Uuid>,
    Json(v): Json<AndroidExecutorSyncGrantWrite>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_executor_sync_grant(
                &a,
                executor,
                v.project_id,
                v.expected_revision,
                v.write,
                v.active,
                v.expires_at,
                v.request_id,
            )
            .await?,
    ))
}
async fn pull(State(s): State<CompatState>, Agent(a): Agent, Query(q): Query<Page>) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_agent_sync_pull(
                &a,
                q.project_id,
                q.after.unwrap_or(0),
                q.limit.unwrap_or(200),
            )
            .await?,
    ))
}
async fn push(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Json(v): Json<AndroidExecutorSyncBatch>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_agent_sync_push(&a, v.project_id, v.changes)
            .await?,
    ))
}
async fn entities(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, kind)): Path<(Uuid, String)>,
    Query(q): Query<Entities>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_agent_sync_entities(&a, q.project_id, &kind, q.after, q.limit.unwrap_or(200))
            .await?,
    ))
}
async fn events(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Query(q): Query<Page>,
    headers: HeaderMap,
) -> std::result::Result<Response, Failure> {
    let after = if let Some(after) = q.after {
        after
    } else {
        headers
            .get("last-event-id")
            .map(|h| {
                h.to_str()
                    .ok()
                    .and_then(|v| v.parse::<i64>().ok())
                    .filter(|v| *v >= 0)
                    .ok_or_else(invalid)
            })
            .transpose()?
            .unwrap_or(0)
    };
    s.service
        .store()
        .android_agent_sync_pull(&a, q.project_id, after, 1)
        .await?;
    let stream = futures_util::stream::unfold(
        (s, a, q.project_id, after, VecDeque::<Value>::new()),
        |(s, a, project, mut cursor, mut pending)| async move {
            if pending.is_empty() {
                loop {
                    let batch = s
                        .service
                        .store()
                        .android_agent_sync_pull(&a, project, cursor, 200)
                        .await
                        .ok()?;
                    cursor = batch["next_cursor"].as_i64()?;
                    pending.extend(batch["changes"].as_array()?.iter().cloned());
                    if !pending.is_empty() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(4)).await;
                }
            }
            let change = pending.pop_front()?;
            let id = change["cursor"].as_i64()?;
            let original = s
                .service
                .store()
                .android_agent_sync_frame(&a, project, id)
                .await
                .ok()??;
            let event = Event::default()
                .event("sync_change")
                .id(id.to_string())
                .json_data(original)
                .ok()?;
            Some((
                Ok::<_, std::convert::Infallible>(event),
                (s, a, project, cursor, pending),
            ))
        },
    );
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(10)))
        .into_response())
}
async fn sync_device(State(s): State<CompatState>, Session(a): Session) -> Answer {
    Ok(Json(
        s.service.store().android_current_sync_device(&a).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginalSync {
    request_sha256: String,
}
async fn sync_original(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(operation): Path<Uuid>,
    Query(q): Query<OriginalSync>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_sync_original(&a, operation, &q.request_sha256)
            .await?,
    ))
}
