//! Owner intent only. Original signed completion is fetched by the controller.
use crate::*;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/orders/{order}/runs/{run}/managed-processes/{attempt}/reconcile",
            post(reconcile),
        )
        .layer(middleware::map_response(no_store))
}

async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn reconcile(
    State(s): State<AppState>,
    Path((order, run, attempt)): Path<(String, String, String)>,
    body: Input<ReconcileOwnerManagedProcess>,
) -> Answer<OwnerManagedProcessReconciliation> {
    Ok(Json(
        s.runtime
            .reconcile_owner_managed_process(&order, &run, &attempt, input(body)?)
            .await?,
    ))
}
