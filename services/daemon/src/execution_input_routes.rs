//! Owner-authenticated local preparation and metadata. No input byte route.
use crate::*;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/orders/{order}/execution-inputs", post(prepare))
        .route("/orders/{order}/execution-inputs/{snapshot}", get(inspect))
        .route(
            "/orders/{order}/execution-inputs/{snapshot}/retire",
            post(retire),
        )
}
async fn prepare(
    State(s): State<AppState>,
    Path(order): Path<String>,
    body: Input<PrepareExecutionInput>,
) -> Answer<PreparedExecutionInput> {
    Ok(Json(
        s.runtime
            .prepare_execution_input(&order, &input(body)?)
            .await?,
    ))
}
async fn inspect(
    State(s): State<AppState>,
    Path((order, snapshot)): Path<(String, String)>,
) -> Answer<PreparedExecutionInput> {
    Ok(Json(
        s.runtime.inspect_execution_input(&order, &snapshot).await?,
    ))
}
async fn retire(
    State(s): State<AppState>,
    Path((order, snapshot)): Path<(String, String)>,
    body: Input<RetireExecutionInput>,
) -> Answer<PreparedExecutionInput> {
    Ok(Json(
        s.runtime
            .retire_execution_input(&order, &snapshot, &input(body)?)
            .await?,
    ))
}
