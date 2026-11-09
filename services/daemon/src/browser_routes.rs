//! Owner browser UI; model calls still enter through the runtime approval path.
use super::*;
use crate::toolbox_routes::failure;
use axum::Extension;
use cowork_contracts::{
    BrowserCommand, BrowserInput, BrowserSessionView, BrowserTakeover, CreateBrowserSession,
    DeleteBrowserProfile,
};
use cowork_toolbox::Toolbox;

pub(super) fn router(service: Arc<Toolbox>) -> Router<AppState> {
    Router::new()
        .route("/toolbox/browser-connections", get(connections))
        .route(
            "/orders/{order}/toolbox/browser-v2/sessions",
            get(sessions).post(create),
        )
        .route(
            "/orders/{order}/toolbox/browser-v2/profile/delete",
            post(delete_profile),
        )
        .route(
            "/orders/{order}/toolbox/browser-v2/sessions/{session}/takeover",
            post(takeover),
        )
        .route(
            "/orders/{order}/toolbox/browser-v2/sessions/{session}/input",
            post(manual_input),
        )
        .route(
            "/orders/{order}/toolbox/browser-v2/sessions/{session}/frame",
            post(frame),
        )
        .route(
            "/orders/{order}/toolbox/browser-v2/sessions/{session}/tabs",
            get(tabs),
        )
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .layer(Extension(service))
}
async fn connections(
    Extension(service): Extension<Arc<Toolbox>>,
) -> Answer<Vec<cowork_contracts::BrowserExtensionConnection>> {
    let value = tokio::task::spawn_blocking(move || service.browser_connections())
        .await
        .map_err(|_| ApiFailure(Error::UncertainEffect))?;
    Ok(Json(value))
}
async fn sessions(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path(order): Path<String>,
) -> Answer<Vec<BrowserSessionView>> {
    state.runtime.store.native_chat_available(&order).await?;
    Ok(Json(service.browser_sessions_v2(&order).map_err(failure)?))
}
async fn create(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path(order): Path<String>,
    body: Input<CreateBrowserSession>,
) -> Answer<BrowserSessionView> {
    state.runtime.store.native_chat_available(&order).await?;
    let project = state.runtime.store.order(&order).await?.project_id;
    let options = input(body)?;
    let value =
        tokio::task::spawn_blocking(move || service.create_browser_v2(&project, &order, options))
            .await
            .map_err(|_| ApiFailure(Error::UncertainEffect))?
            .map_err(failure)?;
    Ok(Json(value))
}
async fn delete_profile(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path(order): Path<String>,
    body: Input<DeleteBrowserProfile>,
) -> Answer<Value> {
    state.runtime.store.native_chat_available(&order).await?;
    let project = state.runtime.store.order(&order).await?.project_id;
    let body = input(body)?;
    tokio::task::spawn_blocking(move || service.delete_browser_profile(&project, body.engine))
        .await
        .map_err(|_| ApiFailure(Error::UncertainEffect))?
        .map_err(failure)?;
    Ok(Json(json!({"deleted":true})))
}
async fn takeover(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path((order, id)): Path<(String, String)>,
    body: Input<BrowserTakeover>,
) -> Answer<BrowserSessionView> {
    state.runtime.store.native_chat_available(&order).await?;
    let body = input(body)?;
    let value =
        tokio::task::spawn_blocking(move || service.browser_takeover(&order, &id, body.manual))
            .await
            .map_err(|_| ApiFailure(Error::UncertainEffect))?
            .map_err(failure)?;
    Ok(Json(value))
}
async fn manual_input(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path((order, id)): Path<(String, String)>,
    body: Input<BrowserInput>,
) -> Answer<Value> {
    state.runtime.store.native_chat_available(&order).await?;
    let body = input(body)?;
    let value =
        tokio::task::spawn_blocking(move || service.browser_manual_input(&order, &id, body))
            .await
            .map_err(|_| ApiFailure(Error::UncertainEffect))?
            .map_err(failure)?;
    Ok(Json(value))
}
async fn frame(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path((order, id)): Path<(String, String)>,
) -> Answer<Value> {
    state.runtime.store.native_chat_available(&order).await?;
    let value = tokio::task::spawn_blocking(move || {
        service.browser_command_v2(
            &order,
            &id,
            BrowserCommand {
                operation: "frame".into(),
                ..Default::default()
            },
            true,
            &std::sync::atomic::AtomicBool::new(false),
        )
    })
    .await
    .map_err(|_| ApiFailure(Error::UncertainEffect))?
    .map_err(failure)?;
    Ok(Json(value))
}
async fn tabs(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path((order, id)): Path<(String, String)>,
) -> Answer<Value> {
    state.runtime.store.native_chat_available(&order).await?;
    let value = tokio::task::spawn_blocking(move || {
        service.browser_command_v2(
            &order,
            &id,
            BrowserCommand {
                operation: "tabs".into(),
                ..Default::default()
            },
            true,
            &std::sync::atomic::AtomicBool::new(false),
        )
    })
    .await
    .map_err(|_| ApiFailure(Error::UncertainEffect))?
    .map_err(failure)?;
    Ok(Json(value))
}
