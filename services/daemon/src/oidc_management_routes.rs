//! Exact existing management/v1 SSO contract over actual Actor authority.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, header},
    routing::get,
};
use cowork_contracts::AndroidOidcSsoWrite;
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{Store, team_service::identity_error};
use std::sync::Arc;
use uuid::Uuid;
#[derive(Clone)]
struct SsoState {
    store: Store,
    keys: Arc<dyn KeyStore>,
    key: KeyId,
}
pub(crate) fn router(store: Store, keys: Arc<dyn KeyStore>, key: KeyId) -> Router {
    Router::new()
        .route(
            "/management/v1/organisations/{organization}/sso",
            get(read).put(write),
        )
        .layer(DefaultBodyLimit::max(65536))
        .with_state(SsoState { store, keys, key })
}
async fn actor(
    s: &SsoState,
    h: &HeaderMap,
) -> Result<cowork_identity::Principal, crate::ApiFailure> {
    if h.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return Err(cowork_domain::Error::Forbidden.into());
    }
    let token = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(cowork_domain::Error::Forbidden)?;
    Ok(s.store
        .identity()
        .authenticate_session(token)
        .await
        .map_err(identity_error)?)
}
async fn read(
    State(s): State<SsoState>,
    Path(org): Path<Uuid>,
    h: HeaderMap,
) -> Result<Json<cowork_contracts::AndroidOidcSsoSettings>, crate::ApiFailure> {
    let a = actor(&s, &h).await?;
    Ok(Json(
        s.store
            .identity()
            .android_oidc_sso_settings(&a, org)
            .await
            .map_err(identity_error)?,
    ))
}
async fn write(
    State(s): State<SsoState>,
    Path(org): Path<Uuid>,
    h: HeaderMap,
    Json(q): Json<AndroidOidcSsoWrite>,
) -> Result<Json<cowork_contracts::AndroidOidcSsoSettings>, crate::ApiFailure> {
    let a = actor(&s, &h).await?;
    let mut ids = h.get_all("idempotency-key").iter();
    let first = ids.next();
    if ids.next().is_some() {
        return Err(cowork_domain::Error::Forbidden.into());
    }
    let operation = if let Some(id) = first {
        let value = id.to_str().map_err(|_| cowork_domain::Error::Forbidden)?;
        let id = Uuid::parse_str(value).map_err(|_| cowork_domain::Error::Forbidden)?;
        if id.is_nil() || id.to_string() != value {
            return Err(cowork_domain::Error::Forbidden.into());
        }
        id
    } else {
        Uuid::new_v4()
    };
    Ok(Json(
        s.store
            .identity()
            .android_oidc_sso_write(&a, org, operation, &q, s.keys.as_ref(), s.key)
            .await
            .map_err(identity_error)?,
    ))
}
