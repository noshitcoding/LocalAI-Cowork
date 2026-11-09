//! Identity-owned reads and native-only operator qualification mutations.
use super::*;
use axum::http::HeaderMap;
use cowork_identity::Principal;
use cowork_storage::Store;
use uuid::Uuid;

#[derive(Clone)]
struct ActorState(Store);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    project_id: Uuid,
    profile_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Project {
    project_id: Uuid,
}

async fn actor(store: &Store, headers: &HeaderMap, native: bool) -> Result<Principal, ApiFailure> {
    let name = if native {
        "x-cowork-actor-session"
    } else {
        "authorization"
    };
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .and_then(|v| v.to_str().ok())
        .ok_or(Error::Forbidden)?;
    if values.next().is_some() {
        return Err(Error::Forbidden.into());
    }
    let token = if native {
        value
    } else {
        value.strip_prefix("Bearer ").ok_or(Error::Forbidden)?
    };
    if token.is_empty() || token.len() > 4096 {
        return Err(Error::Forbidden.into());
    }
    store
        .identity()
        .authenticate_session(token)
        .await
        .map_err(|_| Error::Forbidden.into())
}
async fn private_headers(request: Request, next: Next) -> Response {
    let mut result = next.run(request).await;
    result
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    result.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    result
}

pub(super) fn native_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/native-actor-model-registry/projects",
            get(native_projects),
        )
        .route(
            "/native-actor-model-registry/profiles",
            get(native_profiles),
        )
        .route("/native-actor-model-registry/context", get(native_context))
        .route(
            "/native-actor-model-registry/authority",
            get(native_authority),
        )
        .route("/native-actor-model-registry/entries", get(native_entries))
        .route("/native-actor-model-registry/import", post(import))
        .route("/native-actor-model-registry/revoke", post(revoke))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(middleware::from_fn(private_headers))
        .layer(middleware::from_fn_with_state(
            state,
            native_folders::authorize_native,
        ))
}
/// No public write route accepts qualification artifacts or caller-declared proof.
pub(super) fn actor_router(store: Store) -> Router {
    Router::new()
        .route("/api/v2/actor-model-registry/projects", get(projects))
        .route("/api/v2/actor-model-registry/profiles", get(profiles))
        .route("/api/v2/actor-model-registry/context", get(current_context))
        .route("/api/v2/actor-model-registry/entries", get(entries))
        .layer(middleware::from_fn(private_headers))
        .with_state(ActorState(store))
}
async fn projects(State(s): State<ActorState>, headers: HeaderMap) -> Answer<Vec<Value>> {
    let actor = actor(&s.0, &headers, false).await?;
    Ok(Json(s.0.android_personal_projects(&actor).await?))
}
async fn profiles(State(s): State<ActorState>, headers: HeaderMap) -> Answer<Vec<Value>> {
    let actor = actor(&s.0, &headers, false).await?;
    Ok(Json(s.0.android_personal_providers(&actor).await?))
}
async fn current_context(
    State(s): State<ActorState>,
    headers: HeaderMap,
    Query(q): Query<Selection>,
) -> Answer<ActorModelContext> {
    let actor = actor(&s.0, &headers, false).await?;
    Ok(Json(
        s.0.actor_model_context(&actor, q.project_id, q.profile_id)
            .await?,
    ))
}
async fn entries(
    State(s): State<ActorState>,
    headers: HeaderMap,
    Query(q): Query<Project>,
) -> Answer<Vec<ActorModelRegistryView>> {
    let actor = actor(&s.0, &headers, false).await?;
    Ok(Json(s.0.actor_model_registry(&actor, q.project_id).await?))
}
async fn native_projects(State(s): State<AppState>, headers: HeaderMap) -> Answer<Vec<Value>> {
    let actor = actor(&s.runtime.store, &headers, true).await?;
    Ok(Json(
        s.runtime.store.android_personal_projects(&actor).await?,
    ))
}
async fn native_profiles(State(s): State<AppState>, headers: HeaderMap) -> Answer<Vec<Value>> {
    let actor = actor(&s.runtime.store, &headers, true).await?;
    Ok(Json(
        s.runtime.store.android_personal_providers(&actor).await?,
    ))
}
async fn native_context(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Selection>,
) -> Answer<ActorModelContext> {
    let actor = actor(&s.runtime.store, &headers, true).await?;
    Ok(Json(
        s.runtime
            .store
            .actor_model_context(&actor, q.project_id, q.profile_id)
            .await?,
    ))
}
async fn native_authority(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Project>,
) -> Answer<Value> {
    let actor = actor(&s.runtime.store, &headers, true).await?;
    Ok(Json(
        s.runtime
            .store
            .actor_model_registry_authority(&actor, q.project_id)
            .await?,
    ))
}
async fn native_entries(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Project>,
) -> Answer<Vec<ActorModelRegistryView>> {
    let actor = actor(&s.runtime.store, &headers, true).await?;
    Ok(Json(
        s.runtime
            .store
            .actor_model_registry(&actor, q.project_id)
            .await?,
    ))
}
async fn import(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Input<ImportActorModelQualification>,
) -> Answer<ActorModelRegistryView> {
    let actor = actor(&s.runtime.store, &headers, true).await?;
    Ok(Json(
        s.runtime
            .store
            .import_actor_model_qualification(&actor, &input(body)?)
            .await?,
    ))
}
async fn revoke(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Input<RevokeActorModelQualification>,
) -> Answer<Value> {
    let actor = actor(&s.runtime.store, &headers, true).await?;
    s.runtime
        .store
        .revoke_actor_model_qualification(&actor, &input(body)?)
        .await?;
    Ok(Json(json!({"revoked":true})))
}
