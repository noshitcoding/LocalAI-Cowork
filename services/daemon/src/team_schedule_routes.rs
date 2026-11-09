//! Team schedule CRUD/history use current persisted tenant sessions, never owner tokens.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State},
    http::{StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, put},
};
use cowork_domain::Error;
use cowork_identity::Principal;
use cowork_storage::{
    DeleteTeamSchedule, PauseTeamSchedule, TeamSchedule, TeamScheduleDeleted, TeamScheduleHistory,
    WriteTeamSchedule, team_service::TeamService,
};
use serde::Deserialize;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone)]
struct ScheduleState {
    teams: TeamService,
    limit: Arc<Mutex<(Instant, u32)>>,
}
pub(crate) fn router(teams: TeamService) -> Router {
    let state = ScheduleState {
        teams,
        limit: Arc::new(Mutex::new((Instant::now(), 0))),
    };
    Router::new()
        .route(
            "/api/v2/team-schedules/{team}/{project}",
            get(list).post(write),
        )
        .route(
            "/api/v2/team-schedules/{team}/{project}/{schedule}",
            delete(remove),
        )
        .route(
            "/api/v2/team-schedules/{team}/{project}/{schedule}/pause",
            put(pause),
        )
        .route(
            "/api/v2/team-schedules/{team}/{project}/{schedule}/history",
            get(history),
        )
        .layer(DefaultBodyLimit::max(512 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), protect))
        .with_state(state)
}
fn failure(status: StatusCode, error: &'static str) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Json(serde_json::json!({"error":error})),
    )
        .into_response()
}
struct Failure(Error);
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self(error)
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        match self.0 {
            Error::Validation(_) => failure(
                StatusCode::BAD_REQUEST,
                "invalid_schedule_or_execution_grant",
            ),
            Error::Forbidden | Error::NotFound => failure(StatusCode::NOT_FOUND, "access_denied"),
            Error::Conflict | Error::UncertainEffect => failure(
                StatusCode::CONFLICT,
                "schedule_revision_or_active_occurrence_conflict",
            ),
            _ => failure(StatusCode::SERVICE_UNAVAILABLE, "schedule_unavailable"),
        }
    }
}
async fn protect(
    State(state): State<ScheduleState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let allowed = match state.limit.lock() {
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
        Err(_) => false,
    };
    if !allowed {
        return failure(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        "nosniff".parse().expect("static header"),
    );
    response
}
struct Session(Principal);
impl FromRequestParts<ScheduleState> for Session {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &ScheduleState,
    ) -> Result<Self, Response> {
        let mut headers = parts.headers.get_all(header::AUTHORIZATION).iter();
        let token = headers
            .next()
            .and_then(|h| h.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|v| !v.is_empty())
            .ok_or_else(|| failure(StatusCode::UNAUTHORIZED, "authentication_required"))?;
        if headers.next().is_some() {
            return Err(failure(StatusCode::UNAUTHORIZED, "authentication_required"));
        }
        state
            .teams
            .store()
            .identity()
            .authenticate_session(token)
            .await
            .map(Self)
            .map_err(|_| failure(StatusCode::UNAUTHORIZED, "authentication_required"))
    }
}
type Answer<T> = Result<Json<T>, Failure>;
async fn list(
    State(state): State<ScheduleState>,
    Session(actor): Session,
    Path((team, project)): Path<(Uuid, Uuid)>,
) -> Answer<Vec<TeamSchedule>> {
    Ok(Json(
        state
            .teams
            .store()
            .team_schedules(&actor, team, project)
            .await?,
    ))
}
async fn write(
    State(state): State<ScheduleState>,
    Session(actor): Session,
    Path((team, project)): Path<(Uuid, Uuid)>,
    Json(input): Json<WriteTeamSchedule>,
) -> Answer<TeamSchedule> {
    Ok(Json(
        state
            .teams
            .store()
            .team_write_schedule(&actor, team, project, input)
            .await?,
    ))
}
async fn pause(
    State(state): State<ScheduleState>,
    Session(actor): Session,
    Path((team, project, schedule)): Path<(Uuid, Uuid, Uuid)>,
    Json(input): Json<PauseTeamSchedule>,
) -> Answer<TeamSchedule> {
    Ok(Json(
        state
            .teams
            .store()
            .team_pause_schedule(&actor, team, project, schedule, input)
            .await?,
    ))
}
async fn remove(
    State(state): State<ScheduleState>,
    Session(actor): Session,
    Path((team, project, schedule)): Path<(Uuid, Uuid, Uuid)>,
    Json(input): Json<DeleteTeamSchedule>,
) -> Answer<TeamScheduleDeleted> {
    Ok(Json(
        state
            .teams
            .store()
            .team_delete_schedule(&actor, team, project, schedule, input)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    #[serde(default)]
    cursor: i64,
    #[serde(default = "default_limit")]
    limit: i64,
}
fn default_limit() -> i64 {
    50
}
async fn history(
    State(state): State<ScheduleState>,
    Session(actor): Session,
    Path((team, project, schedule)): Path<(Uuid, Uuid, Uuid)>,
    Query(input): Query<HistoryQuery>,
) -> Answer<TeamScheduleHistory> {
    Ok(Json(
        state
            .teams
            .store()
            .team_schedule_history(&actor, team, project, schedule, input.cursor, input.limit)
            .await?,
    ))
}
