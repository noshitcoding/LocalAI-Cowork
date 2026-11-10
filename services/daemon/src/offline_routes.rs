//! Team grant routes require tenant sessions. Local secret/execution routes additionally
//! require the native-only private header and reject browser Origin headers.
use super::*;
use axum::{extract::FromRequestParts, http::request::Parts};
use cowork_identity::Principal;
use cowork_storage::{offline_execution::OfflineAuthority, team_service::TeamService};
use uuid::Uuid;

#[derive(Clone)]
struct OfflineServerState {
    teams: TeamService,
    authority: Arc<OfflineAuthority>,
}
pub(crate) fn team_router(teams: TeamService, authority: Arc<OfflineAuthority>) -> Router {
    let state = OfflineServerState { teams, authority };
    Router::new()
        .route(
            "/api/v2/team-offline/{team}/{project}/grants",
            get(grants).post(issue),
        )
        .route(
            "/api/v2/team-offline/{team}/{project}/grants/{grant}/revoke",
            post(revoke),
        )
        .route(
            "/api/v2/team-offline/{team}/{project}/history",
            get(history),
        )
        .route(
            "/api/v2/team-offline/{team}/{project}/reconcile",
            post(reconcile),
        )
        .route("/api/v2/offline-authority/status", post(grant_status))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .with_state(state)
}
struct Session(Principal);
impl FromRequestParts<OfflineServerState> for Session {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &OfflineServerState,
    ) -> Result<Self, Response> {
        let mut headers = parts.headers.get_all(header::AUTHORIZATION).iter();
        let token = headers
            .next()
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|v| !v.is_empty())
            .ok_or_else(unauthorized)?;
        if headers.next().is_some() {
            return Err(unauthorized());
        }
        state
            .teams
            .store()
            .identity()
            .authenticate_session(token)
            .await
            .map(Self)
            .map_err(|_| unauthorized())
    }
}
async fn issue(
    State(s): State<OfflineServerState>,
    Session(a): Session,
    Path((t, p)): Path<(Uuid, Uuid)>,
    body: Input<IssueOfflineGrant>,
) -> Answer<SignedOfflineGrant> {
    Ok(Json(
        s.teams
            .store()
            .issue_offline_grant(&s.authority, &a, t, p, input(body)?)
            .await?,
    ))
}
async fn grants(
    State(s): State<OfflineServerState>,
    Session(a): Session,
    Path((t, p)): Path<(Uuid, Uuid)>,
) -> Answer<Vec<SignedOfflineGrant>> {
    Ok(Json(s.teams.store().offline_grants(&a, t, p).await?))
}
async fn revoke(
    State(s): State<OfflineServerState>,
    Session(a): Session,
    Path((t, p, g)): Path<(Uuid, Uuid, Uuid)>,
) -> Answer<serde_json::Value> {
    s.teams.store().revoke_offline_grant(&a, t, p, g).await?;
    Ok(Json(
        serde_json::json!({"revoked":true,"offline_revocation_takes_effect_on_reconnect":true}),
    ))
}
async fn reconcile(
    State(s): State<OfflineServerState>,
    Session(a): Session,
    Path((t, p)): Path<(Uuid, Uuid)>,
    body: Input<OfflineRunExport>,
) -> Answer<OfflineReconciliationReceipt> {
    Ok(Json(
        s.teams
            .store()
            .reconcile_offline_run(&a, t, p, input(body)?)
            .await?,
    ))
}
async fn history(
    State(s): State<OfflineServerState>,
    Session(a): Session,
    Path((t, p)): Path<(Uuid, Uuid)>,
) -> Answer<Vec<OfflineReconciliationReceipt>> {
    Ok(Json(
        s.teams
            .store()
            .offline_reconciliation_history(&a, t, p)
            .await?,
    ))
}
async fn grant_status(
    State(s): State<OfflineServerState>,
    body: Input<OfflineGrantStatusRequest>,
) -> Answer<SignedOfflineGrantStatus> {
    Ok(Json(
        s.teams
            .store()
            .offline_grant_status(&s.authority, input(body)?)
            .await?,
    ))
}

pub(super) fn native_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/offline/device", post(device))
        .route("/offline/device/proof", post(proof))
        .route("/offline/grants", get(local_grants))
        .route("/offline/grants/{grant}", get(local_ticket))
        .route("/offline/grants/install", post(install))
        .route("/offline/model-key", post(credential))
        .route("/offline/tool-config", post(tool_configuration))
        .route(
            "/offline/grants/{grant}/tool-config",
            get(tool_configuration_view),
        )
        .route("/offline/runs", get(local_runs).post(start))
        .route("/offline/runs/{run}", get(detail))
        .route("/offline/runs/{run}/approve", post(approve))
        .route("/offline/runs/{run}/cancel", post(cancel))
        .route(
            "/offline/runs/{run}/workspace-file",
            get(workspace_file).post(workspace_upload),
        )
        .route("/offline/runs/{run}/export", get(export))
        .route(
            "/offline/grants/{grant}/status-request",
            post(status_request),
        )
        .route("/offline/grants/status", post(record_status))
        .merge(super::offline_mcp_routes::router(state.clone()))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(
            state,
            native_folders::authorize_native,
        ))
}
async fn device(State(s): State<AppState>) -> Answer<OfflineDevice> {
    Ok(Json(s.runtime.offline_device().await?))
}
async fn proof(
    State(s): State<AppState>,
    body: Input<OfflineDeviceProofRequest>,
) -> Answer<OfflineDeviceProof> {
    Ok(Json(s.runtime.offline_device_proof(input(body)?).await?))
}
async fn install(
    State(s): State<AppState>,
    body: Input<InstallOfflineGrant>,
) -> Answer<SignedOfflineGrant> {
    Ok(Json(s.runtime.offline_install(input(body)?).await?))
}
async fn local_grants(State(s): State<AppState>) -> Answer<Vec<OfflineLocalGrantView>> {
    Ok(Json(s.runtime.store.offline_local_grant_views().await?))
}
async fn local_ticket(
    State(s): State<AppState>,
    Path(g): Path<Uuid>,
) -> Answer<SignedOfflineGrant> {
    Ok(Json(s.runtime.store.offline_local_ticket(g).await?))
}
async fn credential(
    State(s): State<AppState>,
    body: Input<ConfigureOfflineCredential>,
) -> Answer<serde_json::Value> {
    s.runtime.offline_configure_credential(input(body)?).await?;
    Ok(Json(serde_json::json!({"configured":true})))
}
async fn tool_configuration(
    State(s): State<AppState>,
    body: Input<ConfigureOfflineTools>,
) -> Answer<OfflineToolConfigurationView> {
    Ok(Json(s.runtime.offline_configure_tools(input(body)?).await?))
}
async fn tool_configuration_view(
    State(s): State<AppState>,
    Path(g): Path<Uuid>,
) -> Answer<OfflineToolConfigurationView> {
    Ok(Json(s.runtime.offline_tool_configuration(g).await?))
}
async fn cancel(State(s): State<AppState>, Path(r): Path<Uuid>) -> Answer<OfflineRun> {
    Ok(Json(s.runtime.offline_cancel(r).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceFileQuery {
    path: String,
}
async fn workspace_file(
    State(s): State<AppState>,
    Path(r): Path<Uuid>,
    Query(q): Query<WorkspaceFileQuery>,
) -> Result<Response, ApiFailure> {
    let bytes = s.runtime.offline_workspace_file(r, q.path).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response())
}
async fn workspace_upload(
    State(s): State<AppState>,
    Path(r): Path<Uuid>,
    body: Input<OfflineWorkspaceUpload>,
) -> Answer<OfflineRun> {
    Ok(Json(
        s.runtime.offline_workspace_upload(r, input(body)?).await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    cursor: Option<String>,
}
async fn local_runs(
    State(s): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> Answer<OfflineRunPage> {
    Ok(Json(
        s.runtime.store.offline_local_run_page(q.cursor).await?,
    ))
}
async fn detail(State(s): State<AppState>, Path(r): Path<Uuid>) -> Answer<OfflineRun> {
    Ok(Json(s.runtime.store.offline_local_execution(r).await?.run))
}
async fn start(State(s): State<AppState>, body: Input<StartOfflineRun>) -> Answer<OfflineRun> {
    Ok(Json(s.runtime.offline_start(input(body)?).await?))
}
async fn approve(
    State(s): State<AppState>,
    Path(r): Path<Uuid>,
    body: Input<ApproveOfflineTools>,
) -> Answer<OfflineRun> {
    Ok(Json(
        s.runtime.offline_approve_tools(r, input(body)?).await?,
    ))
}
async fn export(State(s): State<AppState>, Path(r): Path<Uuid>) -> Answer<OfflineRunExport> {
    Ok(Json(s.runtime.offline_export_run(r).await?))
}
async fn status_request(
    State(s): State<AppState>,
    Path(g): Path<Uuid>,
) -> Answer<OfflineGrantStatusRequest> {
    Ok(Json(s.runtime.offline_status_request(g).await?))
}
async fn record_status(
    State(s): State<AppState>,
    body: Input<SignedOfflineGrantStatus>,
) -> Answer<OfflineGrantStatus> {
    Ok(Json(
        s.runtime
            .store
            .record_offline_grant_status(input(body)?)
            .await?,
    ))
}
