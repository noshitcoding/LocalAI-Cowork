//! Source-scoped retained snapshot transfer. The owner middleware supplies authority.
use crate::*;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/orders/{order}/runs/{run}/workspace/transfer",
            get(manifest),
        )
        .route(
            "/orders/{order}/runs/{run}/workspace/transfer/chunk",
            get(chunk),
        )
}

async fn manifest(
    State(s): State<AppState>,
    Path((order, run)): Path<(String, String)>,
) -> Answer<WorkspaceTransferManifest> {
    Ok(Json(
        s.runtime.workspace_transfer_manifest(&order, &run).await?,
    ))
}

async fn chunk(
    State(s): State<AppState>,
    Path((order, run)): Path<(String, String)>,
    Query(input): Query<ReadWorkspaceTransferChunk>,
) -> Result<Response, ApiFailure> {
    Ok(binary_response(
        s.runtime
            .workspace_transfer_chunk(&order, &run, input)
            .await?,
    ))
}
