//! Tenant-only HTTP surface. A daemon single-owner bearer token cannot create a
//! Principal. Setup/settlement secrets are never accepted as session authority.
use crate::*;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Path, Query, Request, State},
    http::{StatusCode, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;
mod audit_http;

#[derive(Clone)]
struct HttpState {
    store: IdentityStore,
    admission: Arc<Mutex<Admission>>,
    audit_authority: Option<crate::audit::AuditAuthority>,
}
struct Admission {
    opened: Instant,
    all: u32,
    credentials: u32,
}

/// A separate router to nest under `/api/v2/identity`. It has no product tool,
/// single-owner, installation-setup or executor-settlement endpoint. Limits are
/// conservative per-process totals; horizontal deployment needs a shared edge
/// limiter too. Forwarded IP headers never establish identity or bypass limits.
pub fn router(store: IdentityStore) -> Router {
    router_with_audit(store, None)
}
pub fn router_with_audit(
    store: IdentityStore,
    audit_authority: Option<crate::audit::AuditAuthority>,
) -> Router {
    let state = HttpState {
        store,
        audit_authority,
        admission: Arc::new(Mutex::new(Admission {
            opened: Instant::now(),
            all: 0,
            credentials: 0,
        })),
    };
    Router::new()
        .route("/bootstrap", post(bootstrap))
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/me/sessions", get(account_sessions))
        .route(
            "/me/sessions/{session}/revoke",
            post(revoke_account_session),
        )
        .route("/me/audit", get(account_audit))
        .route("/identities", get(account_directory))
        .route("/credentials/password", post(change_password))
        .route(
            "/organizations",
            get(organizations).post(create_organization),
        )
        .route("/organizations/{org}", get(get_organization))
        .route("/organizations/{org}/state", put(set_state))
        .route(
            "/organizations/{org}/members",
            get(members).post(add_member),
        )
        .route("/organizations/{org}/members/{member}", put(change_member))
        .route(
            "/organizations/{org}/resources",
            get(resources).post(create_resource),
        )
        .route(
            "/organizations/{org}/resources/{resource}/grants",
            get(content_grants),
        )
        .route(
            "/organizations/{org}/resources/{resource}/grants/{member}",
            put(grant_content),
        )
        .route(
            "/organizations/{org}/quota",
            get(org_quota).put(set_org_quota),
        )
        .route(
            "/platform/quota",
            get(platform_quota).put(set_platform_quota),
        )
        .route("/identities/{identity}/disable", post(disable_identity))
        .route(
            "/identities/{identity}/sessions/revoke",
            post(revoke_sessions),
        )
        .route("/organizations/{org}/audit", get(audit))
        .merge(audit_http::router())
        .layer(DefaultBodyLimit::max(32 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), rate_limit))
        .with_state(state)
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}
fn failure(status: StatusCode, error: &'static str) -> Response {
    let mut response = (status, Json(ErrorBody { error })).into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().expect("static header"));
    response.headers_mut().insert(
        "x-content-type-options",
        "nosniff".parse().expect("static header"),
    );
    response
}
struct ApiError(IdentityError);
impl From<IdentityError> for ApiError {
    fn from(value: IdentityError) -> Self {
        Self(value)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self.0 {
            IdentityError::Validation => failure(StatusCode::BAD_REQUEST, "invalid_request"),
            IdentityError::Unauthenticated => {
                failure(StatusCode::UNAUTHORIZED, "authentication_required")
            }
            // The same response prevents probing foreign content/membership IDs.
            IdentityError::Forbidden | IdentityError::NotFound => {
                failure(StatusCode::NOT_FOUND, "access_denied")
            }
            IdentityError::Conflict => failure(StatusCode::CONFLICT, "request_conflict"),
            IdentityError::QuotaExceeded => failure(StatusCode::CONFLICT, "quota_exhausted"),
            IdentityError::NativeAuditKeyUnavailable => failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "native_audit_key_unavailable",
            ),
            IdentityError::ClockRollback
            | IdentityError::TransactionPoisoned
            | IdentityError::Storage(_)
            | IdentityError::Corrupt => {
                failure(StatusCode::SERVICE_UNAVAILABLE, "identity_unavailable")
            }
        }
    }
}
type ApiResult<T> = std::result::Result<Json<T>, ApiError>;

async fn rate_limit(State(state): State<HttpState>, request: Request, next: Next) -> Response {
    let credential_route = ["/bootstrap", "/register", "/login", "/credentials/password"]
        .iter()
        .any(|path| request.uri().path().ends_with(path));
    let admitted = match state.admission.lock() {
        Ok(mut bucket) => {
            if bucket.opened.elapsed() >= Duration::from_secs(60) {
                bucket.opened = Instant::now();
                bucket.all = 0;
                bucket.credentials = 0;
            }
            if bucket.all >= 600 || (credential_route && bucket.credentials >= 20) {
                false
            } else {
                bucket.all += 1;
                if credential_route {
                    bucket.credentials += 1;
                }
                true
            }
        }
        Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE, "identity_unavailable"),
    };
    if !admitted {
        return failure(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let mut response = next.run(request).await;
    // Credentials are returned only in explicit login/bootstrap responses.
    // Prevent browser/proxy caching of credentials and scoped tenant metadata.
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().expect("static header"));
    response.headers_mut().insert(
        "x-content-type-options",
        "nosniff".parse().expect("static header"),
    );
    response
}

struct Session(Principal);
impl FromRequestParts<HttpState> for Session {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &HttpState,
    ) -> std::result::Result<Self, Response> {
        let mut values = parts.headers.get_all("authorization").iter();
        let value = values
            .next()
            .ok_or_else(|| failure(StatusCode::UNAUTHORIZED, "authentication_required"))?;
        if values.next().is_some() {
            return Err(failure(StatusCode::UNAUTHORIZED, "authentication_required"));
        }
        let token = value
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| failure(StatusCode::UNAUTHORIZED, "authentication_required"))?;
        let actor = state
            .store
            .authenticate_session(token)
            .await
            .map_err(|error| ApiError(error).into_response())?;
        let path = parts
            .extensions
            .get::<axum::extract::OriginalUri>()
            .map(|v| v.0.path())
            .unwrap_or(parts.uri.path());
        state
            .store
            .android_management_request(&actor, parts.method.as_str(), path)
            .await
            .map_err(|_| failure(StatusCode::FORBIDDEN, "management_mfa_required"))?;
        Ok(Self(actor))
    }
}
struct StrictJson<T>(T);
impl<S, T> FromRequest<S> for StrictJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
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
                failure(status, "invalid_request")
            })
    }
}
struct StrictPath<T>(T);
impl<S, T> FromRequestParts<S> for StrictPath<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> std::result::Result<Self, Response> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|_| failure(StatusCode::BAD_REQUEST, "invalid_request"))
    }
}
struct StrictQuery<T>(T);
impl<S, T> FromRequestParts<S> for StrictQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> std::result::Result<Self, Response> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|_| failure(StatusCode::BAD_REQUEST, "invalid_request"))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    bootstrap_secret: String,
    username: String,
    display_name: String,
    password: String,
    organization_name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    username: String,
    display_name: String,
    password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    username: String,
    password: String,
    ttl_millis: i64,
}
#[derive(Serialize)]
struct LoginReply {
    token: String,
    identity: Identity,
    expires_at: i64,
}
impl From<SessionIssued> for LoginReply {
    fn from(value: SessionIssued) -> Self {
        Self {
            token: value.token,
            identity: value.identity,
            expires_at: value.expires_at,
        }
    }
}
#[derive(Serialize)]
struct BootstrapReply {
    organization: Organization,
    session: LoginReply,
}
#[derive(Serialize)]
struct Acknowledgement {
    accepted: bool,
}
async fn bootstrap(
    State(state): State<HttpState>,
    StrictJson(body): StrictJson<Bootstrap>,
) -> ApiResult<BootstrapReply> {
    let (organization, session) = state
        .store
        .bootstrap(
            &body.bootstrap_secret,
            &body.username,
            &body.display_name,
            &body.password,
            &body.organization_name,
        )
        .await?;
    Ok(Json(BootstrapReply {
        organization,
        session: session.into(),
    }))
}
async fn register(
    State(state): State<HttpState>,
    StrictJson(body): StrictJson<Registration>,
) -> ApiResult<Identity> {
    Ok(Json(
        state
            .store
            .register_local_identity(&body.username, &body.display_name, &body.password)
            .await?,
    ))
}
async fn login(
    State(state): State<HttpState>,
    StrictJson(body): StrictJson<Login>,
) -> ApiResult<LoginReply> {
    Ok(Json(
        state
            .store
            .login(&body.username, &body.password, body.ttl_millis)
            .await?
            .into(),
    ))
}
async fn logout(
    State(state): State<HttpState>,
    Session(actor): Session,
) -> ApiResult<Acknowledgement> {
    state.store.logout(&actor).await?;
    Ok(Json(Acknowledgement { accepted: true }))
}
async fn me(State(state): State<HttpState>, Session(actor): Session) -> ApiResult<Identity> {
    Ok(Json(state.store.self_identity(&actor).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateOrganization {
    name: String,
    idempotency_key: String,
}
async fn organizations(
    State(state): State<HttpState>,
    Session(actor): Session,
) -> ApiResult<Vec<Organization>> {
    Ok(Json(state.store.organizations(&actor).await?))
}
async fn create_organization(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictJson(body): StrictJson<CreateOrganization>,
) -> ApiResult<Organization> {
    Ok(Json(
        state
            .store
            .create_organization(&actor, &body.name, &body.idempotency_key)
            .await?,
    ))
}
async fn get_organization(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath(org): StrictPath<Uuid>,
) -> ApiResult<Organization> {
    Ok(Json(state.store.organization(&actor, org).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetState {
    expected_revision: i64,
    state: OrganizationState,
}
async fn set_state(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath(org): StrictPath<Uuid>,
    StrictJson(body): StrictJson<SetState>,
) -> ApiResult<Organization> {
    Ok(Json(
        state
            .store
            .set_organization_state(&actor, org, body.expected_revision, body.state)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddMember {
    identity_id: Uuid,
    role: OrganizationRole,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeMember {
    expected_revision: i64,
    role: OrganizationRole,
    active: bool,
}
async fn members(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath(org): StrictPath<Uuid>,
) -> ApiResult<Vec<Membership>> {
    Ok(Json(state.store.members(&actor, org).await?))
}
async fn add_member(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath(org): StrictPath<Uuid>,
    StrictJson(body): StrictJson<AddMember>,
) -> ApiResult<Membership> {
    Ok(Json(
        state
            .store
            .add_member(&actor, org, body.identity_id, body.role)
            .await?,
    ))
}
async fn change_member(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath((org, member)): StrictPath<(Uuid, Uuid)>,
    StrictJson(body): StrictJson<ChangeMember>,
) -> ApiResult<Membership> {
    Ok(Json(
        state
            .store
            .change_member(
                &actor,
                org,
                member,
                body.expected_revision,
                body.role,
                body.active,
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateResource {
    kind: ContentKind,
    idempotency_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantContent {
    expected_revision: i64,
    permissions: Permissions,
    active: bool,
}
async fn resources(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath(org): StrictPath<Uuid>,
) -> ApiResult<Vec<ContentResource>> {
    Ok(Json(state.store.resources(&actor, org).await?))
}
async fn create_resource(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath(org): StrictPath<Uuid>,
    StrictJson(body): StrictJson<CreateResource>,
) -> ApiResult<ContentResource> {
    Ok(Json(
        state
            .store
            .create_resource(&actor, org, body.kind, &body.idempotency_key)
            .await?,
    ))
}
async fn content_grants(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath((org, resource)): StrictPath<(Uuid, Uuid)>,
) -> ApiResult<Vec<ContentGrant>> {
    Ok(Json(
        state.store.content_grants(&actor, org, resource).await?,
    ))
}
async fn grant_content(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath((org, resource, member)): StrictPath<(Uuid, Uuid, Uuid)>,
    StrictJson(body): StrictJson<GrantContent>,
) -> ApiResult<ContentGrant> {
    Ok(Json(
        state
            .store
            .grant_content(
                &actor,
                org,
                resource,
                member,
                body.expected_revision,
                body.permissions,
                body.active,
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuotaPlan {
    expected_revision: i64,
    limit: QuotaAmounts,
}
async fn org_quota(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath(org): StrictPath<Uuid>,
) -> ApiResult<QuotaStatus> {
    Ok(Json(
        state
            .store
            .quota_status(&actor, QuotaScope::Organization(org))
            .await?,
    ))
}
async fn set_org_quota(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath(org): StrictPath<Uuid>,
    StrictJson(body): StrictJson<QuotaPlan>,
) -> ApiResult<QuotaStatus> {
    Ok(Json(
        state
            .store
            .configure_quota(
                &actor,
                QuotaScope::Organization(org),
                body.expected_revision,
                body.limit,
            )
            .await?,
    ))
}
async fn platform_quota(
    State(state): State<HttpState>,
    Session(actor): Session,
) -> ApiResult<QuotaStatus> {
    Ok(Json(
        state
            .store
            .quota_status(&actor, QuotaScope::Platform)
            .await?,
    ))
}
async fn set_platform_quota(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictJson(body): StrictJson<QuotaPlan>,
) -> ApiResult<QuotaStatus> {
    Ok(Json(
        state
            .store
            .configure_quota(
                &actor,
                QuotaScope::Platform,
                body.expected_revision,
                body.limit,
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    expected_revision: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangePassword {
    old_password: String,
    new_password: String,
    ttl_millis: i64,
}
async fn change_password(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictJson(body): StrictJson<ChangePassword>,
) -> ApiResult<LoginReply> {
    Ok(Json(
        state
            .store
            .change_password(
                &actor,
                &body.old_password,
                &body.new_password,
                body.ttl_millis,
            )
            .await?
            .into(),
    ))
}
async fn revoke_sessions(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath(identity): StrictPath<Uuid>,
    StrictJson(body): StrictJson<Revision>,
) -> ApiResult<Acknowledgement> {
    state
        .store
        .revoke_identity_sessions(&actor, identity, body.expected_revision)
        .await?;
    Ok(Json(Acknowledgement { accepted: true }))
}
async fn account_sessions(
    State(state): State<HttpState>,
    Session(actor): Session,
) -> ApiResult<Vec<AccountSession>> {
    Ok(Json(state.store.account_sessions(&actor).await?))
}
async fn revoke_account_session(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath(session): StrictPath<Uuid>,
    StrictJson(body): StrictJson<Revision>,
) -> ApiResult<Acknowledgement> {
    state
        .store
        .revoke_account_session(&actor, session, body.expected_revision)
        .await?;
    Ok(Json(Acknowledgement { accepted: true }))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryQuery {
    #[serde(default)]
    query: String,
    after: Option<Uuid>,
    #[serde(default = "directory_limit")]
    limit: u32,
}
fn directory_limit() -> u32 {
    100
}
async fn account_directory(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictQuery(q): StrictQuery<DirectoryQuery>,
) -> ApiResult<AccountDirectoryPage> {
    Ok(Json(
        state
            .store
            .account_directory(&actor, &q.query, q.after, q.limit)
            .await?,
    ))
}
async fn account_audit(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictQuery(query): StrictQuery<AuditQuery>,
) -> ApiResult<Vec<AuditEntry>> {
    Ok(Json(
        state
            .store
            .account_audit(&actor, query.after_sequence, query.limit)
            .await?,
    ))
}
async fn disable_identity(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath(identity): StrictPath<Uuid>,
    StrictJson(body): StrictJson<Revision>,
) -> ApiResult<Identity> {
    Ok(Json(
        state
            .store
            .disable_identity(&actor, identity, body.expected_revision)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditQuery {
    #[serde(default)]
    after_sequence: i64,
    #[serde(default = "default_limit")]
    limit: i64,
}
fn default_limit() -> i64 {
    100
}
async fn audit(
    State(state): State<HttpState>,
    Session(actor): Session,
    StrictPath(org): StrictPath<Uuid>,
    StrictQuery(query): StrictQuery<AuditQuery>,
) -> ApiResult<Vec<AuditEntry>> {
    Ok(Json(
        state
            .store
            .audit(&actor, org, query.after_sequence, query.limit)
            .await?,
    ))
}
