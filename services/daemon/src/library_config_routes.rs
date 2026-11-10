//! Shared wire shapes, deliberately separate owner and authenticated actor realms.
use super::*;
use axum::{Extension, http::HeaderMap};
use cowork_identity::Principal;
use cowork_storage::{Store, library_config::LibraryOwner};

#[derive(Clone)]
pub(super) enum RequestOwner {
    Local,
    Actor(Principal),
}
impl RequestOwner {
    pub(super) fn owner(&self) -> LibraryOwner<'_> {
        match self {
            Self::Local => LibraryOwner::Local,
            Self::Actor(a) => LibraryOwner::Actor(a),
        }
    }
}
fn routes() -> Router<Store> {
    Router::new()
        .route(
            "/library/personalities",
            get(personalities).post(create_personality),
        )
        .route(
            "/library/personalities/{id}",
            get(personality)
                .put(update_personality)
                .delete(delete_personality),
        )
        .route(
            "/library/agent-templates",
            get(templates).post(create_template),
        )
        .route(
            "/library/agent-templates/{id}",
            get(template).put(update_template).delete(delete_template),
        )
        .route("/preferences", get(preferences).put(write_preferences))
        .layer(DefaultBodyLimit::max(256 * 1024))
}
/// Caller merges this inside the existing owner bearer/origin middleware.
pub(super) fn owner_router(store: Store) -> Router<AppState> {
    routes()
        .layer(Extension(RequestOwner::Local))
        .with_state(store)
}
/// Actor routes never accept the owner bearer and never consult owner content.
pub(super) fn actor_router(store: Store) -> Router {
    Router::new()
        .nest("/api/v1", routes())
        .layer(middleware::from_fn_with_state(
            store.clone(),
            authenticate_actor,
        ))
        .with_state(store)
}
pub(super) async fn authenticate_actor(
    State(store): State<Store>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Response {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let token = values
        .next()
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    let principal = if values.next().is_none() {
        if let Some(token) = token.filter(|v| !v.is_empty() && v.len() <= 4096) {
            store.identity().authenticate_session(token).await.ok()
        } else {
            None
        }
    } else {
        None
    };
    let Some(principal) = principal else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiError {
                code: "authentication_required".into(),
                message: "Sign in with your own account".into(),
                retryable: false,
                correlation_id: id(),
            }),
        )
            .into_response();
    };
    request
        .extensions_mut()
        .insert(RequestOwner::Actor(principal));
    next.run(request).await
}
async fn personalities(
    State(s): State<Store>,
    Extension(o): Extension<RequestOwner>,
) -> Answer<Vec<Personality>> {
    Ok(Json(s.library_personalities(o.owner()).await?))
}
async fn personality(
    State(s): State<Store>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
) -> Answer<Personality> {
    Ok(Json(s.library_personality(o.owner(), &id).await?))
}
async fn create_personality(
    State(s): State<Store>,
    Extension(o): Extension<RequestOwner>,
    body: Input<WritePersonality>,
) -> Answer<Personality> {
    Ok(Json(
        s.library_write_personality(o.owner(), None, &input(body)?)
            .await?,
    ))
}
async fn update_personality(
    State(s): State<Store>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
    body: Input<WritePersonality>,
) -> Answer<Personality> {
    Ok(Json(
        s.library_write_personality(o.owner(), Some(&id), &input(body)?)
            .await?,
    ))
}
async fn delete_personality(
    State(s): State<Store>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
    body: Input<DeleteLibraryEntry>,
) -> Answer<LibraryDeleted> {
    Ok(Json(
        s.library_delete_personality(o.owner(), &id, &input(body)?)
            .await?,
    ))
}
async fn templates(
    State(s): State<Store>,
    Extension(o): Extension<RequestOwner>,
) -> Answer<Vec<AgentTemplate>> {
    Ok(Json(s.library_agent_templates(o.owner()).await?))
}
async fn template(
    State(s): State<Store>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
) -> Answer<AgentTemplate> {
    Ok(Json(s.library_agent_template(o.owner(), &id).await?))
}
async fn create_template(
    State(s): State<Store>,
    Extension(o): Extension<RequestOwner>,
    body: Input<WriteAgentTemplate>,
) -> Answer<AgentTemplate> {
    Ok(Json(
        s.library_write_agent_template(o.owner(), None, &input(body)?)
            .await?,
    ))
}
async fn update_template(
    State(s): State<Store>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
    body: Input<WriteAgentTemplate>,
) -> Answer<AgentTemplate> {
    Ok(Json(
        s.library_write_agent_template(o.owner(), Some(&id), &input(body)?)
            .await?,
    ))
}
async fn delete_template(
    State(s): State<Store>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
    body: Input<DeleteLibraryEntry>,
) -> Answer<LibraryDeleted> {
    Ok(Json(
        s.library_delete_agent_template(o.owner(), &id, &input(body)?)
            .await?,
    ))
}
async fn preferences(
    State(s): State<Store>,
    Extension(o): Extension<RequestOwner>,
) -> Answer<PreferencesRecord> {
    Ok(Json(s.library_preferences(o.owner()).await?))
}
async fn write_preferences(
    State(s): State<Store>,
    Extension(o): Extension<RequestOwner>,
    body: Input<WritePreferences>,
) -> Answer<PreferencesRecord> {
    Ok(Json(
        s.library_write_preferences(o.owner(), &input(body)?)
            .await?,
    ))
}
