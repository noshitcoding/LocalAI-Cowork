use super::*;

pub(super) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/folders", get(list).post(register))
        .route("/folder-requests", get(lookup))
        .route("/folder-revocations", post(revoke))
        .layer(middleware::from_fn_with_state(state, authorize_native))
}

pub(super) async fn authorize_native(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    // A browser Origin is rejected even if an incorrectly configured client supplied both keys.
    let Some(expected) = state.native_token.as_deref().filter(|t| t.len() >= 48) else {
        return ApiFailure(Error::Forbidden).into_response();
    };
    let actual = request
        .headers()
        .get("x-cowork-native")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let mut different = actual.len() ^ expected.len();
    for (a, b) in actual.as_bytes().iter().zip(expected.as_bytes()) {
        different |= (*a ^ *b) as usize;
    }
    if different != 0 || request.headers().contains_key(header::ORIGIN) {
        return ApiFailure(Error::Forbidden).into_response();
    }
    next.run(request).await
}

async fn broker(
    state: &AppState,
    project: &str,
) -> Result<Arc<cowork_broker::FileBroker>, ApiFailure> {
    if !state
        .runtime
        .store
        .projects()
        .await?
        .iter()
        .any(|p| p.id == project)
    {
        return Err(Error::NotFound.into());
    }
    state
        .runtime
        .files
        .clone()
        .ok_or_else(|| Error::Forbidden.into())
}

async fn list(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Answer<Vec<RegisteredFolder>> {
    let files = broker(&state, &query.project_id).await?;
    let folders = tokio::task::spawn_blocking(move || files.registered_folders(&query.project_id))
        .await
        .map_err(|_| Error::Storage("Native registry task failed".into()))??;
    Ok(Json(folders))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LookupQuery {
    project_id: String,
    key: String,
}

async fn lookup(
    State(state): State<AppState>,
    Query(query): Query<LookupQuery>,
) -> Answer<FolderRequestLookup> {
    cowork_domain::valid_text(&query.key, 128)?;
    let files = broker(&state, &query.project_id).await?;
    let folder = tokio::task::spawn_blocking(move || {
        files.registration_for_request(&query.project_id, &query.key)
    })
    .await
    .map_err(|_| Error::Storage("Native registry task failed".into()))??;
    Ok(Json(FolderRequestLookup { folder }))
}

async fn register(
    State(state): State<AppState>,
    body: Input<NativeFolderSelection>,
) -> Answer<RegisteredFolder> {
    let input = input(body)?;
    let files = broker(&state, &input.project_id).await?;
    let folder = tokio::task::spawn_blocking(move || files.register_native_folder(&input))
        .await
        .map_err(|_| Error::UncertainEffect)??;
    Ok(Json(folder))
}

async fn revoke(State(state): State<AppState>, body: Input<RevokeFolder>) -> Answer<FolderGrant> {
    let input = input(body)?;
    let files = broker(&state, &input.project_id).await?;
    let grant = tokio::task::spawn_blocking(move || {
        files.revoke_native_folder(
            &input.project_id,
            &input.folder_id,
            &input.expected_revision,
        )
    })
    .await
    .map_err(|_| Error::UncertainEffect)??;
    Ok(Json(grant))
}
