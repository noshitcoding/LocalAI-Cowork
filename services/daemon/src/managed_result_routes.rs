//! Owner-only local result views. GET never invokes the remote receiver.
use crate::*;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/orders/{order}/runs/{run}/managed-results", get(list))
        .route(
            "/orders/{order}/runs/{run}/managed-results/{attempt}/receive",
            post(receive),
        )
        .route(
            "/orders/{order}/runs/{run}/managed-results/{attempt}/delivery",
            get(delivery_history),
        )
        .route(
            "/orders/{order}/runs/{run}/managed-results/{attempt}/delivery/prepare",
            post(prepare_delivery),
        )
        .route(
            "/orders/{order}/runs/{run}/managed-results/{attempt}/delivery/confirm",
            post(confirm_delivery),
        )
        .route(
            "/orders/{order}/runs/{run}/managed-results/{attempt}/delivery/retirement/refresh",
            post(refresh_retirement),
        )
        .route(
            "/orders/{order}/runs/{run}/managed-result-versions/{version}/manifest",
            get(manifest),
        )
        .route(
            "/orders/{order}/runs/{run}/managed-result-versions/{version}/chunk",
            get(chunk),
        )
        .layer(middleware::map_response(no_store))
}

async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn list(
    State(s): State<AppState>,
    Path((order, run)): Path<(String, String)>,
) -> Answer<OwnerManagedResultList> {
    Ok(Json(s.runtime.owner_managed_results(&order, &run).await?))
}

async fn receive(
    State(s): State<AppState>,
    Path((order, run, attempt)): Path<(String, String, String)>,
) -> Answer<OwnerManagedResultEntry> {
    Ok(Json(
        s.runtime
            .receive_owner_managed_result(&order, &run, &attempt)
            .await?,
    ))
}

async fn manifest(
    State(s): State<AppState>,
    Path((order, run, version)): Path<(String, String, String)>,
) -> Answer<OwnerManagedResultManifest> {
    Ok(Json(
        s.runtime
            .owner_managed_result_manifest(&order, &run, &version)
            .await?,
    ))
}

async fn delivery_history(
    State(s): State<AppState>,
    Path((order, run, attempt)): Path<(String, String, String)>,
) -> Answer<OwnerManagedResultDeliveryView> {
    Ok(Json(
        s.runtime
            .owner_managed_result_delivery_view(&order, &run, &attempt)
            .await?,
    ))
}
async fn prepare_delivery(
    State(s): State<AppState>,
    Path((order, run, attempt)): Path<(String, String, String)>,
) -> Answer<OwnerManagedResultDeliveryView> {
    Ok(Json(
        s.runtime
            .prepare_owner_managed_result_delivery_view(&order, &run, &attempt)
            .await?,
    ))
}
async fn confirm_delivery(
    State(s): State<AppState>,
    Path((order, run, attempt)): Path<(String, String, String)>,
    Json(input): Json<ConfirmOwnerManagedResultDelivery>,
) -> Answer<OwnerManagedResultDeliveryView> {
    Ok(Json(
        s.runtime
            .confirm_owner_managed_result_delivery_view(&order, &run, &attempt, &input)
            .await?,
    ))
}
async fn refresh_retirement(
    State(s): State<AppState>,
    Path((order, run, attempt)): Path<(String, String, String)>,
) -> Answer<OwnerManagedResultDeliveryView> {
    Ok(Json(
        s.runtime
            .refresh_owner_managed_result_retirement_view(&order, &run, &attempt)
            .await?,
    ))
}

async fn chunk(
    State(s): State<AppState>,
    Path((order, run, version)): Path<(String, String, String)>,
    Query(input): Query<ReadWorkspaceTransferChunk>,
) -> Result<Response, ApiFailure> {
    Ok(binary_response(
        s.runtime
            .owner_managed_result_chunk(&order, &run, &version, input)
            .await?,
    ))
}
