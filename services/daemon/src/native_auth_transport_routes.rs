//! Dual-authority native bridge. Not mounted in tenant, browser or Android routes.
use super::*;
pub(super) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/auth-transports", get(pending).post(prepare))
        .route("/auth-transports/{operation}", get(read))
        .route("/auth-transports/capture", post(capture))
        .route("/auth-transports/resolve", post(resolve))
        .layer(middleware::from_fn_with_state(
            state,
            native_folders::authorize_native,
        ))
}
async fn pending(State(s): State<AppState>) -> Answer<Vec<NativeAuthTransportView>> {
    Ok(Json(s.runtime.native_auth_transport_pending().await?))
}
async fn prepare(
    State(s): State<AppState>,
    body: Input<NativeAuthTransportRequest>,
) -> Answer<NativeAuthTransportView> {
    Ok(Json(
        s.runtime
            .native_auth_transport_prepare(&input(body)?)
            .await?,
    ))
}
async fn read(
    State(s): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Answer<NativeAuthTransportReplay> {
    Ok(Json(s.runtime.native_auth_transport_read(id).await?))
}
async fn capture(
    State(s): State<AppState>,
    body: Input<NativeAuthTransportCapture>,
) -> Answer<NativeAuthTransportView> {
    Ok(Json(
        s.runtime
            .native_auth_transport_capture(&input(body)?)
            .await?,
    ))
}
async fn resolve(
    State(s): State<AppState>,
    body: Input<NativeAuthTransportResolution>,
) -> Answer<NativeAuthTransportView> {
    Ok(Json(
        s.runtime
            .native_auth_transport_resolve(&input(body)?)
            .await?,
    ))
}
