//! Original device-agent HTTP/WebSocket wire over explicit fresh V2 pairing.
use super::*;
use axum::extract::{
    WebSocketUpgrade,
    ws::{Message, WebSocket},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cowork_storage::AndroidAgentPrincipal;
use futures_util::{SinkExt, StreamExt};
#[path = "android_executor_sync_routes.rs"]
mod sync_api;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .merge(sync_api::routes())
        .route("/executors", get(list).post(pair_original))
        .route("/devices/pair", post(pair))
        .route("/executors/{executor}", axum::routing::patch(update))
        .route(
            "/executors/{executor}/credentials",
            get(credentials).post(credential),
        )
        .route(
            "/executors/{executor}/credentials/{credential}/revoke",
            post(revoke),
        )
        .route("/agent/executors/{executor}/register", post(register))
        .route("/agent/executors/{executor}/heartbeat", post(heartbeat))
        .route("/agent/executors/{executor}/claim", post(claim))
        .route("/agent/executors/{executor}/connect", get(connect))
        .route(
            "/agent/executors/{executor}/desktop-streams/{stream}",
            get(desktop_stream),
        )
        .route(
            "/agent/executors/{executor}/runs/{run}/heartbeat",
            post(lease_heartbeat),
        )
        .route("/agent/executors/{executor}/runs/{run}/events", post(event))
        .route(
            "/agent/executors/{executor}/runs/{run}/complete",
            post(complete),
        )
        .route("/agent/executors/{executor}/runs/{run}/fail", post(fail))
        .route(
            "/agent/executors/{executor}/runs/{run}/approvals",
            post(approval),
        )
        .route(
            "/agent/executors/{executor}/runs/{run}/approvals/{id}",
            get(approval_get),
        )
        .route(
            "/agent/executors/{executor}/runs/{run}/input-requests",
            post(input),
        )
        .route(
            "/agent/executors/{executor}/runs/{run}/input-requests/{id}",
            get(input_get),
        )
        .route(
            "/agent/executors/{executor}/runs/{run}/checkpoints",
            post(checkpoint),
        )
        .route(
            "/agent/executors/{executor}/runs/{run}/snapshot",
            get(snapshot),
        )
        .route(
            "/agent/executors/{executor}/runs/{run}/snapshot/chunks/{digest}",
            get(chunk_get),
        )
        .route(
            "/agent/executors/{executor}/runs/{run}/result-snapshot",
            get(result_snapshot).post(begin_snapshot),
        )
        .route(
            "/agent/executors/{executor}/runs/{run}/result-snapshot/{manifest}/chunks/{digest}",
            axum::routing::put(chunk_put).layer(DefaultBodyLimit::max(4 * 1024 * 1024)),
        )
        .route(
            "/agent/executors/{executor}/runs/{run}/result-snapshot/{manifest}/commit",
            post(commit_snapshot),
        )
        .route(
            "/agent/executors/{executor}/runs/{run}/artifacts",
            post(artifact).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route("/runs/{run}/approvals", get(approval_list))
        .route("/runs/{run}/approvals/{id}/resolve", post(resolve))
        .route("/runs/{run}/input-requests", get(input_list))
        .route("/runs/{run}/input-requests/{id}/respond", post(respond))
        .route("/runs/{run}/result-snapshot", get(public_result_snapshot))
        .route("/runs/{run}/result-snapshot/file", get(public_result_file))
        .route("/runs/{run}/snapshot", get(public_input_snapshot))
}
pub(super) fn secret() -> Result<Zeroizing<String>> {
    let mut bytes = [0u8; 32];
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| invalid())?;
    Ok(Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes)))
}
async fn public_result_snapshot(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(run): Path<Uuid>,
) -> Answer {
    if s.service
        .store()
        .android_private_server_run(&a, run)
        .await?
        .is_some()
    {
        return Ok(Json(
            s.service
                .store()
                .android_private_result_snapshot(&a, run)
                .await?,
        ));
    }
    Ok(Json(
        s.service
            .store()
            .android_device_snapshot(&a, run, false)
            .await?,
    ))
}
async fn public_input_snapshot(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(run): Path<Uuid>,
) -> Answer {
    if let Some(run) = s
        .service
        .store()
        .android_private_server_run(&a, run)
        .await?
    {
        let id = run["spec"]["snapshot_id"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or(Error::NotFound)?;
        return Ok(Json(
            s.service
                .store()
                .android_private_snapshot(&a, id, false)
                .await?,
        ));
    }
    Ok(Json(
        s.service
            .store()
            .android_device_snapshot(&a, run, true)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotFile {
    path: String,
}
async fn public_result_file(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(run): Path<Uuid>,
    Query(q): Query<SnapshotFile>,
) -> std::result::Result<Response, Failure> {
    if s.service
        .store()
        .android_private_server_run(&a, run)
        .await?
        .is_some()
    {
        let bytes = s
            .service
            .store()
            .android_private_result_file(&a, run, &q.path, s.keys.as_ref())
            .await?;
        return Ok((
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=private-result",
                ),
            ],
            bytes.to_vec(),
        )
            .into_response());
    }
    let bytes = s
        .service
        .store()
        .android_device_snapshot_file(&a, run, false, &q.path)
        .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=device-result",
            ),
        ],
        bytes,
    )
        .into_response())
}
struct Agent(AndroidAgentPrincipal);
impl FromRequestParts<CompatState> for Agent {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        s: &CompatState,
    ) -> std::result::Result<Self, Response> {
        let mut headers = parts.headers.get_all(header::AUTHORIZATION).iter();
        let token = headers
            .next()
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        let segments = parts.uri.path().split('/').collect::<Vec<_>>();
        let id = segments
            .windows(2)
            .find(|v| v[0] == "executors")
            .and_then(|v| Uuid::parse_str(v[1]).ok());
        if headers.next().is_some() || token.is_none() || id.is_none() {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"device_authentication_required"})),
            )
                .into_response());
        }
        s.service
            .store()
            .android_agent_authenticate(id.unwrap_or_default(), token.unwrap_or_default())
            .await
            .map(Self)
            .map_err(|_| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error":"device_authentication_required"})),
                )
                    .into_response()
            })
    }
}
async fn list(State(s): State<CompatState>, Session(a): Session) -> Answer {
    Ok(Json(json!(s.service.store().android_devices(&a).await?)))
}
async fn pair(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(v): Json<PairDeviceExecutor>,
) -> Answer {
    Ok(Json(s.service.store().android_pair_device(&a, v).await?))
}
async fn pair_original(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(v): Json<DeviceExecutorRegistration>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_pair_device(
                &a,
                PairDeviceExecutor {
                    registration: v,
                    team_id: None,
                },
            )
            .await?,
    ))
}
async fn update(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(v): Json<UpdateDeviceExecutor>,
) -> Answer {
    s.service.store().android_update_device(&a, id, v).await?;
    Ok(Json(json!({"schema_version":2,"accepted":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewCredential {
    label: String,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
}
async fn credential(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(v): Json<NewCredential>,
) -> Answer {
    let token = secret()?;
    Ok(Json(
        s.service
            .store()
            .android_create_device_credential(
                &a,
                id,
                &v.label,
                v.expires_at.map(|v| v.timestamp_millis()),
                &token,
            )
            .await?,
    ))
}
async fn credentials(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
) -> Answer {
    Ok(Json(json!(
        s.service.store().android_device_credentials(&a, id).await?
    )))
}
async fn revoke(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((id, c)): Path<(Uuid, Uuid)>,
) -> Answer {
    s.service
        .store()
        .android_revoke_device_credential(&a, id, c)
        .await?;
    Ok(Json(json!({"schema_version":2,"accepted":true})))
}
async fn register(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Json(v): Json<DeviceExecutorRegistration>,
) -> Answer {
    Ok(Json(s.service.store().android_agent_register(&a, v).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Heartbeat {
    protocol_version: u16,
    active_run_ids: Vec<Uuid>,
    health: Value,
}
async fn heartbeat(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Json(v): Json<Heartbeat>,
) -> Answer {
    if v.health.to_string().len() > 64 * 1024 {
        return Err(invalid().into());
    }
    Ok(Json(
        s.service
            .store()
            .android_agent_heartbeat(&a, v.protocol_version, &v.active_run_ids)
            .await?,
    ))
}
async fn claim(State(s): State<CompatState>, Agent(a): Agent) -> Answer {
    Ok(Json(json!(
        s.service.store().android_device_claim(&a).await?
    )))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lease {
    lease_token: Uuid,
}
async fn lease_heartbeat(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, id)): Path<(Uuid, Uuid)>,
    Json(v): Json<Lease>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_device_lease_heartbeat(&a, id, v.lease_token)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentEvent {
    lease_token: Uuid,
    source_event_id: Option<Uuid>,
    kind: String,
    #[serde(default)]
    payload: Value,
}
async fn event(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, id)): Path<(Uuid, Uuid)>,
    Json(v): Json<AgentEvent>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_device_append_event(
                &a,
                id,
                v.lease_token,
                v.source_event_id,
                &v.kind,
                v.payload,
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Complete {
    lease_token: Uuid,
    result: Value,
    result_snapshot_manifest_id: Option<Uuid>,
    #[serde(default)]
    result_diff_summary: Value,
}
async fn complete(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, id)): Path<(Uuid, Uuid)>,
    Json(v): Json<Complete>,
) -> Answer {
    if v.result_diff_summary.to_string().len() > 128 * 1024 {
        return Err(invalid().into());
    }
    Ok(Json(
        s.service
            .store()
            .android_device_finish_with_diff(
                &a,
                id,
                v.lease_token,
                true,
                v.result,
                v.result_snapshot_manifest_id,
                v.result_diff_summary,
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fail {
    lease_token: Uuid,
    error: Value,
}
async fn fail(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, id)): Path<(Uuid, Uuid)>,
    Json(v): Json<Fail>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_device_finish(&a, id, v.lease_token, false, v.error, None)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    lease_token: Uuid,
    source_request_id: Option<Uuid>,
    requested_action: Value,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
}
async fn approval(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, id)): Path<(Uuid, Uuid)>,
    Json(v): Json<Approval>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_agent_request(
                &a,
                id,
                v.lease_token,
                "approval",
                v.source_request_id,
                v.requested_action,
                v.expires_at.map(|v| v.timestamp_millis()),
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    lease_token: Uuid,
    source_request_id: Option<Uuid>,
    prompt: Value,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
}
async fn input(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, id)): Path<(Uuid, Uuid)>,
    Json(v): Json<Input>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_agent_request(
                &a,
                id,
                v.lease_token,
                "input",
                v.source_request_id,
                v.prompt,
                v.expires_at.map(|v| v.timestamp_millis()),
            )
            .await?,
    ))
}
async fn approval_get(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, run, id)): Path<(Uuid, Uuid, Uuid)>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_agent_request_status(&a, run, id, "approval")
            .await?,
    ))
}
async fn input_get(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, run, id)): Path<(Uuid, Uuid, Uuid)>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_agent_request_status(&a, run, id, "input")
            .await?,
    ))
}
async fn approval_list(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(run): Path<Uuid>,
) -> Answer {
    if s.service
        .store()
        .android_private_server_run(&a, run)
        .await?
        .is_some()
    {
        return Ok(Json(json!(
            s.service
                .store()
                .android_private_loop_requests(&a, run, "approval")
                .await?
        )));
    }
    Ok(Json(json!(
        s.service
            .store()
            .android_device_requests(&a, run, "approval")
            .await?
    )))
}
async fn input_list(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(run): Path<Uuid>,
) -> Answer {
    if s.service
        .store()
        .android_private_server_run(&a, run)
        .await?
        .is_some()
    {
        return Ok(Json(json!(
            s.service
                .store()
                .android_private_loop_requests(&a, run, "input")
                .await?
        )));
    }
    Ok(Json(json!(
        s.service
            .store()
            .android_device_requests(&a, run, "input")
            .await?
    )))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resolve {
    expected_revision: i64,
    decision: String,
}
async fn resolve(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((run, id)): Path<(Uuid, Uuid)>,
    Json(v): Json<Resolve>,
) -> Answer {
    if s.service
        .store()
        .android_private_server_run(&a, run)
        .await?
        .is_some()
    {
        return Ok(Json(
            s.service
                .store()
                .android_private_loop_resolve(
                    &a,
                    run,
                    id,
                    "approval",
                    v.expected_revision,
                    json!(v.decision),
                    s.keys.as_ref(),
                )
                .await?,
        ));
    }
    Ok(Json(
        s.service
            .store()
            .android_device_resolve(
                &a,
                run,
                id,
                "approval",
                v.expected_revision,
                json!(v.decision),
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Respond {
    expected_revision: i64,
    response: Value,
}
async fn respond(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((run, id)): Path<(Uuid, Uuid)>,
    Json(v): Json<Respond>,
) -> Answer {
    if s.service
        .store()
        .android_private_server_run(&a, run)
        .await?
        .is_some()
    {
        return Ok(Json(
            s.service
                .store()
                .android_private_loop_resolve(
                    &a,
                    run,
                    id,
                    "input",
                    v.expected_revision,
                    v.response,
                    s.keys.as_ref(),
                )
                .await?,
        ));
    }
    Ok(Json(
        s.service
            .store()
            .android_device_resolve(&a, run, id, "input", v.expected_revision, v.response)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    lease_token: Uuid,
    source_checkpoint_id: Option<Uuid>,
    safe_to_resume: bool,
    executor_state: Value,
}
async fn checkpoint(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, run)): Path<(Uuid, Uuid)>,
    Json(v): Json<Checkpoint>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_agent_checkpoint(
                &a,
                run,
                v.lease_token,
                v.source_checkpoint_id.unwrap_or_else(Uuid::new_v4),
                v.safe_to_resume,
                v.executor_state,
            )
            .await?,
    ))
}
fn leased(h: &HeaderMap) -> Result<Uuid> {
    let mut values = h.get_all("x-cowork-lease-token").iter();
    let id = values
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or(Error::Forbidden)?;
    if values.next().is_some() {
        return Err(Error::Forbidden);
    }
    Ok(id)
}
async fn snapshot(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, run)): Path<(Uuid, Uuid)>,
    h: HeaderMap,
) -> Answer {
    if let Some((manifest, _)) = s
        .service
        .store()
        .android_agent_private_snapshot(&a, run, leased(&h)?, None, s.keys.as_ref())
        .await?
    {
        return Ok(Json(manifest));
    }
    Ok(Json(
        s.service
            .store()
            .android_agent_get_snapshot(&a, run, leased(&h)?, true)
            .await?
            .ok_or(Error::NotFound)?,
    ))
}
async fn result_snapshot(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, run)): Path<(Uuid, Uuid)>,
    h: HeaderMap,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_agent_get_snapshot(&a, run, leased(&h)?, false)
            .await?
    )))
}
async fn begin_snapshot(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, run)): Path<(Uuid, Uuid)>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_agent_begin_snapshot(&a, run, leased(&h)?, v)
            .await?,
    ))
}
async fn commit_snapshot(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, run, id)): Path<(Uuid, Uuid, Uuid)>,
    h: HeaderMap,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_agent_commit_snapshot(&a, run, leased(&h)?, id)
            .await?,
    ))
}
async fn chunk_put(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, run, id, digest)): Path<(Uuid, Uuid, Uuid, String)>,
    h: HeaderMap,
    bytes: Bytes,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_agent_snapshot_chunk(&a, run, leased(&h)?, id, &digest, &bytes)
            .await?,
    ))
}
async fn chunk_get(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, run, digest)): Path<(Uuid, Uuid, String)>,
    h: HeaderMap,
) -> std::result::Result<Response, Failure> {
    if let Some((_, bytes)) = s
        .service
        .store()
        .android_agent_private_snapshot(&a, run, leased(&h)?, Some(&digest), s.keys.as_ref())
        .await?
    {
        return Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response());
    }
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream")],
        s.service
            .store()
            .android_agent_get_chunk(&a, run, leased(&h)?, &digest)
            .await?,
    )
        .into_response())
}
#[derive(Deserialize)]
struct Artifact {
    path: String,
    source: String,
    source_event_id: Option<Uuid>,
}
async fn artifact(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, run)): Path<(Uuid, Uuid)>,
    Query(q): Query<Artifact>,
    h: HeaderMap,
    bytes: Bytes,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_agent_artifact(
                &a,
                run,
                leased(&h)?,
                &q.path,
                &q.source,
                q.source_event_id,
                &bytes,
            )
            .await?,
    ))
}
async fn connect(
    State(s): State<CompatState>,
    Agent(a): Agent,
    ws: WebSocketUpgrade,
) -> std::result::Result<Response, Failure> {
    let guard = s.devices.hub.connection()?;
    Ok(ws
        .max_message_size(1_100_000)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            agent_socket(s, a, socket).await;
        })
        .into_response())
}
async fn agent_socket(s: CompatState, a: AndroidAgentPrincipal, mut socket: WebSocket) {
    use crate::android_device_channel::send;
    let Ok((owner, mut commands)) = s.devices.hub.install(&a).await else {
        let _ = tokio::time::timeout(Duration::from_secs(2), socket.close()).await;
        return;
    };
    let stop = s.devices.hub.stopped();
    let mut timer = tokio::time::interval(Duration::from_secs(5));
    if send(&mut socket,Message::Text(json!({"type":"hello","schema_version":2,"executor_id":a.executor_id,"heartbeat_interval_seconds":10}).to_string().into())).await {
        'connection: loop { tokio::select! {
            _=stop.cancelled()=>break,
            _=timer.tick()=>{if s.service.store().android_hub_renew(&a,&owner).await.is_err() || s.service.store().android_agent_heartbeat(&a,2,&[]).await.is_err(){break}},
            v=commands.recv()=>{let Some(v)=v else{break};if s.service.store().android_hub_current(&owner).await.is_err() || !send(&mut socket,Message::Text(v.to_string().into())).await{break}},
            v=socket.next()=>match v {
                Some(Ok(Message::Text(raw)))=>{
                    if s.service.store().android_hub_current(&owner).await.is_err(){break}
                    let Ok(v)=serde_json::from_str::<Value>(&raw)else{break};
                    match ws_operation(&s,&a,v).await {
                        Ok(values)=>{for value in values{if !send(&mut socket,Message::Text(value.to_string().into())).await{break 'connection}}},
                        Err(_)=>{let _=send(&mut socket,Message::Text(json!({"type":"error","code":"device_request_rejected","message":"Device request is not currently authorized","run_id":null}).to_string().into())).await;break}
                    }
                },
                Some(Ok(Message::Ping(v)))=>{if !send(&mut socket,Message::Pong(v)).await{break}},
                Some(Ok(Message::Pong(_)))=>{},
                _=>break,
            }
        }}
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.close()).await;
    s.devices.hub.remove(&owner).await;
}
async fn ws_operation(s: &CompatState, a: &AndroidAgentPrincipal, v: Value) -> Result<Vec<Value>> {
    let kind = v["type"].as_str().ok_or_else(invalid)?;
    let mut output = Vec::new();
    let run = v["run_id"].as_str().and_then(|v| Uuid::parse_str(v).ok());
    match kind {
        "heartbeat" => {
            let h: Heartbeat =
                serde_json::from_value(v["heartbeat"].clone()).map_err(|_| invalid())?;
            s.service
                .store()
                .android_agent_heartbeat(a, h.protocol_version, &h.active_run_ids)
                .await?;
            if h.active_run_ids.is_empty()
                && let Some(lease) = s.service.store().android_device_claim(a).await?
            {
                output.push(json!({"type":"lease","lease":lease}))
            }
        }
        "lease_heartbeat" => {
            let token = v["lease_token"]
                .as_str()
                .and_then(|v| Uuid::parse_str(v).ok())
                .ok_or_else(invalid)?;
            s.service
                .store()
                .android_device_lease_heartbeat(a, run.ok_or_else(invalid)?, token)
                .await?;
        }
        "event" => {
            let q: AgentEvent =
                serde_json::from_value(v["request"].clone()).map_err(|_| invalid())?;
            s.service
                .store()
                .android_device_append_event(
                    a,
                    run.ok_or_else(invalid)?,
                    q.lease_token,
                    q.source_event_id,
                    &q.kind,
                    q.payload,
                )
                .await?;
        }
        "complete" => {
            let q: Complete =
                serde_json::from_value(v["request"].clone()).map_err(|_| invalid())?;
            s.service
                .store()
                .android_device_finish_with_diff(
                    a,
                    run.ok_or_else(invalid)?,
                    q.lease_token,
                    true,
                    q.result,
                    q.result_snapshot_manifest_id,
                    q.result_diff_summary,
                )
                .await?;
        }
        "fail" => {
            let q: Fail = serde_json::from_value(v["request"].clone()).map_err(|_| invalid())?;
            s.service
                .store()
                .android_device_finish(
                    a,
                    run.ok_or_else(invalid)?,
                    q.lease_token,
                    false,
                    q.error,
                    None,
                )
                .await?;
        }
        _ => return Err(invalid()),
    }
    output.push(json!({"type":"ack","operation":kind,"run_id":run}));
    Ok(output)
}
async fn desktop_stream(
    State(s): State<CompatState>,
    Agent(a): Agent,
    Path((_, id)): Path<(Uuid, Uuid)>,
    ws: WebSocketUpgrade,
) -> std::result::Result<Response, Failure> {
    let guard = s.devices.hub.connection()?;
    Ok(ws
        .max_message_size(16 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            let _ = s
                .devices
                .hub
                .receive_stream(s.service.store(), &a, id, socket)
                .await;
        })
        .into_response())
}
