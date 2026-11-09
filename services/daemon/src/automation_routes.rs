//! Personal-owner management, preview and event submission.
use super::*;
use axum::http::HeaderMap;

enum AutomationFailure {
    Api(ApiFailure),
    TooLarge,
}
impl From<Error> for AutomationFailure {
    fn from(error: Error) -> Self {
        Self::Api(ApiFailure(error))
    }
}
impl From<ApiFailure> for AutomationFailure {
    fn from(error: ApiFailure) -> Self {
        Self::Api(error)
    }
}
impl IntoResponse for AutomationFailure {
    fn into_response(self) -> Response {
        match self {
            Self::Api(error) => error.into_response(),
            Self::TooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(ApiError {
                    code: "payload_too_large".into(),
                    message: "Automation request exceeds 48 KiB".into(),
                    retryable: false,
                    correlation_id: id(),
                }),
            )
                .into_response(),
        }
    }
}
type AutomationAnswer<T> = std::result::Result<Json<T>, AutomationFailure>;
fn automation_input<T>(body: Input<T>) -> std::result::Result<T, AutomationFailure> {
    match body {
        Err(error) if error.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            Err(AutomationFailure::TooLarge)
        }
        body => input(body).map_err(AutomationFailure::Api),
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/automation-triggers",
            get(triggers).post(write_trigger),
        )
        .route(
            "/projects/{project}/automation-triggers/{trigger}/preview",
            post(preview),
        )
        .route(
            "/projects/{project}/automation-triggers/{trigger}/events",
            get(events).post(submit),
        )
        .route(
            "/projects/{project}/automation-events/{event}/retire",
            post(retire),
        )
        .route(
            "/projects/{project}/automation-triggers/{trigger}/webhook",
            get(webhook_grant).put(configure_webhook),
        )
        .route(
            "/projects/{project}/automation-triggers/{trigger}/file-watch",
            get(file_watch_status),
        )
        .layer(DefaultBodyLimit::max(48 * 1024))
}

pub(super) fn public_router() -> Router<AppState> {
    Router::new()
        .route("/api/automation-webhooks/{trigger}", post(webhook))
        .layer(DefaultBodyLimit::max(48 * 1024))
}

async fn webhook_grant(
    State(state): State<AppState>,
    Path((project, trigger)): Path<(String, String)>,
) -> AutomationAnswer<Option<AutomationWebhookGrant>> {
    Ok(Json(
        state
            .runtime
            .store
            .automation_webhook_grant(&project, &trigger)
            .await?,
    ))
}
async fn configure_webhook(
    State(state): State<AppState>,
    Path((project, trigger)): Path<(String, String)>,
    body: Input<ConfigureAutomationWebhook>,
) -> AutomationAnswer<AutomationWebhookGrant> {
    Ok(Json(
        state
            .runtime
            .store
            .configure_automation_webhook(&project, &trigger, &automation_input(body)?)
            .await?,
    ))
}
async fn webhook(
    State(state): State<AppState>,
    Path(trigger): Path<String>,
    headers: HeaderMap,
    body: Input<SubmitAutomationEvent>,
) -> AutomationAnswer<AutomationEvent> {
    // Server-to-server ingress never accepts browser Origins or query-string credentials.
    if headers.contains_key(header::ORIGIN) {
        return Err(ApiFailure(Error::Forbidden).into());
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiFailure(Error::Forbidden))?;
    Ok(Json(
        state
            .runtime
            .store
            .submit_automation_webhook(&trigger, token, &automation_input(body)?)
            .await?,
    ))
}

async fn triggers(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> AutomationAnswer<Vec<AutomationTrigger>> {
    Ok(Json(
        state.runtime.store.automation_triggers(&project).await?,
    ))
}
async fn write_trigger(
    State(state): State<AppState>,
    Path(project): Path<String>,
    body: Input<WriteAutomationTrigger>,
) -> AutomationAnswer<AutomationTrigger> {
    Ok(Json(
        state
            .runtime
            .write_automation_trigger(&project, &automation_input(body)?)
            .await?,
    ))
}
async fn file_watch_status(
    State(state): State<AppState>,
    Path((project, trigger)): Path<(String, String)>,
) -> AutomationAnswer<AutomationFileWatchStatus> {
    Ok(Json(
        state
            .runtime
            .store
            .automation_file_watch_status(&project, &trigger)
            .await?,
    ))
}
async fn preview(
    State(state): State<AppState>,
    Path((project, trigger)): Path<(String, String)>,
    body: Input<SubmitAutomationEvent>,
) -> AutomationAnswer<AutomationEventPreview> {
    Ok(Json(
        state
            .runtime
            .store
            .preview_automation_event(&project, &trigger, &automation_input(body)?)
            .await?,
    ))
}
async fn submit(
    State(state): State<AppState>,
    Path((project, trigger)): Path<(String, String)>,
    body: Input<SubmitAutomationEvent>,
) -> AutomationAnswer<AutomationEvent> {
    Ok(Json(
        state
            .runtime
            .store
            .submit_automation_event(
                &project,
                &trigger,
                AutomationEventSource::Manual,
                &automation_input(body)?,
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    before: Option<i64>,
    limit: Option<i64>,
}
async fn events(
    State(state): State<AppState>,
    Path((project, trigger)): Path<(String, String)>,
    Query(query): Query<HistoryQuery>,
) -> AutomationAnswer<AutomationEventPage> {
    Ok(Json(
        state
            .runtime
            .store
            .automation_events(&project, &trigger, query.before, query.limit.unwrap_or(50))
            .await?,
    ))
}
async fn retire(
    State(state): State<AppState>,
    Path((project, event)): Path<(String, String)>,
    body: Input<RetireAutomationEvent>,
) -> AutomationAnswer<AutomationEvent> {
    Ok(Json(
        state
            .runtime
            .store
            .retire_automation_event(&project, &event, &automation_input(body)?)
            .await?,
    ))
}
