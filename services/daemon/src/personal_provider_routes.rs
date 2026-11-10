//! Owner configuration returns metadata only; native path rejects browser Origin.
use super::*;
pub(super) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/providers/with-credential", post(configure))
        .merge(
            Router::new()
                .route("/native-providers/with-credential", post(configure))
                .layer(middleware::from_fn_with_state(
                    state,
                    native_folders::authorize_native,
                )),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}
async fn configure(
    State(state): State<AppState>,
    body: Input<ConfigurePersonalProvider>,
) -> Answer<ProviderProfile> {
    Ok(Json(
        state
            .runtime
            .configure_personal_provider(&input(body)?)
            .await?,
    ))
}
