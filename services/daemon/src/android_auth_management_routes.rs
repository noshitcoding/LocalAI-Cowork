//! Owner-authorized operator setup, outside the tenant and V1 session routers.
use super::*;
pub(super) fn router() -> Router<AppState> {
    Router::new().route("/operator/auth-security", get(status).put(configure))
}
async fn status(State(s): State<AppState>) -> Answer<Value> {
    Ok(Json(s.runtime.store.android_management_policy().await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configure {
    operation_id: uuid::Uuid,
    expected_revision: i64,
    admin_required: bool,
}
async fn configure(State(s): State<AppState>, body: Input<Configure>) -> Answer<Value> {
    let v = input(body)?;
    Ok(Json(
        s.runtime
            .store
            .android_management_set_policy(v.operation_id, v.expected_revision, v.admin_required)
            .await?,
    ))
}
