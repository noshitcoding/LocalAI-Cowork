//! Tenant project routes use only persisted tenant sessions. The single-owner
//! daemon token never passes this router's Principal extractor.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Path, Request, State},
    http::{StatusCode, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use cowork_domain::Error;
use cowork_identity::{IdentityError, IdentityStore, Principal};
use cowork_storage::{CreateTenantProject, Store, TenantProject};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone)]
struct TenantState {
    store: Store,
    identity: IdentityStore,
    limiter: Arc<Mutex<(Instant, u32)>>,
}
pub(crate) fn router(store: Store, identity: IdentityStore) -> Router {
    let state = TenantState {
        store,
        identity,
        limiter: Arc::new(Mutex::new((Instant::now(), 0))),
    };
    Router::new()
        .route(
            "/organizations/{org}/projects",
            get(projects).post(create_project),
        )
        .route("/organizations/{org}/projects/{project}", get(project))
        .layer(DefaultBodyLimit::max(8 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), admission))
        .with_state(state)
}
#[derive(Serialize)]
struct TenantError {
    error: &'static str,
}
fn fail(status: StatusCode, error: &'static str) -> Response {
    let mut response = (status, Json(TenantError { error })).into_response();
    protect(&mut response);
    response
}
fn protect(response: &mut Response) {
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().expect("static header"));
    response.headers_mut().insert(
        "x-content-type-options",
        "nosniff".parse().expect("static header"),
    );
}
struct Failure(Error);
impl From<Error> for Failure {
    fn from(value: Error) -> Self {
        Self(value)
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        match self.0 {
            Error::Validation(_) => fail(StatusCode::BAD_REQUEST, "invalid_request"),
            Error::Forbidden | Error::NotFound => fail(StatusCode::NOT_FOUND, "access_denied"),
            Error::Conflict => fail(StatusCode::CONFLICT, "request_conflict"),
            _ => fail(StatusCode::SERVICE_UNAVAILABLE, "tenant_unavailable"),
        }
    }
}
type Answer<T> = std::result::Result<Json<T>, Failure>;
async fn admission(State(state): State<TenantState>, request: Request, next: Next) -> Response {
    let admitted = match state.limiter.lock() {
        Ok(mut limit) => {
            if limit.0.elapsed() >= Duration::from_secs(60) {
                *limit = (Instant::now(), 0);
            }
            if limit.1 >= 600 {
                false
            } else {
                limit.1 += 1;
                true
            }
        }
        Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE, "tenant_unavailable"),
    };
    if !admitted {
        return fail(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let mut response = next.run(request).await;
    protect(&mut response);
    response
}
struct Session(Principal);
impl FromRequestParts<TenantState> for Session {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &TenantState,
    ) -> std::result::Result<Self, Response> {
        let mut headers = parts.headers.get_all("authorization").iter();
        let first = headers
            .next()
            .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "authentication_required"))?;
        if headers.next().is_some() {
            return Err(fail(StatusCode::UNAUTHORIZED, "authentication_required"));
        }
        let bearer = first
            .to_str()
            .ok()
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "authentication_required"))?;
        let actor =
            state
                .identity
                .authenticate_session(bearer)
                .await
                .map_err(|error| match error {
                    IdentityError::Unauthenticated => {
                        fail(StatusCode::UNAUTHORIZED, "authentication_required")
                    }
                    _ => fail(StatusCode::SERVICE_UNAVAILABLE, "tenant_unavailable"),
                })?;
        let path = parts
            .extensions
            .get::<axum::extract::OriginalUri>()
            .map(|uri| uri.0.path())
            .unwrap_or(parts.uri.path());
        state
            .identity
            .android_management_request(&actor, parts.method.as_str(), path)
            .await
            .map_err(|_| fail(StatusCode::FORBIDDEN, "management_mfa_required"))?;
        Ok(Self(actor))
    }
}
struct StrictJson<T>(T);
impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for StrictJson<T> {
    type Rejection = Response;
    async fn from_request(request: Request, state: &S) -> std::result::Result<Self, Response> {
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|error| {
                let status = if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    StatusCode::BAD_REQUEST
                };
                fail(status, "invalid_request")
            })
    }
}
struct StrictPath<T>(T);
impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for StrictPath<T> {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> std::result::Result<Self, Response> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|_| fail(StatusCode::BAD_REQUEST, "invalid_request"))
    }
}
async fn projects(
    State(state): State<TenantState>,
    Session(actor): Session,
    StrictPath(org): StrictPath<Uuid>,
) -> Answer<Vec<TenantProject>> {
    Ok(Json(state.store.tenant_projects(&actor, org).await?))
}
async fn create_project(
    State(state): State<TenantState>,
    Session(actor): Session,
    StrictPath(org): StrictPath<Uuid>,
    StrictJson(input): StrictJson<CreateTenantProject>,
) -> Answer<TenantProject> {
    Ok(Json(
        state
            .store
            .create_tenant_project(&actor, org, input)
            .await?,
    ))
}
async fn project(
    State(state): State<TenantState>,
    Session(actor): Session,
    StrictPath((org, project)): StrictPath<(Uuid, Uuid)>,
) -> Answer<TenantProject> {
    Ok(Json(
        state.store.tenant_project(&actor, org, project).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use cowork_identity::{OrganizationRole, Permissions};
    use serde_json::{Value, json};
    use tower::ServiceExt;
    const BOOTSTRAP: &str = "http-tenant-bootstrap-32-synthetic-bytes-minimum";
    const SETTLEMENT: &str = "http-tenant-settlement-32-synthetic-bytes-minimum";
    const PASSWORD: &str = "HTTP-tenant-synthetic-password-not-real";

    async fn fixture() -> (tempfile::TempDir, Store, Router, String, Uuid) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&directory.path().join("tenant-api.sqlite3"))
            .await
            .unwrap();
        let identity = store.identity();
        identity
            .initialize_credentials(BOOTSTRAP, SETTLEMENT)
            .await
            .unwrap();
        let (org, issued) = identity
            .bootstrap(
                BOOTSTRAP,
                "http-tenant-owner",
                "Owner",
                PASSWORD,
                "Tenant route",
            )
            .await
            .unwrap();
        let app = Router::new().nest("/api/v2/tenant", router(store.clone(), identity));
        (directory, store, app, issued.token, org.id)
    }
    async fn call(
        app: &Router,
        method: &str,
        path: &str,
        token: &str,
        input: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("/api/v2/tenant{path}"))
            .header("authorization", format!("Bearer {token}"));
        if input.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let request = builder
            .body(Body::from(
                input
                    .map(|value| serde_json::to_vec(&value).unwrap())
                    .unwrap_or_default(),
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    #[tokio::test]
    async fn tenant_routes_create_real_scoped_projects_with_exact_idempotent_receipts() {
        let (_dir, store, app, token, org) = fixture().await;
        let path = format!("/organizations/{org}/projects");
        let input = json!({"name":"Tenant via HTTP", "idempotency_key":"http-project-key"});
        let (status, project) = call(&app, "POST", &path, &token, Some(input.clone())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(project["organization_id"], org.to_string());
        assert!(project.get("host_path").is_none());
        assert_eq!(
            call(&app, "POST", &path, &token, Some(input)).await.1,
            project
        );
        let (status, conflict) = call(
            &app,
            "POST",
            &path,
            &token,
            Some(json!({"name":"Changed", "idempotency_key":"http-project-key"})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(conflict, json!({"error":"request_conflict"}));
        assert_eq!(
            call(&app, "GET", &path, &token, None).await.1,
            json!([project.clone()])
        );
        assert_eq!(
            call(
                &app,
                "GET",
                &format!("{path}/{}", project["id"].as_str().unwrap()),
                &token,
                None
            )
            .await
            .1,
            project
        );
        assert!(store.projects().await.unwrap().is_empty());
        store.close().await;
    }
    #[tokio::test]
    async fn tenant_http_rejects_single_owner_authority_paths_and_unknown_fields() {
        let (_dir, store, app, token, org) = fixture().await;
        let path = format!("/organizations/{org}/projects");
        for credential in ["single-owner-daemon-bearer", BOOTSTRAP, SETTLEMENT] {
            let (status, body) = call(&app, "GET", &path, credential, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body, json!({"error":"authentication_required"}));
        }
        for unexpected in ["host_path", "id", "organization_id"] {
            let mut input = json!({"name":"Malicious path", "idempotency_key":"path-adoption"});
            input[unexpected] = json!("C:\\Users\\Riege");
            let (status, body) = call(&app, "POST", &path, &token, Some(input)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body, json!({"error":"invalid_request"}));
        }
        let (status, _) = call(
            &app,
            "POST",
            &path,
            &token,
            Some(json!({"name":"x".repeat(9 * 1024),"idempotency_key":"oversized"})),
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        store.close().await;
    }
    #[tokio::test]
    async fn tenant_route_acl_isolation_cannot_be_bypassed_by_org_admin_role() {
        let (_dir, store, app, token, org) = fixture().await;
        let identity = store.identity();
        let owner = identity.authenticate_session(&token).await.unwrap();
        let user = identity
            .register_local_identity("http-scoped-admin", "Admin", PASSWORD)
            .await
            .unwrap();
        let issued = identity
            .login("http-scoped-admin", PASSWORD, 60_000)
            .await
            .unwrap();
        let membership = identity
            .add_member(&owner, org, user.id, OrganizationRole::Admin)
            .await
            .unwrap();
        let path = format!("/organizations/{org}/projects");
        let (_, project) = call(
            &app,
            "POST",
            &path,
            &token,
            Some(json!({"name":"Private HTTP project", "idempotency_key":"private-http"})),
        )
        .await;
        let id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
        assert_eq!(
            call(&app, "GET", &path, &issued.token, None).await.1,
            json!([])
        );
        assert_eq!(
            call(&app, "GET", &format!("{path}/{id}"), &issued.token, None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        identity
            .grant_content(
                &owner,
                org,
                id,
                membership.id,
                0,
                Permissions::read_only(),
                true,
            )
            .await
            .unwrap();
        assert_eq!(
            call(&app, "GET", &format!("{path}/{id}"), &issued.token, None)
                .await
                .1,
            project
        );
        identity
            .change_member(&owner, org, membership.id, 1, membership.role, false)
            .await
            .unwrap();
        assert_eq!(
            call(&app, "GET", &format!("{path}/{id}"), &issued.token, None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        identity
            .add_member(&owner, org, user.id, OrganizationRole::Member)
            .await
            .unwrap();
        assert_eq!(
            call(&app, "GET", &path, &issued.token, None).await.1,
            json!([])
        );
        let other = identity
            .create_organization(&owner, "Other HTTP tenant", "other-http-org")
            .await
            .unwrap();
        assert_eq!(
            call(
                &app,
                "GET",
                &format!("/organizations/{}/projects/{id}", other.id),
                &issued.token,
                None
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        store.close().await;
    }
}
