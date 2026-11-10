//! Authenticated tenant sessions only; daemon/native bearer is not a Principal.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Path, Query, Request, State},
    http::{StatusCode, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use cowork_contracts::*;
use cowork_domain::Error;
use cowork_identity::{
    ContentAction, IdentityError, Membership, OrganizationRole, Permissions, Principal,
};
use cowork_storage::team_service::{TeamService, identity_error};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;
#[path = "team_container_recovery_routes.rs"]
mod container_recovery;
#[path = "team_knowledge_access_routes.rs"]
mod knowledge_access;
#[path = "team_lost_host_recovery_routes.rs"]
mod lost_host_recovery;
#[path = "team_mcp_oauth_routes.rs"]
mod mcp_oauth;
#[path = "team_recovery_routes.rs"]
mod recovery;

#[derive(Clone)]
struct TeamState {
    service: TeamService,
    limit: Arc<Mutex<(Instant, u32)>>,
}
pub(crate) fn router(service: TeamService) -> Router {
    let state = TeamState {
        service,
        limit: Arc::new(Mutex::new((Instant::now(), 0))),
    };
    Router::new()
        .route("/", get(teams).post(create_team))
        .route("/{team}", get(team).put(update_team))
        .route("/{team}/membership", get(self_member))
        .route("/{team}/members", get(members).post(add_member))
        .route(
            "/{team}/members/{member}",
            axum::routing::put(update_member),
        )
        .route("/{team}/provider", get(provider).put(configure_provider))
        .route("/{team}/github", get(github).put(configure_github))
        .route(
            "/{team}/mcp-servers",
            get(mcp_servers).post(create_mcp_server),
        )
        .route(
            "/{team}/mcp-servers/{server}",
            axum::routing::put(update_mcp_server),
        )
        .route(
            "/{team}/mcp-servers/{server}/discover",
            axum::routing::post(discover_mcp_server),
        )
        .route("/{team}/mcp-servers/{server}/tools", get(mcp_tools))
        .route("/{team}/tools", get(available_tools))
        .route("/{team}/projects", get(projects).post(create_project))
        .route(
            "/{team}/projects/{project}",
            get(project).put(update_project),
        )
        .route(
            "/{team}/projects/{project}/grants",
            get(project_grants).put(grant_project),
        )
        .route(
            "/{team}/projects/{project}/orders",
            get(orders).post(create_order),
        )
        .route("/{team}/orders/{order}", axum::routing::put(update_order))
        .route(
            "/{team}/projects/{project}/documents",
            get(documents).post(save_document),
        )
        .route(
            "/{team}/projects/{project}/documents/{document}/revisions",
            get(document_revisions),
        )
        .route(
            "/{team}/projects/{project}/sync",
            get(sync_project).post(sync_apply),
        )
        .route("/{team}/runs", axum::routing::post(create_run))
        .route("/{team}/projects/{project}/runs", get(runs))
        .route("/{team}/runs/{run}", get(run))
        .route("/{team}/runs/{run}/steps", get(steps))
        .route("/{team}/runs/{run}/cancel", axum::routing::post(cancel_run))
        .route("/{team}/runs/{run}/tool-approvals", get(tool_approvals))
        .route(
            "/{team}/runs/{run}/tool-approvals/{approval}/approve",
            axum::routing::post(approve_tool),
        )
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .merge(knowledge_access::routes())
        .merge(recovery::routes())
        .merge(container_recovery::routes())
        .merge(lost_host_recovery::routes())
        .merge(mcp_oauth::routes())
        .layer(middleware::from_fn_with_state(state.clone(), protect))
        .with_state(state)
}
#[derive(Serialize)]
struct ApiError {
    error: &'static str,
}
fn failure(status: StatusCode, error: &'static str) -> Response {
    let mut response = (status, Json(ApiError { error })).into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().expect("static header"));
    response.headers_mut().insert(
        "x-content-type-options",
        "nosniff".parse().expect("static header"),
    );
    response
}
struct Failure(Error);
impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Self(e)
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        match self.0 {
            Error::Validation(message) if message == "knowledge_base_disabled" => {
                failure(StatusCode::FORBIDDEN, "knowledge_base_disabled")
            }
            Error::Validation(_) => failure(StatusCode::BAD_REQUEST, "invalid_request"),
            Error::Forbidden | Error::NotFound => failure(StatusCode::NOT_FOUND, "access_denied"),
            Error::Conflict => failure(StatusCode::CONFLICT, "request_conflict"),
            Error::UncertainEffect => failure(StatusCode::CONFLICT, "unconfirmed_original_effects"),
            Error::NativeKeyUnavailable => {
                failure(StatusCode::SERVICE_UNAVAILABLE, "team_key_unavailable")
            }
            _ => failure(StatusCode::SERVICE_UNAVAILABLE, "team_unavailable"),
        }
    }
}
type Answer<T> = std::result::Result<Json<T>, Failure>;
async fn github(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
) -> Answer<Option<TeamGithubConfiguration>> {
    Ok(Json(s.service.github(&a, t).await?))
}
async fn configure_github(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
    StrictJson(v): StrictJson<ConfigureTeamGithub>,
) -> Answer<TeamGithubConfiguration> {
    Ok(Json(s.service.configure_github(&a, t, v).await?))
}
async fn mcp_servers(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
) -> Answer<Vec<TeamMcpServer>> {
    Ok(Json(s.service.mcp_servers(&a, t).await?))
}
async fn create_mcp_server(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
    StrictJson(v): StrictJson<ConfigureTeamMcpServer>,
) -> Answer<TeamMcpServer> {
    Ok(Json(s.service.configure_mcp_server(&a, t, None, v).await?))
}
async fn update_mcp_server(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, id)): StrictPath<(Uuid, Uuid)>,
    StrictJson(v): StrictJson<ConfigureTeamMcpServer>,
) -> Answer<TeamMcpServer> {
    Ok(Json(
        s.service.configure_mcp_server(&a, t, Some(id), v).await?,
    ))
}
async fn discover_mcp_server(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, id)): StrictPath<(Uuid, Uuid)>,
) -> Answer<TeamMcpServer> {
    Ok(Json(s.service.discover_mcp_server(&a, t, id).await?))
}
async fn mcp_tools(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, id)): StrictPath<(Uuid, Uuid)>,
) -> Answer<Vec<TeamMcpTool>> {
    Ok(Json(s.service.mcp_tools(&a, t, id).await?))
}
async fn available_tools(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
) -> Answer<serde_json::Value> {
    Ok(Json(
        serde_json::json!({"names":s.service.available_tools(&a,t).await?}),
    ))
}
async fn protect(State(state): State<TeamState>, request: Request, next: Next) -> Response {
    let admitted = match state.limit.lock() {
        Ok(mut limit) => {
            if limit.0.elapsed() >= Duration::from_secs(60) {
                *limit = (Instant::now(), 0);
            }
            if limit.1 >= 600 {
                false
            } else {
                limit.1 += 1;
                true
            }
        }
        Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE, "team_unavailable"),
    };
    if !admitted {
        return failure(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().expect("static header"));
    response
}
struct Session(Principal);
impl FromRequestParts<TeamState> for Session {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &TeamState,
    ) -> std::result::Result<Self, Response> {
        let mut headers = parts.headers.get_all("authorization").iter();
        let bearer = headers
            .next()
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or_else(|| failure(StatusCode::UNAUTHORIZED, "authentication_required"))?;
        if headers.next().is_some() {
            return Err(failure(StatusCode::UNAUTHORIZED, "authentication_required"));
        }
        let actor = state
            .service
            .store()
            .identity()
            .authenticate_session(bearer)
            .await
            .map_err(|error| match error {
                IdentityError::Unauthenticated => {
                    failure(StatusCode::UNAUTHORIZED, "authentication_required")
                }
                _ => failure(StatusCode::SERVICE_UNAVAILABLE, "team_unavailable"),
            })?;
        let path = parts
            .extensions
            .get::<axum::extract::OriginalUri>()
            .map(|uri| uri.0.path())
            .unwrap_or(parts.uri.path());
        state
            .service
            .store()
            .identity()
            .android_management_request(&actor, parts.method.as_str(), path)
            .await
            .map_err(|_| failure(StatusCode::FORBIDDEN, "management_mfa_required"))?;
        Ok(Self(actor))
    }
}
struct StrictJson<T>(T);
impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for StrictJson<T> {
    type Rejection = Response;
    async fn from_request(request: Request, state: &S) -> std::result::Result<Self, Response> {
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(v)| Self(v))
            .map_err(|e| {
                failure(
                    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
                        StatusCode::PAYLOAD_TOO_LARGE
                    } else {
                        StatusCode::BAD_REQUEST
                    },
                    "invalid_request",
                )
            })
    }
}
struct StrictPath<T>(T);
impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for StrictPath<T> {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> std::result::Result<Self, Response> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(v)| Self(v))
            .map_err(|_| failure(StatusCode::BAD_REQUEST, "invalid_request"))
    }
}
fn role(value: TeamRole) -> OrganizationRole {
    match value {
        TeamRole::Owner => OrganizationRole::Owner,
        TeamRole::Admin => OrganizationRole::Admin,
        TeamRole::Member => OrganizationRole::Member,
        TeamRole::Viewer => OrganizationRole::Viewer,
    }
}
fn member(value: Membership) -> TeamMember {
    TeamMember {
        id: value.id,
        team_id: value.organization_id,
        identity_id: value.identity_id,
        role: match value.role {
            OrganizationRole::Owner => TeamRole::Owner,
            OrganizationRole::Admin => TeamRole::Admin,
            OrganizationRole::Member => TeamRole::Member,
            OrganizationRole::Viewer => TeamRole::Viewer,
        },
        active: value.active,
        revision: value.revision,
    }
}
async fn teams(State(s): State<TeamState>, Session(a): Session) -> Answer<Vec<Team>> {
    Ok(Json(s.service.teams(&a).await?))
}
async fn self_member(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
) -> Answer<TeamMember> {
    Ok(Json(s.service.self_member(&a, t).await?))
}
async fn create_team(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictJson(v): StrictJson<CreateTeam>,
) -> Answer<Team> {
    Ok(Json(s.service.create_team(&a, v).await?))
}
async fn team(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
) -> Answer<Team> {
    Ok(Json(s.service.team(&a, t).await?))
}
async fn update_team(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
    StrictJson(v): StrictJson<UpdateTeam>,
) -> Answer<Team> {
    Ok(Json(s.service.update_team(&a, t, v).await?))
}
async fn members(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
) -> Answer<Vec<TeamMember>> {
    Ok(Json(
        s.service
            .store()
            .identity()
            .members(&a, t)
            .await
            .map_err(identity_error)?
            .into_iter()
            .map(member)
            .collect(),
    ))
}
async fn add_member(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
    StrictJson(v): StrictJson<AddTeamMember>,
) -> Answer<TeamMember> {
    Ok(Json(member(
        s.service
            .store()
            .identity()
            .add_member(&a, t, v.identity_id, role(v.role))
            .await
            .map_err(identity_error)?,
    )))
}
async fn update_member(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, m)): StrictPath<(Uuid, Uuid)>,
    StrictJson(v): StrictJson<UpdateTeamMember>,
) -> Answer<TeamMember> {
    Ok(Json(member(
        s.service
            .store()
            .identity()
            .change_member(&a, t, m, v.expected_revision, role(v.role), v.active)
            .await
            .map_err(identity_error)?,
    )))
}
async fn provider(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
) -> Answer<Option<TeamProvider>> {
    Ok(Json(s.service.provider(&a, t).await?))
}
async fn configure_provider(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
    StrictJson(v): StrictJson<ConfigureTeamProvider>,
) -> Answer<TeamProvider> {
    Ok(Json(s.service.configure_provider(&a, t, v).await?))
}
async fn projects(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
) -> Answer<Vec<TeamProject>> {
    Ok(Json(s.service.projects(&a, t).await?))
}
async fn create_project(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
    StrictJson(v): StrictJson<CreateTeamProject>,
) -> Answer<TeamProject> {
    Ok(Json(s.service.create_project(&a, t, v).await?))
}
async fn project(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
) -> Answer<TeamProject> {
    Ok(Json(s.service.project(&a, t, p).await?))
}
async fn update_project(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
    StrictJson(v): StrictJson<UpdateTeamProject>,
) -> Answer<TeamProject> {
    Ok(Json(s.service.update_project(&a, t, p, v).await?))
}
async fn project_grants(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
) -> Answer<Vec<TeamProjectGrantView>> {
    Ok(Json(s.service.project_grants(&a, t, p).await?))
}
async fn grant_project(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
    StrictJson(v): StrictJson<TeamProjectGrant>,
) -> Answer<serde_json::Value> {
    let mut actions = Vec::new();
    if v.read {
        actions.push(ContentAction::Read);
    }
    if v.write {
        actions.push(ContentAction::Write);
    }
    if v.execute {
        actions.push(ContentAction::Execute);
    }
    if v.share {
        actions.push(ContentAction::Share);
    }
    let result = s
        .service
        .store()
        .identity()
        .grant_content(
            &a,
            t,
            p,
            v.membership_id,
            v.expected_revision,
            Permissions::from_actions(&actions),
            v.active,
        )
        .await
        .map_err(identity_error)?;
    Ok(Json(
        serde_json::to_value(result).map_err(cowork_domain::storage_error)?,
    ))
}
async fn orders(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
) -> Answer<Vec<TeamOrder>> {
    Ok(Json(s.service.orders(&a, t, p).await?))
}
async fn create_order(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
    StrictJson(v): StrictJson<CreateTeamOrder>,
) -> Answer<TeamOrder> {
    Ok(Json(s.service.create_order(&a, t, p, v).await?))
}
async fn update_order(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, o)): StrictPath<(Uuid, Uuid)>,
    StrictJson(v): StrictJson<UpdateTeamOrder>,
) -> Answer<TeamOrder> {
    Ok(Json(s.service.update_order(&a, t, o, v).await?))
}
async fn documents(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
) -> Answer<Vec<TeamDocument>> {
    Ok(Json(s.service.documents(&a, t, p).await?))
}
async fn save_document(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
    StrictJson(v): StrictJson<SaveTeamDocument>,
) -> Answer<TeamDocument> {
    Ok(Json(s.service.save_document(&a, t, p, v).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentRevisionQuery {
    #[serde(default)]
    after: i64,
    limit: Option<i64>,
}
async fn document_revisions(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p, d)): StrictPath<(Uuid, Uuid, Uuid)>,
    Query(q): Query<DocumentRevisionQuery>,
) -> Answer<Vec<KnowledgeRevision>> {
    Ok(Json(
        s.service
            .document_revisions(&a, t, p, d, q.after, q.limit.unwrap_or(100))
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncQuery {
    #[serde(default)]
    after: i64,
    limit: Option<i64>,
}
async fn sync_project(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
    Query(q): Query<SyncQuery>,
) -> Answer<cowork_storage::team_service::TeamSyncPage> {
    Ok(Json(
        s.service
            .sync_project(&a, t, p, q.after, q.limit.unwrap_or(100))
            .await?,
    ))
}
async fn sync_apply(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
    StrictJson(v): StrictJson<cowork_storage::team_service::TeamSyncMutation>,
) -> Answer<serde_json::Value> {
    Ok(Json(s.service.apply_sync(&a, t, p, v).await?))
}
async fn create_run(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
    StrictJson(v): StrictJson<CreateTeamRun>,
) -> Answer<TeamRun> {
    Ok(Json(s.service.create_run(&a, t, v).await?))
}
async fn runs(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
) -> Answer<Vec<TeamRun>> {
    Ok(Json(s.service.runs(&a, t, p).await?))
}
async fn run(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r)): StrictPath<(Uuid, Uuid)>,
) -> Answer<TeamRun> {
    Ok(Json(s.service.run(&a, t, r).await?))
}
async fn steps(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r)): StrictPath<(Uuid, Uuid)>,
) -> Answer<Vec<TeamRunStep>> {
    Ok(Json(s.service.run_steps(&a, t, r).await?))
}
async fn cancel_run(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r)): StrictPath<(Uuid, Uuid)>,
) -> Answer<TeamRun> {
    Ok(Json(s.service.cancel_run(&a, t, r).await?))
}
async fn tool_approvals(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r)): StrictPath<(Uuid, Uuid)>,
) -> Answer<Vec<TeamToolApproval>> {
    Ok(Json(s.service.tool_approvals(&a, t, r).await?))
}
async fn approve_tool(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r, id)): StrictPath<(Uuid, Uuid, Uuid)>,
    StrictJson(v): StrictJson<ApproveTeamTool>,
) -> Answer<TeamToolApproval> {
    Ok(Json(s.service.approve_tool(&a, t, r, id, v).await?))
}
