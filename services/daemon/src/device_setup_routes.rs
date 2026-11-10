//! Installation-owner setup, deliberately outside tenant/member authorization.
use super::*;
use crate::android_device_services::{AndroidDeviceServices, operator::Configure};
use axum::Extension;

pub(super) fn router(devices: Arc<AndroidDeviceServices>) -> Router<AppState> {
    Router::new()
        .route("/operator/devices", get(status).put(configure))
        .route("/operator/devices/check", post(check))
        .layer(Extension(devices))
}
async fn status(
    Extension(devices): Extension<Arc<AndroidDeviceServices>>,
) -> Json<serde_json::Value> {
    Json(devices.operator_configuration())
}
async fn configure(
    Extension(devices): Extension<Arc<AndroidDeviceServices>>,
    body: Input<Configure>,
) -> Answer<serde_json::Value> {
    Ok(Json(devices.configure(input(body)?).await?))
}
async fn check(
    Extension(devices): Extension<Arc<AndroidDeviceServices>>,
) -> Answer<serde_json::Value> {
    Ok(Json(devices.check_configuration().await?))
}
